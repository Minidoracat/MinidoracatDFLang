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
import json
import os
import shutil
import subprocess
import sys
import time
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
            loaded = dfhack("lua", "local s = dfhack.BASE_G.DFLANG; print(s and s.dir or '')", quiet=True).stdout.strip()
            if loaded and not any(f"/mods/{item['folder']}/" in loaded for item in ITEMS if item["lang"] in langs):
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


def main() -> int:
    args = sys.argv[1:]
    if not args or args[0] not in ("up", "run", "click", "mouse", "down"):
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
    return down()


if __name__ == "__main__":
    sys.exit(main())
