# /// script
# requires-python = ">=3.11"
# ///
"""本機實機測試：把 dist/ 的工作坊包掛進遊戲、啟動、下 DFHack 指令、收尾還原。

用法（repo 根目錄，先跑 scripts/build.py）：
  uv run scripts/dev_game.py up [--lang zh-Hant|zh-Hans|both] [--with-dfi18n]   掛包、遮蔽 DFI18n（--with-dfi18n 不遮蔽）、啟動，等到 DFHack 回應；
                                                               只測一種語言時寫進測試用的 dflang.json，載入的不是 dist 的包就失敗
  uv run scripts/dev_game.py run dflang status                 轉給 dfhack-run
  uv run scripts/dev_game.py click X Y                         以文字格座標模擬滑鼠左鍵（不動真滑鼠、不搶焦點）
  uv run scripts/dev_game.py mouse PX PY [r]                   以畫面像素座標按住滑鼠三幀再放開（地圖點擊用）
  uv run scripts/dev_game.py sweep                             從目前畫面把分頁一路點開：量每個分頁標籤有沒有置中、收集未翻譯紀錄
  uv run scripts/dev_game.py down                              關遊戲、拆掛載、移除遮蔽，還原 dflang 設定檔

點擊方式依畫面而異：標題選單、設定用 click（DFHack simulateInput）；世界地圖與出發地圖在遊戲邏輯迴圈裡讀滑鼠，
要用 mouse 讓按鍵狀態維持幾幀；出發準備畫面的按鈕兩種都不收，改用 computer 對遊戲視窗先移過去再點。

遮蔽 DFI18n：在 mods/ 放一個 ID 也是 dfi18n、版本 999、沒有腳本的空 MOD。DFHack 每個 ID 只載入最高版本，
真的 DFI18n 腳本就不會執行；down 會把它刪掉。掛載位置是 %APPDATA%/Bay 12 Games/Dwarf Fortress/mods
（非 Portable Mode 的預設）。

遊戲目錄 dfhack-config/ 裡原有的 dflang* 檔（玩家自己的語言設定、未翻譯紀錄）在 up 時移到
temp/dfhack-config-backup/，down 時放回；測試期間的未翻譯紀錄（dflang misses on）在 down 時另存到
temp/misses/<時間>.tsv，給 scripts/inventory.py 用。
"""
import ctypes
import ctypes.wintypes as wt
import functools
import json
import os
import shutil
import subprocess
import sys
import time
from collections import Counter
from pathlib import Path

from dfpaths import DF, DFHACK

ROOT = Path(__file__).resolve().parents[1]
MODS = Path(os.environ["APPDATA"]) / "Bay 12 Games/Dwarf Fortress/mods"
SHADOW = MODS / "dflang-test-shadow-dfi18n"
CONFIG_DIR = DF / "dfhack-config"
BACKUP = ROOT / "temp/dfhack-config-backup"
MISSES_ARCHIVE = ROOT / "temp/misses"
DFHACK_RUN = DFHACK / "hack/dfhack-run.exe"
ITEMS = json.loads((ROOT / "scripts/workshop.json").read_text(encoding="utf-8"))["items"]


def dfhack(*args: str, quiet: bool = False) -> subprocess.CompletedProcess:
    result = subprocess.run([str(DFHACK_RUN), *args], capture_output=True, text=True, encoding="utf-8", errors="replace")
    if not quiet:
        print(result.stdout, end="")
        print(result.stderr, end="", file=sys.stderr)
    return result


def kill_game() -> None:
    subprocess.run(["taskkill", "/IM", "Dwarf Fortress.exe", "/F"], capture_output=True)
    time.sleep(3)


def up(langs: list[str], with_dfi18n: bool = False) -> int:
    kill_game()
    # 遮蔽 MOD 或掛載連結還在表示測試進行中（up 了兩次沒有 down），這時的 dflang* 是測試自己產生的，不是玩家的，不備份
    testing = SHADOW.exists() or any(os.path.isjunction(MODS / item["folder"]) for item in ITEMS)
    user_files = list(CONFIG_DIR.glob("dflang*"))
    if user_files and not BACKUP.exists() and not testing:
        BACKUP.mkdir(parents=True)
        for f in user_files:
            shutil.move(f, BACKUP / f.name)
    if not with_dfi18n:
        SHADOW.mkdir(parents=True, exist_ok=True)
        (SHADOW / "info.txt").write_text(
            "[ID:dfi18n]\n[NUMERIC_VERSION:999]\n[DISPLAYED_VERSION:test-shadow]\n"
            "[EARLIEST_COMPATIBLE_NUMERIC_VERSION:1]\n[EARLIEST_COMPATIBLE_DISPLAYED_VERSION:test]\n"
            "[AUTHOR:DFLang test]\n[NAME:DFLang test shadow for DFI18n]\n"
            "[DESCRIPTION:Temporary local stub that hides DFI18n scripts during DFLang tests.]\n", encoding="ascii")
    for item in ITEMS:
        if item["lang"] not in langs:
            continue
        src, link = ROOT / "dist" / item["folder"], MODS / item["folder"]
        if not src.is_dir():
            print(f"缺 {src}；先跑 uv run scripts/build.py", file=sys.stderr)
            return 1
        if not link.exists():
            subprocess.run(["cmd", "/c", "mklink", "/J", str(link), str(src)], check=True, capture_output=True)
    if len(langs) == 1:
        # 玩家訂閱了本 MOD 時，工作坊的副本（mods/<ID> (<版本>)）讓另一種語言也算已安裝，載入器會照預設選繁中：指定要測的語言
        config = CONFIG_DIR / "dflang.json"
        data = json.loads(config.read_text(encoding="utf-8")) if config.is_file() else {}
        config.write_text(json.dumps({**data, "lang": langs[0]}), encoding="utf-8")
    os.startfile("steam://rungameid/975370")
    deadline = time.time() + 180
    while time.time() < deadline:
        time.sleep(3)
        if "ready" in dfhack("lua", "print('ready')", quiet=True).stdout:
            time.sleep(5)  # 等標題畫面畫出第一幀
            # 同 ID 的包 DFHack 載入版本最高的，同版本取最後列出的（mods/ 的掛載連結排在工作坊副本後面）；
            # 工作坊的版本比 dist 新時會載入工作坊的包，測到的就不是這次建置的 DLL
            loaded = loaded_package()
            if loaded and not is_dist(loaded, langs):
                print(f"載入的不是 dist 的包：{loaded}", file=sys.stderr)
                return 1
            return dfhack("dflang", "status").returncode
    print("等不到 DFHack 回應", file=sys.stderr)
    return 1


def down() -> int:
    kill_game()
    for item in ITEMS:
        link = MODS / item["folder"]
        if os.path.isjunction(link):
            os.rmdir(link)  # 只移除連結，不動 dist/
    shutil.rmtree(SHADOW, ignore_errors=True)
    misses = CONFIG_DIR / "dflang-misses.tsv"
    if misses.is_file() and misses.stat().st_size:
        MISSES_ARCHIVE.mkdir(parents=True, exist_ok=True)
        shutil.copy2(misses, MISSES_ARCHIVE / time.strftime("%Y%m%d-%H%M%S.tsv"))
    for f in CONFIG_DIR.glob("dflang*"):
        f.unlink()
    if BACKUP.is_dir():
        for f in BACKUP.iterdir():
            shutil.move(f, CONFIG_DIR / f.name)
        BACKUP.rmdir()
    print("已還原：", sorted(p.name for p in MODS.iterdir()))
    return 0


# 先移過去停兩幀（地圖依上一幀的游標位置算出指著哪一格），再按住三幀、放開後位置再留幾幀。
# 真滑鼠不在遊戲視窗上時 mouse_focus 是 false，遊戲每幀把游標位置清成 -1，所以每幀都要設回去。
# dfhack.timeout 的回呼在遊戲主執行緒的後續幀執行
MOUSE_HOLD = """local px, py, right = {px}, {py}, {right}
local g, e = df.global.gps, df.global.enabler
local function at() e.mouse_focus = true; g.precise_mouse_x, g.precise_mouse_y = px, py; g.mouse_x, g.mouse_y = px // g.tile_pixel_x, py // g.tile_pixel_y end
local function press(v) if right then e.mouse_rbut, e.mouse_rbut_down = v, v else e.mouse_lbut, e.mouse_lbut_down = v, v end end
at()
for i = 1, 8 do dfhack.timeout(i, 'frames', at) end
dfhack.timeout(2, 'frames', function() press(1) end)
dfhack.timeout(5, 'frames', function() press(0) end)"""


def loaded_package() -> str:
    """引擎從哪個包載入（DFLANG.dir）；引擎沒在跑是空字串"""
    return dfhack("lua", "local s = dfhack.BASE_G.DFLANG; print(s and s.dir or '')", quiet=True).stdout.strip()


def is_dist(path: str, langs: list[str]) -> bool:
    return any(f"/mods/{item['folder']}/" in path for item in ITEMS if item["lang"] in langs)


# ---- sweep：從目前畫面把分頁一路點開，量每個分頁標籤的字有沒有置中 ----
#
# 引擎判成置中（overlay_dump 的 align 是 c）不代表畫面上置中：遊戲的分頁英文本身比分頁正中偏右半格，
# 中文曾經照英文的正中間畫、整排偏右 5 px，align 對、目測也看不出來。所以量畫出來的像素：
# 框的中心從 screentexpos_lower 的底圖中段算，字的中心從視窗截圖的筆畫量。

# 本幀整句貼圖記下、畫在底圖上的半格位移上列（分頁標籤）：第一行是焦點與格子像素大小，
# 之後每行 x0、y、原文格數、底圖中段 [l, r)、對齊、英文（翻譯過的格子由 english_at 讀回）
SWEEP_LUA = r"""local g, s = df.global.gps, dfhack.BASE_G.DFLANG
print(table.concat(dfhack.gui.getCurFocus(true), '|'), g.tile_pixel_x, g.tile_pixel_y)
for line in s.engine.overlay_dump():gmatch('[^\n]+') do
  local top, row, x0, y, shadow, align = line:match('^(%d)\t(%a+)\t(%-?%d+)\t(%-?%d+)\t%[(.-)%]\t%d+\t(%a)')
  if row == 'Top' then
    x0, y = tonumber(x0), tonumber(y)
    local lower = top == '1' and g.screentexpos_top_lower or g.screentexpos_lower
    local function at(x) return (x >= 0 and x < g.dimx) and lower[x * g.dimy + y] or 0 end
    local mid = at(x0)
    if mid ~= 0 then
      local l, r = x0, x0
      while at(l - 1) == mid do l = l - 1 end
      while at(r) == mid do r = r + 1 end
      local en = shadow
      if not en:find('%S') then
        local t = {}
        for x = x0, x0 + #shadow - 1 do local c = s.engine.english_at(x, y); t[#t + 1] = c and string.char(c) or ' ' end
        en = table.concat(t)
      end
      print(x0, y, #shadow, l, r, align, en)
    end
  end
end"""
# 第一次打開某分頁會跳出說明彈窗（模態，擋住之後的點擊）；按 Esc 會連面板一起關
CLOSE_HELP = "pcall(function() df.global.game.main_interface.help.open = false end)"
MAX_CLICKS = 400


class BitmapInfoHeader(ctypes.Structure):
    _fields_ = [("size", wt.DWORD), ("width", wt.LONG), ("height", wt.LONG), ("planes", wt.WORD), ("bit_count", wt.WORD),
                ("compression", wt.DWORD), ("size_image", wt.DWORD), ("x_per_meter", wt.LONG), ("y_per_meter", wt.LONG),
                ("colors_used", wt.DWORD), ("colors_important", wt.DWORD)]


@functools.cache
def win32():
    """user32、gdi32，回傳值是指標的函式設好型別（預設的 int 會截斷 64 位元的 handle）"""
    user32, gdi32 = ctypes.windll.user32, ctypes.windll.gdi32
    user32.SetProcessDpiAwarenessContext(ctypes.c_void_p(-4))  # 依螢幕 DPI：視窗大小、游標位置都是實際像素
    for f, res, args in ((user32.FindWindowW, wt.HWND, [wt.LPCWSTR, wt.LPCWSTR]), (user32.GetDC, wt.HDC, [wt.HWND]),
                         (gdi32.CreateCompatibleDC, wt.HDC, [wt.HDC]), (gdi32.SelectObject, wt.HGDIOBJ, [wt.HDC, wt.HGDIOBJ]),
                         (gdi32.CreateDIBSection, wt.HBITMAP, [wt.HDC, ctypes.c_void_p, wt.UINT, ctypes.POINTER(ctypes.c_void_p), wt.HANDLE, wt.DWORD]),
                         (user32.PrintWindow, wt.BOOL, [wt.HWND, wt.HDC, wt.UINT]), (user32.ReleaseDC, ctypes.c_int, [wt.HWND, wt.HDC]),
                         (gdi32.DeleteObject, wt.BOOL, [wt.HGDIOBJ]), (gdi32.DeleteDC, wt.BOOL, [wt.HDC]),
                         (user32.WindowFromPoint, wt.HWND, [wt.POINT]), (user32.GetAncestor, wt.HWND, [wt.HWND, wt.UINT])):
        f.restype, f.argtypes = res, args
    return user32, gdi32


def cursor_on_game() -> bool:
    """真游標停在遊戲視窗上（游標底下最上層的視窗是遊戲；被別的視窗蓋住的部分不算）：遊戲讀作業系統的游標位置，
    模擬的點擊會被它蓋掉、點到別的地方"""
    user32, _ = win32()
    hwnd = user32.FindWindowW("SDL_app", "Dwarf Fortress")
    point = wt.POINT()
    if not hwnd or not user32.GetCursorPos(ctypes.byref(point)):
        return False
    return user32.GetAncestor(user32.WindowFromPoint(point), 2) == hwnd  # GA_ROOT


def capture() -> tuple[int, bytes] | None:
    """遊戲視窗客戶區的畫面（原生解析度，每像素 BGRA）：PrintWindow 加 PW_RENDERFULLCONTENT，視窗被蓋住也抓得到"""
    user32, gdi32 = win32()
    hwnd = user32.FindWindowW("SDL_app", "Dwarf Fortress")
    rect = wt.RECT()
    if not hwnd or not user32.GetClientRect(hwnd, ctypes.byref(rect)):
        return None
    w, h = rect.right, rect.bottom
    header = BitmapInfoHeader(ctypes.sizeof(BitmapInfoHeader), w, -h, 1, 32)  # 高度給負的：由上往下排
    window_dc = user32.GetDC(hwnd)
    dc = gdi32.CreateCompatibleDC(window_dc)
    bits = ctypes.c_void_p()
    bitmap = gdi32.CreateDIBSection(dc, ctypes.byref(header), 0, ctypes.byref(bits), None, 0)
    old = gdi32.SelectObject(dc, bitmap)
    ok = user32.PrintWindow(hwnd, dc, 3)  # PW_CLIENTONLY | PW_RENDERFULLCONTENT
    data = ctypes.string_at(bits, w * h * 4) if ok else None
    gdi32.SelectObject(dc, old)
    gdi32.DeleteObject(bitmap)
    gdi32.DeleteDC(dc)
    user32.ReleaseDC(hwnd, window_dc)
    return (w, data) if data else None


def ink_center(img: bytes, w: int, x_lo: int, x_hi: int, y_lo: int, y_hi: int) -> float | None:
    """區塊裡字的筆畫最左、最右兩端的中點（像素）：底色取最多的顏色，整列都和底色不同的是邊框亮線、不算"""
    rows = [[img[(y * w + x) * 4:(y * w + x) * 4 + 3] for x in range(x_lo, x_hi)] for y in range(y_lo, y_hi)]
    bg = Counter(p for row in rows for p in row).most_common(1)[0][0]
    cols = set()
    for row in rows:
        far = [sum(abs(a - b) for a, b in zip(p, bg)) > 150 for p in row]
        if sum(far) < 0.9 * len(far):
            cols.update(i for i, f in enumerate(far) if f)
    return x_lo + (min(cols) + max(cols) + 1) / 2 if cols else None


def sweep() -> int:
    loaded = loaded_package()
    if not is_dist(loaded, [item["lang"] for item in ITEMS]):
        print(f"載入的不是 dist 的包（{loaded or '引擎沒在跑'}）：先跑 up", file=sys.stderr)
        return 1
    misses = CONFIG_DIR / "dflang-misses.tsv"
    dfhack("dflang", "misses", "on", quiet=True)
    before = len(misses.read_text(encoding="utf-8").splitlines()) if misses.is_file() else 0
    worst: dict[tuple[int, int, str], tuple[float, str]] = {}  # (x0, y, 英文) → (最大偏移 px, 焦點)
    seen_rows: set = set()
    # stopped：中途停下的原因（點到別的畫面、截圖失敗、真游標一直停在遊戲視窗上、點太多次），有就算失敗：只檢查了一部分
    count = {"states": 0, "clicks": 0, "stopped": "", "root": ""}
    cell = [10, 16]

    def look() -> list[tuple[int, int, int, int, int, str]] | None:
        out = dfhack("lua", SWEEP_LUA, quiet=True).stdout.splitlines()
        shot = capture()
        if not out or not shot:
            count["stopped"] = "讀不到畫面（遊戲關了？）"
            return None
        focus, dx, dy = out[0].split("\t")
        # 點分頁只會換下層的焦點（dwarfmode/ViewSheets/UNIT/… 的後段）；開頭兩段變了是點到別的地方
        root = "/".join(focus.split("|")[0].split("/")[:2])
        if count["root"] and root != count["root"]:
            count["stopped"] = f"畫面跳到 {focus}：點到別的地方了（多半是真滑鼠動到遊戲視窗）"
            return None
        count["root"] = root
        cell[:] = [int(dx), int(dy)]
        dx, dy = cell
        w, img = shot
        count["states"] += 1
        tabs = []
        for line in out[1:]:
            x0, y, n, l, r, align, label = line.split("\t", 6)
            x0, y, n, l, r = int(x0), int(y), int(n), int(l), int(r)
            if align != "c":
                continue  # 半格位移但靠左的字（冒險裝備清單的分類標題）不是分頁
            tabs.append((x0, y, n, l, r, label))
            # 介面格子從視窗左上角畫起；半格位移的字畫在 y 格往下半格起的一格高
            y_lo, y_hi = y * dy + dy // 2 + 1, (y + 1) * dy + dy // 2 - 1
            if r * dx > w or (y_hi + 1) * w * 4 > len(img):
                continue
            center = ink_center(img, w, l * dx, r * dx, y_lo, y_hi)
            if center is None:
                continue
            # 英文在底圖中段裡置中（和 hooks.rs 的 centered_in 同一個條件）就該對準中段的正中間，否則照英文的正中間
            left, right = x0 - l, r - x0 - n
            boxed = abs(left - right) <= 1 or (abs(left - right) <= 2 and min(left, right) >= 4)
            offset = center - ((l + r) * dx / 2 if boxed else (x0 + n / 2) * dx)
            key = (x0, y, label.strip())
            if key not in worst or abs(offset) > abs(worst[key][0]):
                worst[key] = (offset, focus)
        return tabs

    def explore(min_y: int) -> None:
        tabs = look()
        if tabs is None:
            return
        # 最上面一排還沒點過的分頁逐一點開，每點一個就往下看新出現的子分頁；同一排（位置與標籤都相同）只展開一次
        for y0 in sorted({t[1] for t in tabs if t[1] >= min_y}):
            row = sorted(t for t in tabs if t[1] == y0)
            signature = (y0, tuple((t[0], t[2], t[5]) for t in row))
            if signature in seen_rows:
                continue
            seen_rows.add(signature)
            dx, dy = cell
            for x0, y, n, *_ in row:
                if count["clicks"] >= MAX_CLICKS:
                    count["stopped"] = f"點了 {MAX_CLICKS} 次就停了，還有分頁沒點到"
                if count["stopped"]:
                    return
                waited = time.time()
                while cursor_on_game():
                    if time.time() - waited > 60:
                        count["stopped"] = "真游標一直停在遊戲視窗上（模擬的點擊會被它蓋掉）"
                        return
                    time.sleep(0.5)
                count["clicks"] += 1
                dfhack("lua", MOUSE_HOLD.format(px=int((x0 + n / 2) * dx), py=int((y + 0.5) * dy), right="false"), quiet=True)
                time.sleep(1.0)
                dfhack("lua", CLOSE_HELP, quiet=True)
                time.sleep(0.3)
                explore(y0 + 1)
            return

    if cursor_on_game():
        print("真游標在遊戲視窗上，先移開（每次點之前也會等它移開）", file=sys.stderr)
    explore(0)
    if not worst:
        print(count["stopped"] or "畫面上沒有量得到的分頁標籤（先開到有分頁的畫面；整句貼圖要開著）", file=sys.stderr)
        return 1
    tolerance = cell[0] / 4
    fails = sorted(((abs(o), key, o, f) for key, (o, f) in worst.items() if abs(o) > tolerance), reverse=True)
    print(f"sweep：{count['states']} 個畫面、點了 {count['clicks']} 次、量了 {len(worst)} 個分頁標籤（容許 {tolerance:g} px）")
    for _, (x0, y, label), o, focus in fails:
        print(f"FAIL {o:+.1f} px ({x0},{y}) {label}  [{focus}]")
    largest = max(worst.items(), key=lambda kv: abs(kv[1][0]))
    print(f"最大偏移 {largest[1][0]:+.1f} px：({largest[0][0]},{largest[0][1]}) {largest[0][2]}")
    if count["stopped"]:
        print(f"沒有檢查完：{count['stopped']}", file=sys.stderr)
    lines = misses.read_text(encoding="utf-8").splitlines() if misses.is_file() else []
    print(f"未翻譯紀錄新增 {len(lines) - before} 筆（{misses}）")
    for line in lines[before:before + 20]:
        print("  " + line)
    return 1 if fails or count["stopped"] else 0


def main() -> int:
    args = sys.argv[1:]
    if not args or args[0] not in ("up", "run", "click", "mouse", "sweep", "down"):
        print(__doc__)
        return 2
    if args[0] == "up":
        lang = args[2] if len(args) > 2 and args[1] == "--lang" else "zh-Hant"
        return up(["zh-Hant", "zh-Hans"] if lang == "both" else [lang], "--with-dfi18n" in args)
    if args[0] == "run":
        return dfhack(*args[1:]).returncode
    if args[0] == "click":
        x, y = int(args[1]), int(args[2])
        return dfhack("lua", f"local gui=require('gui'); df.global.gps.mouse_x={x}; df.global.gps.mouse_y={y}; "
                             "gui.simulateInput(dfhack.gui.getDFViewscreen(true), '_MOUSE_L')").returncode
    if args[0] == "mouse":
        right = "true" if args[3:4] == ["r"] else "false"
        return dfhack("lua", MOUSE_HOLD.format(px=int(args[1]), py=int(args[2]), right=right)).returncode
    if args[0] == "sweep":
        return sweep()
    return down()


if __name__ == "__main__":
    sys.exit(main())
