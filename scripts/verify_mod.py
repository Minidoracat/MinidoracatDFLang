# /// script
# requires-python = ">=3.11"
# dependencies = ["fonttools>=4.50"]
# ///
"""發版前驗證閘門（repo 根目錄：uv run scripts/verify_mod.py），任一 FAIL 即 exit 1。

1. data/strings/*.tsv：欄位 en, zh-Hant, zh-Hans, align, note；原文能用 CP437 表示（遊戲只會畫 CP437），全庫不重複；align 只能是空、c、r
2. 繁簡同步：同一條兩欄同時有譯文或同時留空（繁簡分開翻，但兩包要涵蓋同一批句子）
3. 開頭空白：譯文開頭的空白數與原文相同（那是遊戲的縮排）
4. 寬度：譯文格數不超過原文（CP437 字一格，其他字兩格），超出會蓋到旁邊的介面
5. 文字檔 UTF-8、無 BOM、LF（遊戲端逐位元組讀，BOM 與 \\r 會黏進原文或譯文）
6. 字型：每種語言譯文用到的非 CP437 字，主字型或補字字型（temp/fonts/）至少一份有
7. scripts/workshop.json：繁、簡各一項；ID、名稱、說明只用 ASCII（DF 以 CP437 讀 info.txt）
8. engine/patterns.tsv 有引擎要 hook 的四個函式
9. dist/ 已建置時：每包檔案齊全、沒有雜檔，且字典與 data/ 一致（沒建置則略過）
"""
import sys
from pathlib import Path

from fontTools.ttLib import TTFont

from build import DIST, FONTS, load_config, read_strings
from cp437 import BYTE_OF, cells, encodable

ROOT = Path(__file__).resolve().parents[1]
HEADER = ["en", "zh-Hant", "zh-Hans", "align", "note"]
HOOKS = ("addst", "addst_flag", "top_addst", "addcoloredst")
PACKAGE_SUFFIXES = {".txt", ".tsv", ".lua", ".dll", ".otf", ".md"}

results: list[tuple[bool, str]] = []


def check(ok: bool, msg: str, problems=()) -> None:
    problems = list(problems)
    results.append((ok and not problems, msg + (f" {problems[:8]}" if problems else "")))


def leading_spaces(s: str) -> int:
    return len(s) - len(s.lstrip(" "))


def main() -> int:
    config = load_config()
    langs = [item["lang"] for item in config["items"]]
    files = sorted((ROOT / "data/strings").glob("*.tsv"))
    rows = read_strings()

    headers = [f.name for f in files if f.read_text(encoding="utf-8").split("\n", 1)[0].split("\t") != HEADER]
    rows = [r for r in rows if r["file"] not in headers]  # 欄名錯的檔案（含 BOM）其餘檢查無從判斷
    seen, dupes, bad_src, bad_align, bad_cols = set(), [], [], [], []
    for r in rows:
        if None in r.values() or None in r:
            bad_cols.append(f"{r['file']}: {r.get('en')!r}")
            continue
        if r["en"] in seen:
            dupes.append(f"{r['file']}: {r['en']!r}")
        seen.add(r["en"])
        if not r["en"] or not encodable(r["en"]):
            bad_src.append(f"{r['file']}: {r['en']!r}")
        if r["align"] not in ("", "c", "r"):
            bad_align.append(f"{r['file']}: {r['en']!r} align={r['align']!r}")
    check(not headers and not bad_cols, f"字串檔欄位（{len(files)} 檔、{len(rows)} 條）", headers + bad_cols)
    check(not dupes, "原文不重複", dupes)
    check(not bad_src, "原文能用 CP437 表示", bad_src)
    check(not bad_align, "align 只能是空、c、r", bad_align)

    rows = [r for r in rows if None not in r.values()]
    unpaired = [f"{r['file']}: {r['en']!r}" for r in rows if bool(r["zh-Hant"].strip()) != bool(r["zh-Hans"].strip())]
    check(not unpaired, "繁簡兩欄同時有譯文或同時留空", unpaired)
    indent = [f"{r['file']}: {r['en']!r} [{lang}]" for r in rows for lang in langs
              if r[lang] and leading_spaces(r[lang]) != leading_spaces(r["en"])]
    check(not indent, "譯文開頭空白與原文相同", indent)
    wide = [f"{r['file']}: {r['en']!r} [{lang}] {cells(r[lang])}>{cells(r['en'])}" for r in rows for lang in langs
            if r[lang] and cells(r[lang]) > cells(r["en"])]
    check(not wide, "譯文格數不超過原文", wide)

    text_files = [*(ROOT / "data").rglob("*.tsv"), *(ROOT / "glossary").glob("*.tsv"), *(ROOT / "lua").glob("*.lua"),
                  ROOT / "scripts/workshop.json", ROOT / "engine/patterns.tsv"]
    bad_enc = []
    for f in text_files:
        raw = f.read_bytes()
        try:
            raw.decode("utf-8")
        except UnicodeDecodeError:
            bad_enc.append(f"{f.name}: not UTF-8")
        if raw.startswith(b"\xef\xbb\xbf") or b"\r" in raw:
            bad_enc.append(f"{f.name}: BOM 或 CR")
    check(not bad_enc, "文字檔 UTF-8、無 BOM、LF", bad_enc)

    for item in config["items"]:
        fonts = [FONTS / f for f in [item["font"], *item["fallback_fonts"]]]
        if not all(f.is_file() for f in fonts):
            check(False, f"{item['lang']} 字型：temp/fonts/ 缺字型，先跑 uv run scripts/build.py --fetch-fonts")
            continue
        cmaps = [TTFont(f).getBestCmap() for f in fonts]
        used = {c for r in rows for c in r[item["lang"]] if c not in BYTE_OF}
        missing = sorted(c for c in used if not any(ord(c) in cmap for cmap in cmaps))
        check(not missing, f"{item['lang']} 字型涵蓋 {len(used)} 個字", missing)

    meta = [f"{item['lang']}.{k}" for item in config["items"] for k in ("id", "name", "description")
            if not item[k].isascii()]
    check(sorted(langs) == ["zh-Hans", "zh-Hant"] and not meta, "workshop.json 繁簡各一項、info.txt 欄位為 ASCII", meta)

    patterns = {line.split("\t")[0] for line in (ROOT / "engine/patterns.tsv").read_text(encoding="utf-8").splitlines()
                if line and not line.startswith("#")}
    check(all(h in patterns for h in HOOKS), "engine/patterns.tsv 含四個 hook 函式", [h for h in HOOKS if h not in patterns])

    for item in config["items"]:
        out = DIST / item["folder"]
        if not out.is_dir():
            results.append((True, f"{item['lang']} dist 未建置，略過"))
            continue
        suffix = item["lang"].lower().replace("-", "_")
        required = ["info.txt", f"scripts_modinstalled/dflang_{suffix}.lua", "scripts_modinstalled/dflang.lua",
                    "dflang/dflang.dll", "dflang/strings.tsv", "dflang/glyphs.txt", "dflang/patterns.tsv",
                    "dflang/LICENSE.txt", "dflang/LICENSE-fonts.txt", "dflang/CREDITS.md"]
        lacking = [p for p in required if not (out / p).is_file()] + ([] if list((out / "dflang/fonts").glob("*.otf")) else ["dflang/fonts/*.otf"])
        stray = [p.relative_to(out).as_posix() for p in out.rglob("*") if p.is_file() and p.suffix not in PACKAGE_SUFFIXES]
        expected = "".join(f"{r['en']}\t{r[item['lang']]}\t{r['align']}\n" for r in rows if r[item["lang"]].strip())
        stale = (out / "dflang/strings.tsv").is_file() and (out / "dflang/strings.tsv").read_text(encoding="utf-8") != expected
        check(not lacking and not stray and not stale,
              f"{item['lang']} dist 檔案齊全、無雜檔、字典與 data/ 一致" + ("（字典過期，重跑 build.py）" if stale else ""),
              lacking + stray)

    for ok, msg in results:
        print(("PASS " if ok else "FAIL ") + msg)
    failed = sum(not ok for ok, _ in results)
    print(f"{len(results) - failed}/{len(results)} PASS")
    return 1 if failed else 0


if __name__ == "__main__":
    sys.exit(main())
