# /// script
# requires-python = ">=3.11"
# ///
"""本機實機測試：把 dist/ 的工作坊包掛進遊戲、啟動、下 DFHack 指令、收尾還原。

用法（repo 根目錄，先跑 scripts/build.py；play 自己會建置）：
  uv run scripts/dev_game.py play [--lang zh-Hant|zh-Hans|auto]     啟動器（scripts/dev-play.cmd 雙擊執行）：建置、掛包、開遊戲，遊戲關掉後自動 down
  uv run scripts/dev_game.py up [--lang zh-Hant|zh-Hans|auto] [--with-dfi18n] [--save 名稱]...
                                                               掛包、遮蔽 DFI18n（--with-dfi18n 不遮蔽）、啟動，等到 DFHack 回應；
                                                               要測的語言寫進測試用的 dflang.json（auto 不寫，照系統判斷），載入的不是 dist 的包就失敗；
                                                               --save 把 temp/test-world/<名稱> 複製進遊戲的 save/
  uv run scripts/dev_game.py run dflang status                 轉給 dfhack-run
  uv run scripts/dev_game.py click X Y                         以文字格座標模擬滑鼠左鍵（不動真滑鼠、不搶焦點）
  uv run scripts/dev_game.py mouse PX PY [r]                   以畫面像素座標按住滑鼠三幀再放開（地圖點擊用；會結束 hover）
  uv run scripts/dev_game.py hover PX PY | off                 游標停在畫面像素座標、不按鍵（提示框這類要停留才出現的畫面），直到 hover off
  uv run scripts/dev_game.py key A_TRAVEL [LEAVESCREEN ...]    依序送遊戲按鍵（df.interface_key 的名稱），最後印出焦點
  uv run scripts/dev_game.py shot 檔名.png [--crop X Y W H] [--wait 秒]
                                                               遊戲視窗截圖：原解析度，座標和 mouse 一樣是畫面像素；
                                                               預設先等 1 秒（狀態剛變時遊戲可能還沒畫新的一幀）
  uv run scripts/dev_game.py screen                            焦點與這一幀畫面上的字（中文、沒翻的英文），一段一行
  uv run scripts/dev_game.py sweep                             從目前畫面把分頁一路點開：量每個分頁標籤有沒有置中、收集未翻譯紀錄
  uv run scripts/dev_game.py down                              關遊戲、拆掛載、移除遮蔽，還原 dflang 設定檔，移走 --save 的測試存檔

點擊方式依畫面而異：標題選單、設定用 click（DFHack simulateInput）；世界地圖與出發地圖在遊戲邏輯迴圈裡讀滑鼠，
要用 mouse 讓按鍵狀態維持幾幀；停留才出現的提示框、世界畫面的新聞用 hover。mouse、hover 用引擎的假游標
（engine/src/mouse.rs），不動真滑鼠。出發準備畫面的按鈕兩種都不收，改用 computer 對遊戲視窗先移過去再點。

遮蔽 DFI18n：在 mods/ 放一個 ID 也是 dfi18n、版本 999、沒有腳本的空 MOD。DFHack 每個 ID 只載入最高版本，
真的 DFI18n 腳本就不會執行；down 會把它刪掉。掛載位置是 %APPDATA%/Bay 12 Games/Dwarf Fortress/mods
（非 Portable Mode 的預設）。

遊戲目錄 dfhack-config/ 裡原有的 dflang* 檔（玩家自己的語言設定、未翻譯紀錄）在 up 時移到
temp/dfhack-config-backup/，down 時放回；測試期間的未翻譯紀錄（dflang misses on）在 down 時另存到
temp/misses/<時間>.tsv，給 scripts/inventory.py 用。

開發用的 DFHack 腳本 scripts/dev_lua/<名稱>.lua 在 up 時裝成 dfhack-config/scripts/dflang-dev-<名稱>.lua
（`run dflang-dev-<名稱>` 執行），down 時刪掉；它們寫在 dfhack-config/ 的 dflang-*.tsv 在 down 時另存到 temp/dumps/。
--save 放進去的測試存檔記在 temp/dev-game-saves.json：down 時和來源相同就刪掉，被遊戲存過的搬到 temp/saves/；
save/ 已有同名的存檔（可能是玩家自己的）時 up 失敗，不覆蓋。
"""
import ctypes
import ctypes.wintypes as wt
import filecmp
import functools
import json
import os
import re
import shutil
import struct
import subprocess
import sys
import time
import zlib
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
WORKSHOP = json.loads((ROOT / "scripts/workshop.json").read_text(encoding="utf-8"))
FOLDER = WORKSHOP["folder"]
LANGS = [language["lang"] for language in WORKSHOP["languages"]]
LINK = MODS / FOLDER
SAVES = MODS.parent / "save"
TEST_WORLDS = ROOT / "temp/test-world"
SAVE_STATE = ROOT / "temp/dev-game-saves.json"
SAVE_KEEP = ROOT / "temp/saves"
DEV_LUA = ROOT / "scripts/dev_lua"
DEV_SCRIPTS = CONFIG_DIR / "scripts"  # DFHack 的腳本路徑之一
DUMPS_ARCHIVE = ROOT / "temp/dumps"


def dfhack(*args: str, quiet: bool = False) -> subprocess.CompletedProcess:
    result = subprocess.run([str(DFHACK_RUN), *args], capture_output=True, text=True, encoding="utf-8", errors="replace")
    if not quiet:
        print(result.stdout, end="")
        print(result.stderr, end="", file=sys.stderr)
    return result


def game_running() -> bool:
    out = subprocess.run(["tasklist", "/FI", "IMAGENAME eq Dwarf Fortress.exe", "/NH"], capture_output=True, text=True).stdout
    return "Dwarf Fortress.exe" in out


def kill_game() -> None:
    subprocess.run(["taskkill", "/IM", "Dwarf Fortress.exe", "/F"], capture_output=True)
    time.sleep(3)


def play(lang: str | None) -> int:
    """啟動器：建置、掛上 dist、開遊戲，等遊戲關掉再 down。up 與 down 都會強制關遊戲，所以遊戲開著時不動手"""
    if game_running():
        print("遊戲正在執行：先存檔、關掉遊戲，再開啟動器", file=sys.stderr)
        return 1
    # 上次的視窗在遊戲開著時被關掉：掛載與暫移的玩家設定還在，先還原再開始
    if SHADOW.exists() or BACKUP.exists() or os.path.isjunction(LINK):
        print("上次的測試沒有收尾，先還原")
        down()
    if not lang:
        lang = {"2": "zh-Hans", "3": "auto"}.get(input("要測哪種語言？1 繁體中文（預設）  2 简体中文  3 照系統判斷：").strip(), "zh-Hant")
    if subprocess.run(["uv", "run", str(ROOT / "scripts/build.py")], cwd=ROOT).returncode != 0:
        print("建置失敗，沒有開遊戲", file=sys.stderr)
        return 1
    started = False
    try:
        started = up(lang) == 0
        if started:
            print("\n遊戲關掉後會自動還原（拆掉掛載、放回你自己的 dflang 設定）。這個視窗先別關；"
                  "在這裡按 Ctrl+C 會直接關掉遊戲再還原，沒存檔的進度會不見")
            while game_running():
                time.sleep(3)
    except KeyboardInterrupt:
        print("\n中斷：關掉遊戲並還原")
    down()
    return 0 if started else 1


def up(lang: str, with_dfi18n: bool = False, saves: list[str] | None = None) -> int:
    if lang not in (*LANGS, "auto"):
        print(f"--lang 只能是 {'、'.join(LANGS)} 或 auto", file=sys.stderr)
        return 2
    if saves and not install_saves(saves):
        return 1
    kill_game()
    # 遮蔽 MOD 或掛載連結還在表示測試進行中（up 了兩次沒有 down），這時的 dflang* 是測試自己產生的，不是玩家的，不備份
    testing = SHADOW.exists() or os.path.isjunction(LINK)
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
    src = ROOT / "dist" / FOLDER
    if not src.is_dir():
        print(f"缺 {src}；先跑 uv run scripts/build.py", file=sys.stderr)
        return 1
    if not LINK.exists():
        subprocess.run(["cmd", "/c", "mklink", "/J", str(LINK), str(src)], check=True, capture_output=True)
    # 指定語言寫進測試用的 dflang.json；auto 拿掉 lang，遊戲照系統判斷
    config = CONFIG_DIR / "dflang.json"
    data = json.loads(config.read_text(encoding="utf-8")) if config.is_file() else {}
    data.pop("lang", None)
    if lang != "auto":
        data["lang"] = lang
    if data or config.is_file():
        config.write_text(json.dumps(data), encoding="utf-8")
    install_dev_lua()
    os.startfile("steam://rungameid/975370")
    deadline = time.time() + 180
    while time.time() < deadline:
        time.sleep(3)
        if "ready" in dfhack("lua", "print('ready')", quiet=True).stdout:
            time.sleep(5)  # 等標題畫面畫出第一幀
            # 同 ID 的包 DFHack 載入版本最高的，同版本取最後列出的（mods/ 的掛載連結排在工作坊副本後面）；
            # 工作坊的版本比 dist 新時會載入工作坊的包，測到的就不是這次建置的 DLL
            loaded = loaded_package()
            if loaded and not is_dist(loaded):
                print(f"載入的不是 dist 的包：{loaded}", file=sys.stderr)
                return 1
            return dfhack("dflang", "status").returncode
    print("等不到 DFHack 回應", file=sys.stderr)
    return 1


def down() -> int:
    kill_game()
    if os.path.isjunction(LINK):
        os.rmdir(LINK)  # 只移除連結，不動 dist/
    shutil.rmtree(SHADOW, ignore_errors=True)
    stamp = time.strftime("%Y%m%d-%H%M%S")
    misses = CONFIG_DIR / "dflang-misses.tsv"
    if misses.is_file() and misses.stat().st_size:
        MISSES_ARCHIVE.mkdir(parents=True, exist_ok=True)
        shutil.copy2(misses, MISSES_ARCHIVE / f"{stamp}.tsv")
    for f in CONFIG_DIR.glob("dflang-*.tsv"):  # dev_lua 腳本的輸出（dflang-dump.tsv、dflang-jobnames.tsv）
        if f != misses and f.stat().st_size:
            DUMPS_ARCHIVE.mkdir(parents=True, exist_ok=True)
            shutil.copy2(f, DUMPS_ARCHIVE / f"{stamp}-{f.name}")
    for f in CONFIG_DIR.glob("dflang*"):
        f.unlink()
    for f in DEV_SCRIPTS.glob("dflang-dev-*.lua"):
        f.unlink()
    if BACKUP.is_dir():
        for f in BACKUP.iterdir():
            shutil.move(f, CONFIG_DIR / f.name)
        BACKUP.rmdir()
    remove_saves()
    print("已還原：", sorted(p.name for p in MODS.iterdir()))
    return 0


def install_dev_lua() -> None:
    """scripts/dev_lua/<名稱>.lua 裝成 DFHack 腳本 dflang-dev-<名稱>（down 刪掉）"""
    DEV_SCRIPTS.mkdir(parents=True, exist_ok=True)
    for f in DEV_LUA.glob("*.lua"):
        shutil.copy2(f, DEV_SCRIPTS / f"dflang-dev-{f.name}")


def install_saves(names: list[str]) -> bool:
    """測試存檔從 temp/test-world/ 複製進遊戲的 save/，記在 SAVE_STATE 給 down 移走。玩家自己的同名存檔（region1 是遊戲的預設名稱）不覆蓋"""
    ours = set(json.loads(SAVE_STATE.read_text(encoding="utf-8"))) if SAVE_STATE.is_file() else set()
    for name in names:
        if not (TEST_WORLDS / name).is_dir():
            have = "、".join(sorted(p.name for p in TEST_WORLDS.iterdir() if p.is_dir())) if TEST_WORLDS.is_dir() else "沒有"
            print(f"沒有測試存檔 temp/test-world/{name}（有：{have}）", file=sys.stderr)
            return False
        if (SAVES / name).exists() and name not in ours:
            print(f"遊戲的 save/ 已經有 {name}（可能是玩家自己的存檔），不覆蓋", file=sys.stderr)
            return False
    for name in names:
        if not (SAVES / name).exists():
            shutil.copytree(TEST_WORLDS / name, SAVES / name)
        ours.add(name)
    SAVE_STATE.parent.mkdir(parents=True, exist_ok=True)
    SAVE_STATE.write_text(json.dumps(sorted(ours)), encoding="utf-8")
    return True


def same_tree(a: Path, b: Path) -> bool:
    def files(d: Path) -> list[Path]:
        return sorted(p.relative_to(d) for p in d.rglob("*") if p.is_file())
    names = files(a)
    return names == files(b) and all(filecmp.cmp(a / n, b / n, shallow=False) for n in names)


def remove_saves() -> None:
    """up --save 放進去的測試存檔移出遊戲的 save/：和來源相同就刪掉，被遊戲存過的搬到 temp/saves/（也許是故意存的進度）"""
    if not SAVE_STATE.is_file():
        return
    for name in json.loads(SAVE_STATE.read_text(encoding="utf-8")):
        dst = SAVES / name
        if not dst.is_dir():
            continue
        if same_tree(dst, TEST_WORLDS / name):
            shutil.rmtree(dst)
            print(f"已移走測試存檔 {name}")
        else:
            keep = SAVE_KEEP / f"{name}-{time.strftime('%Y%m%d-%H%M%S')}"
            keep.parent.mkdir(parents=True, exist_ok=True)
            shutil.move(dst, keep)
            print(f"測試存檔 {name} 被遊戲存過，搬到 {keep.relative_to(ROOT).as_posix()}")
    SAVE_STATE.unlink()


# 先移過去停兩幀（地圖依上一幀的游標位置算出指著哪一格），再按住三幀、放開後位置再留幾幀。
# 遊戲的游標只來自 SDL：主迴圈每圈在 enabler.mouse_focus（游標在視窗裡，SDL 的 ENTER／LEAVE 設）為 true 時呼叫
# SDL_GetMouseState，和存著的不同就改寫 gps 的游標欄位，否則寫 -1。Lua 寫 gps 會被下一圈蓋掉，所以用引擎的假游標
# （engine/src/mouse.rs 攔 SDL_GetMouseState），點完改回真游標、mouse_focus 放回原值。dfhack.timeout 的回呼在遊戲主執行緒的後續幀執行
MOUSE_HOLD = """local px, py, right = {px}, {py}, {right}
local g, e, engine = df.global.gps, df.global.enabler, dfhack.BASE_G.DFLANG.engine
local focus = e.mouse_focus
dfhack.BASE_G.DFLANG_HOVER = nil
engine.fake_mouse(px, py)
local function at() e.mouse_focus = true; g.precise_mouse_x, g.precise_mouse_y = px, py; g.mouse_x, g.mouse_y = px // g.tile_pixel_x, py // g.tile_pixel_y end
local function press(v) if right then e.mouse_rbut, e.mouse_rbut_down = v, v else e.mouse_lbut, e.mouse_lbut_down = v, v end end
at()
for i = 1, 8 do dfhack.timeout(i, 'frames', at) end
dfhack.timeout(2, 'frames', function() press(1) end)
dfhack.timeout(5, 'frames', function() press(0) end)
dfhack.timeout(9, 'frames', function() engine.fake_mouse(); e.mouse_focus = focus end)"""

# 假游標停著（見 MOUSE_HOLD）：每幀把 mouse_focus 設 true，直到 hover off 或下一個 mouse（換掉 DFLANG_HOVER 就停）
HOVER = """local e, engine = df.global.enabler, dfhack.BASE_G.DFLANG.engine
local ok, err = engine.fake_mouse({px}, {py})
if not ok then qerror(err) end
local id = (dfhack.BASE_G.DFLANG_HOVER or 0) + 1
dfhack.BASE_G.DFLANG_HOVER = id
local function keep() if dfhack.BASE_G.DFLANG_HOVER == id then e.mouse_focus = true; dfhack.timeout(1, 'frames', keep) end end
keep()"""
# 真游標平常不在遊戲視窗上（視窗放在最底層），mouse_focus 放回 false
HOVER_OFF = """dfhack.BASE_G.DFLANG_HOVER = nil
dfhack.BASE_G.DFLANG.engine.fake_mouse()
df.global.enabler.mouse_focus = false"""


def loaded_package() -> str:
    """引擎從哪個包載入（DFLANG.dir）；引擎沒在跑是空字串"""
    return dfhack("lua", "local s = dfhack.BASE_G.DFLANG; print(s and s.dir or '')", quiet=True).stdout.strip()


def is_dist(path: str) -> bool:
    return f"/mods/{FOLDER}/" in path


FOCUS_LUA = "print(table.concat(dfhack.gui.getCurFocus(true), '|'))"
KEY_LUA = ("local gui = require('gui'); if df.interface_key['{name}'] == nil then print('NOKEY') "
           "else gui.simulateInput(dfhack.gui.getDFViewscreen(true), '{name}') end")
SCREEN_LUA = ("local s = dfhack.BASE_G.DFLANG; " + FOCUS_LUA + "; "
              "if not (s and s.engine and s.engine.screen_text) then print('（引擎沒有 screen_text：先跑 build.py 再 up）') "
              "else local t = s.engine.screen_text(); print(t ~= '' and t or '（這一幀沒有記下的字：整句貼圖或翻譯關著）') end")


def key(names: list[str]) -> int:
    """依序送遊戲按鍵（給目前的遊戲畫面，不是 DFHack 視窗），中間留幾幀讓遊戲處理；最後印出焦點"""
    for name in names:
        if not re.fullmatch(r"[A-Z0-9_]+", name) or "NOKEY" in dfhack("lua", KEY_LUA.format(name=name), quiet=True).stdout:
            print(f"沒有這個按鍵：{name}（名稱照 df.interface_key，例 A_TRAVEL、LEAVESCREEN）", file=sys.stderr)
            return 2
        time.sleep(0.3)
    return dfhack("lua", FOCUS_LUA).returncode


def screen() -> int:
    """焦點和這一幀畫面上的字（engine.screen_text）：確認走到哪個畫面、還有沒有英文時用，不必截圖"""
    return dfhack("lua", SCREEN_LUA).returncode


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
                         (user32.WindowFromPoint, wt.HWND, [wt.POINT]), (user32.GetAncestor, wt.HWND, [wt.HWND, wt.UINT]),
                         (user32.MonitorFromWindow, wt.HMONITOR, [wt.HWND, wt.DWORD]),
                         (user32.SetWindowPos, wt.BOOL, [wt.HWND, wt.HWND, ctypes.c_int, ctypes.c_int, ctypes.c_int, ctypes.c_int, wt.UINT])):
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


def onto_screen(user32, hwnd) -> None:
    """遊戲視窗不在任何螢幕上時移回 (0,0)、放到最底層（不搶焦點、不蓋住使用者的視窗）：畫面外的視窗 PrintWindow 一直拿到
    移出去以前的那一幀（2026-10-20 拍相簿，打開人物頁後截到的還是居民清單）"""
    if user32.MonitorFromWindow(hwnd, 0):  # MONITOR_DEFAULTTONULL：和任何螢幕都不相交才回 NULL
        return
    user32.SetWindowPos(hwnd, wt.HWND(1), 0, 0, 0, 0, 0x0011)  # HWND_BOTTOM；SWP_NOSIZE | SWP_NOACTIVATE
    print("遊戲視窗在畫面外，已移回 (0,0) 放到最底層", file=sys.stderr)
    time.sleep(1.5)  # 等遊戲畫新的一幀


def capture() -> tuple[int, bytes] | None:
    """遊戲視窗客戶區的畫面（原生解析度，每像素 BGRA）：PrintWindow 加 PW_RENDERFULLCONTENT，視窗被蓋住也抓得到"""
    user32, gdi32 = win32()
    hwnd = user32.FindWindowW("SDL_app", "Dwarf Fortress")
    rect = wt.RECT()
    if not hwnd or not user32.GetClientRect(hwnd, ctypes.byref(rect)):
        return None
    onto_screen(user32, hwnd)
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


def png(img: bytes, w: int, box: tuple[int, int, int, int]) -> bytes:
    """BGRA 畫面的一塊轉成 PNG（RGB）；不用額外套件"""
    x0, y0, cw, ch = box
    raw = bytearray()
    for y in range(y0, y0 + ch):
        row = img[(y * w + x0) * 4:(y * w + x0 + cw) * 4]
        rgb = bytearray(cw * 3)
        rgb[0::3], rgb[1::3], rgb[2::3] = row[2::4], row[1::4], row[0::4]
        raw += b"\0" + rgb

    def chunk(tag: bytes, data: bytes) -> bytes:
        return struct.pack(">I", len(data)) + tag + data + struct.pack(">I", zlib.crc32(tag + data))
    return (b"\x89PNG\r\n\x1a\n" + chunk(b"IHDR", struct.pack(">IIBBBBB", cw, ch, 8, 2, 0, 0, 0))
            + chunk(b"IDAT", zlib.compress(bytes(raw), 6)) + chunk(b"IEND", b""))


def shot(path: str, crop: tuple[int, int, int, int] | None, wait: float) -> int:
    """遊戲視窗截圖存成 PNG：原解析度，crop 是畫面像素 X Y W H（看字的清晰度用）"""
    time.sleep(wait)
    got = capture()
    if not got:
        print("截不到遊戲視窗（遊戲沒開？）", file=sys.stderr)
        return 1
    w, img = got
    h = len(img) // (w * 4)
    x, y, cw, ch = crop or (0, 0, w, h)
    x, y = max(0, x), max(0, y)
    cw, ch = min(cw, w - x), min(ch, h - y)
    if cw <= 0 or ch <= 0:
        print(f"裁切範圍在畫面外（畫面 {w}×{h}）", file=sys.stderr)
        return 2
    out = Path(path)
    out.parent.mkdir(parents=True, exist_ok=True)
    out.write_bytes(png(img, w, (x, y, cw, ch)))
    print(f"{out.resolve()}  {cw}×{ch}")
    return 0


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
    if not is_dist(loaded):
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


def option(args: list[str], name: str, count: int = 1) -> list[str] | None:
    """args 裡 name 後面的 count 個值；沒有這個旗標時 None"""
    if name not in args:
        return None
    i = args.index(name)
    return args[i + 1:i + 1 + count] if len(args) > i + count else None


def main() -> int:
    args = sys.argv[1:] or [""]
    if args[0] in ("play", "up"):
        lang = (option(args, "--lang") or [None])[0]
        saves = [args[i + 1] for i, a in enumerate(args[:-1]) if a == "--save"]
        return play(lang) if args[0] == "play" else up(lang or "zh-Hant", "--with-dfi18n" in args, saves)
    if args[0] == "run":
        return dfhack(*args[1:]).returncode
    if args[0] == "click":
        x, y = int(args[1]), int(args[2])
        return dfhack("lua", f"local gui=require('gui'); df.global.gps.mouse_x={x}; df.global.gps.mouse_y={y}; "
                             "gui.simulateInput(dfhack.gui.getDFViewscreen(true), '_MOUSE_L')").returncode
    if args[0] == "mouse":
        right = "true" if args[3:4] == ["r"] else "false"
        return dfhack("lua", MOUSE_HOLD.format(px=int(args[1]), py=int(args[2]), right=right)).returncode
    if args[0] == "hover" and args[1:]:
        if args[1] == "off":
            return dfhack("lua", HOVER_OFF).returncode
        return dfhack("lua", HOVER.format(px=int(args[1]), py=int(args[2]))).returncode
    if args[0] == "key" and args[1:]:
        return key(args[1:])
    if args[0] == "shot":
        if len(args) < 2 or args[1].startswith("--"):
            print(__doc__)
            return 2
        crop = option(args, "--crop", 4)
        return shot(args[1], tuple(map(int, crop)) if crop else None, float((option(args, "--wait") or ["1"])[0]))
    if args[0] == "screen":
        return screen()
    if args[0] == "sweep":
        return sweep()
    if args[0] == "down":
        return down()
    print(__doc__)
    return 2


if __name__ == "__main__":
    sys.exit(main())
