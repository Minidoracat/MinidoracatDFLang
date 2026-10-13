# /// script
# requires-python = ">=3.11"
# ///
"""翻譯查詢：一句英文原文在本專案、術語庫、參考資料與遊戲字串裡的狀況。

用法（repo 根目錄）：
  uv run scripts/lookup.py "Create new world"
  uv run scripts/lookup.py --refresh     從已訂閱的工作坊資料更新 translation-reference/（gitignored）

參考資料是矮人要塞中文維基翻譯組的簡體中文資料（CC BY-NC 4.0），只供查詢術語與理解語境；
譯文逐句自己寫，採用其譯名的術語在 glossary/terms.tsv 的 ref 欄標 dfzh-wiki。
"""
import csv
import re
import shutil
import sys
import tomllib
from pathlib import Path

ROOT = Path(__file__).resolve().parents[1]
REFERENCE = ROOT / "translation-reference"
SNAPSHOTS = ROOT.parent / "df-decompiled-reference/snapshots"
# 工作坊項目 → 參考資料子目錄
REFERENCE_SOURCES = {
    "3635900931/dfi18n-data": "dfzh-wiki-zh-hans",   # DFI18n Data - 简体中文（維基翻譯組）
    "3613958631/local-data": "dfi18n-local-zh-hans",  # DFI18n 本體附帶的簡中補充資料
}


def read_tsv(path: Path) -> list[dict]:
    with path.open(encoding="utf-8", newline="") as fh:
        return list(csv.DictReader(fh, delimiter="\t", quoting=csv.QUOTE_NONE))


def refresh() -> int:
    from dfpaths import WORKSHOP
    copied = 0
    for src, name in REFERENCE_SOURCES.items():
        if (WORKSHOP / src).is_dir():
            shutil.rmtree(REFERENCE / name, ignore_errors=True)
            shutil.copytree(WORKSHOP / src, REFERENCE / name)
            copied += 1
            print(f"{WORKSHOP / src} -> translation-reference/{name}")
    if not copied:
        print("找不到已訂閱的工作坊資料；可改用 git clone https://github.com/DFI18n/dfi18n-data-zh-hans translation-reference/dfzh-wiki-zh-hans")
        return 1
    return 0


def reference_pairs() -> list[tuple[str, str, str]]:
    """(來源檔, 原文或規則, 譯文)：simple CSV 的整句，加上規則集裡不含 {佔位} 的固定詞。"""
    pairs = []
    for f in sorted(REFERENCE.rglob("*.csv")):
        with f.open(encoding="utf-8-sig", newline="") as fh:
            pairs += [(f.relative_to(REFERENCE).as_posix(), r["text"], r["translation"])
                      for r in csv.DictReader(fh) if r.get("text")]
    for f in sorted(REFERENCE.rglob("*.toml")):
        try:
            data = tomllib.loads(f.read_text(encoding="utf-8"))
        except tomllib.TOMLDecodeError:
            continue
        for ruleset in data.get("rulesets", []):
            for key, value in (ruleset.get("rules") or {}).items():
                pairs.append((f.relative_to(REFERENCE).as_posix(), key, value))
    return pairs


def game_strings(text: str) -> list[str]:
    snaps = sorted(p for p in SNAPSHOTS.glob("*") if (p / "exe/strings.tsv").is_file()) if SNAPSHOTS.is_dir() else []
    if not snaps:
        return ["（沒有反編譯快照）"]
    found = []
    for line in (snaps[-1] / "exe/strings.tsv").read_text(encoding="utf-8").splitlines():
        if line.endswith("\t" + text):
            found.append(f"exe {line.split(chr(9))[0]}")
    for line in (snaps[-1] / "raws_text.tsv").read_text(encoding="utf-8").splitlines():
        if text in line.split("\t")[-1].split(":"):
            found.append("raws " + line.split("\t")[0])
            break
    return found or ["（exe 字串與 raws 都沒有完全相同的一段；可能是組字結果）"]


def lookup(text: str) -> int:
    low = text.lower()
    rows = [{**r, "file": p.name} for p in sorted((ROOT / "data/strings").glob("*.tsv")) for r in read_tsv(p)]
    print("== 本專案")
    exact = [r for r in rows if r["en"] == text]
    near = [r for r in rows if low in r["en"].lower() and r not in exact][:10]
    for r in exact + near:
        print(f"{'=' if r in exact else '~'} {r['file']}  {r['en']!r}\n    繁 {r['zh-Hant']}\n    簡 {r['zh-Hans']}")
    if not exact + near:
        print("（沒有）")

    print("== 術語")
    terms = [t for t in read_tsv(ROOT / "glossary/terms.tsv")
             if re.search(rf"(?<![A-Za-z]){re.escape(t['en'])}(?:e?s)?(?![A-Za-z])", text, re.I)]
    for t in terms:
        print(f"{t['en']}：繁 {t['zh-Hant']}／簡 {t['zh-Hans']}（{t['status']}{'，參考 ' + t['ref'] if t['ref'] else ''}）{t['note']}")
    if not terms:
        print("（沒有）")

    print("== 參考（維基翻譯組簡中，CC BY-NC 4.0，只供參考）")
    if not REFERENCE.is_dir():
        print("（沒有 translation-reference/；先跑 --refresh）")
    else:
        pairs = reference_pairs()
        hits = [p for p in pairs if p[1] == text] + [p for p in pairs if p[1] != text and low in p[1].lower()][:10]
        for src, key, value in hits:
            print(f"{'=' if key == text else '~'} {src}  {key!r} → {value}")
        if not hits:
            print("（沒有）")

    print("== 遊戲字串（最新快照）")
    for line in game_strings(text):
        print(line)
    return 0


def main() -> int:
    if sys.argv[1:] == ["--refresh"]:
        return refresh()
    if len(sys.argv) == 2:
        return lookup(sys.argv[1])
    print(__doc__)
    return 2


if __name__ == "__main__":
    sys.exit(main())
