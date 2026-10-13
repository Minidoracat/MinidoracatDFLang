# /// script
# requires-python = ">=3.11"
# ///
"""本機實機測試：把 dist/ 的工作坊包掛進遊戲、啟動、下 DFHack 指令、收尾還原。

用法（repo 根目錄，先跑 scripts/build.py）：
  uv run scripts/dev_game.py up [--lang zh-Hant|zh-Hans|both]   掛包、遮蔽 DFI18n、啟動，等到 DFHack 回應
  uv run scripts/dev_game.py run dflang status                 轉給 dfhack-run
  uv run scripts/dev_game.py click X Y                         以文字格座標模擬滑鼠左鍵（不動真滑鼠、不搶焦點）
  uv run scripts/dev_game.py down                              關遊戲、拆掛載、移除遮蔽與 dflang 設定檔

遮蔽 DFI18n：在 mods/ 放一個 ID 也是 dfi18n、版本 999、沒有腳本的空 MOD。DFHack 每個 ID 只載入最高版本，
真的 DFI18n 腳本就不會執行；down 會把它刪掉。掛載位置是 %APPDATA%/Bay 12 Games/Dwarf Fortress/mods
（非 Portable Mode 的預設）。
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


def up(langs: list[str]) -> int:
    kill_game()
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
    os.startfile("steam://rungameid/975370")
    deadline = time.time() + 180
    while time.time() < deadline:
        time.sleep(3)
        if "ready" in dfhack("lua", "print('ready')", quiet=True).stdout:
            time.sleep(5)  # 等標題畫面畫出第一幀
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
    for f in (DF / "dfhack-config").glob("dflang*"):
        f.unlink()
    print("已還原：", sorted(p.name for p in MODS.iterdir()))
    return 0


def main() -> int:
    args = sys.argv[1:]
    if not args or args[0] not in ("up", "run", "click", "down"):
        print(__doc__)
        return 2
    if args[0] == "up":
        lang = args[2] if len(args) > 2 and args[1] == "--lang" else "zh-Hant"
        return up(["zh-Hant", "zh-Hans"] if lang == "both" else [lang])
    if args[0] == "run":
        return dfhack(*args[1:]).returncode
    if args[0] == "click":
        x, y = int(args[1]), int(args[2])
        return dfhack("lua", f"local gui=require('gui'); df.global.gps.mouse_x={x}; df.global.gps.mouse_y={y}; "
                             "gui.simulateInput(dfhack.gui.getDFViewscreen(true), '_MOUSE_L')").returncode
    return down()


if __name__ == "__main__":
    sys.exit(main())
