# /// script
# requires-python = ">=3.11"
# ///
"""查遊戲與 DFHack 有沒有比 scripts/upstream.json 記的更新的版本；有就各開一個 GitHub Issue。

三個來源各自比對：官網免費版的版本（bay12games.com 的下載檔名）、Steam 版 public 分支的 buildid、
DFHack 的最新 release。官網版有新版時下載它的 exe 跑 make_patterns.py --check，結果寫進 Issue
（免費版和 Steam 版不是同一個 exe，結果只當參考）。適配完把 upstream.json 改成新版本。

用法（repo 根目錄）：
  uv run scripts/watch_upstream.py          # 只印出比對結果與 Issue 內容
  uv run scripts/watch_upstream.py --open   # 開 Issue（同標題的 Issue 已存在就不開；要 gh 與 GH_TOKEN）
"""
import io
import json
import os
import re
import subprocess
import sys
import tempfile
import urllib.request
import zipfile
from pathlib import Path

ROOT = Path(__file__).resolve().parents[1]
BAY12 = "https://www.bay12games.com/dwarves/"
STEAM = "https://api.steamcmd.net/v1/info/975370"
DFHACK = "https://api.github.com/repos/DFHack/dfhack/releases/latest"
LABEL = "upstream"


def fetch(url: str) -> bytes:
    req = urllib.request.Request(url, headers={"User-Agent": "MinidoracatDFLang-watch"})
    with urllib.request.urlopen(req, timeout=120) as r:
        return r.read()


def latest() -> dict[str, str]:
    versions = re.findall(r"df_(\d+)_(\d+)_win\.zip", fetch(BAY12).decode("utf-8", "replace"))
    if not versions:
        sys.exit(f"{BAY12} 找不到 df_<版本>_win.zip")
    major, minor = max(versions, key=lambda v: (int(v[0]), int(v[1])))
    steam = json.loads(fetch(STEAM))["data"]["975370"]["depots"]["branches"]["public"]["buildid"]
    return {"df": f"{major}.{minor}", "steam_build": str(steam), "dfhack": json.loads(fetch(DFHACK))["tag_name"]}


def check_patterns(version: str) -> str:
    """下載官網免費版，回傳 make_patterns.py --check 的輸出。"""
    url = f"{BAY12}df_{version.replace('.', '_')}_win.zip"
    with tempfile.TemporaryDirectory() as tmp:
        with zipfile.ZipFile(io.BytesIO(fetch(url))) as z:
            z.extract("Dwarf Fortress.exe", tmp)
        r = subprocess.run(["uv", "run", "scripts/make_patterns.py", "--check"], cwd=ROOT, capture_output=True,
                           text=True, encoding="utf-8", env={**os.environ, "DF_PATH": tmp})
    return (r.stdout + r.stderr).strip() or f"make_patterns.py 沒有輸出（結束碼 {r.returncode}）"


def issues(known: dict[str, str], now: dict[str, str]) -> list[tuple[str, str]]:
    out = []
    if now["df"] != known["df"]:
        result = check_patterns(now["df"])
        verdict = ("特徵碼全部命中：引擎大概還能用，要補的是新文字。" if result.endswith("patterns OK")
                   else "**有特徵碼沒命中：玩家更新遊戲後中文化可能失效，要優先處理。**")
        out.append((f"遊戲更新：Dwarf Fortress {now['df']}",
                    f"官網免費版從 {known['df']} 更新到 {now['df']}（{BAY12}）。\n\n"
                    f"用免費版的執行檔驗引擎的特徵碼（Steam 版是另一個執行檔，還要另外驗）：\n\n"
                    f"```\n{result}\n```\n\n{verdict}\n\n"
                    f"適配步驟見 `docs/analysis.md`「遊戲更新」；做完把 `scripts/upstream.json` 改成新版本。"))
    if now["steam_build"] != known["steam_build"]:
        out.append((f"遊戲更新：Steam build {now['steam_build']}",
                    f"Steam 版 public 分支的 build 從 {known['steam_build']} 變成 {now['steam_build']}"
                    f"（官網版本目前是 {now['df']}）。\n\n版本號沒變時通常是小修正，"
                    f"仍要用 Steam 版的執行檔跑 `make_patterns.py --check`；確認後更新 `scripts/upstream.json`。"))
    if now["dfhack"] != known["dfhack"]:
        out.append((f"DFHack 更新：{now['dfhack']}",
                    f"DFHack 從 {known['dfhack']} 更新到 {now['dfhack']}。\n\n"
                    f"- 發布說明：https://github.com/DFHack/dfhack/releases/tag/{now['dfhack']}\n"
                    f"- 變更比較：https://github.com/DFHack/dfhack/compare/{known['dfhack']}...{now['dfhack']}\n\n"
                    f"要查新增或改動的畫面文字；做完把 `scripts/upstream.json` 改成新版本。"))
    return out


def gh(*args: str) -> str:
    return subprocess.run(["gh", *args], cwd=ROOT, check=True, capture_output=True, text=True, encoding="utf-8").stdout


def main() -> int:
    known = json.loads((ROOT / "scripts/upstream.json").read_text(encoding="utf-8"))
    now = latest()
    print(f"已適配 {known}\n目前   {now}")
    todo = issues(known, now)
    if not todo:
        print("沒有新版本")
    existing = ({i["title"] for i in json.loads(gh("issue", "list", "--state", "all", "--label", LABEL,
                                                    "--limit", "500", "--json", "title"))}
                if "--open" in sys.argv and todo else set())
    for title, body in todo:
        if "--open" not in sys.argv:
            print(f"\n=== {title} ===\n{body}")
        elif title in existing:
            print(f"已有 Issue：{title}")
        else:
            print(gh("issue", "create", "--title", title, "--body", body, "--label", LABEL).strip())
    return 0


if __name__ == "__main__":
    sys.exit(main())
