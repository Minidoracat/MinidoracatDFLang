# /// script
# requires-python = ">=3.11"
# dependencies = ["fonttools>=4.50"]
# ///
"""發版前驗證閘門（repo 根目錄：uv run scripts/verify_mod.py），任一 FAIL 即 exit 1。

1. data/strings/*.tsv（整句）、data/rules/*.tsv（句型）與 data/screens/*.tsv（依畫面套用的整句，檔名是畫面）：
   欄位 en, zh-Hant, zh-Hans, align, note；原文能用 CP437 表示（遊戲只會畫 CP437），同一類不重複
   （screens 是同一個畫面檔裡不重複）；align 只能是空、c、r；screens 的檔名只用英數、底線、點
2. 句型佔位：原文至少一個 {1}～{9}、每個只出現一次、不能相鄰；樣板要有英文字、限定佔位（{#n}、{@n}）或頭尾固定字，
   否則會套到幾乎所有字串；兩種譯文用的佔位和原文一致、各一次
3. 繁簡同步：同一條兩欄同時有譯文或同時留空（繁簡分開翻，但兩包要涵蓋同一批句子）
4. 開頭空白：譯文開頭的空白數與原文相同（那是遊戲的縮排）
5. 寬度：譯文格數不超過原文（CP437 字一格，其他字兩格），超出會蓋到旁邊的介面；句型只算樣板本身，
   佔位填入後超寬的情況由引擎在執行時退回英文。名稱檔（strings/names_*.tsv）至少可用 6 格：
   短英文名（ogre、hydra、kiwi）的通行中文名多半是三個字
6. 文字檔 UTF-8、無 BOM、LF（遊戲端逐位元組讀，BOM 與 \\r 會黏進原文或譯文）
7. 字型：每種語言譯文與吉祥物名牌（語言標籤、作者名）用到的字，主字型或補字字型（temp/fonts/）至少一份有
8. 吉祥物：art/mascot.png 是 8-bit RGBA PNG（引擎只讀這種）；每包的 mascot_label 是全形字
9. scripts/workshop.json：繁、簡各一項；ID、名稱、說明、作者只用 ASCII（DF 以 CP437 讀 info.txt）
10. engine/patterns.tsv 有引擎要 hook 的六個函式（四個畫字函式、段落排版、錨定貼圖）
11. dist/ 已建置時：每包檔案齊全、沒有雜檔，且字典、句型與 data/ 一致（沒建置則略過）
"""
import re
import sys
from pathlib import Path

from fontTools.ttLib import TTFont

from build import DIST, FONTS, KINDS, dict_lines, load_config, read_rows
from cp437 import BYTE_OF, cells, encodable

ROOT = Path(__file__).resolve().parents[1]
HEADER = ["en", "zh-Hant", "zh-Hans", "align", "note"]
HOOKS = ("addst", "addst_flag", "top_addst", "addcoloredst", "mtb_set_width", "add_anchored_tile", "wrap_text")
PACKAGE_SUFFIXES = {".txt", ".tsv", ".lua", ".dll", ".otf", ".md", ".png"}
PLACEHOLDER = re.compile(r"\{[#@]?([1-9])\}")  # {1}～{9}；{#1} 只收數字、{@1} 只收字典裡的字詞
MARKUP = re.compile(r"\[(?:[RBP]|C:\d+:\d+:\d+)\]")  # 段落的換行與換色標記

results: list[tuple[bool, str]] = []


def check(ok: bool, msg: str, problems=()) -> None:
    problems = list(problems)
    results.append((ok and not problems, msg + (f" {problems[:8]}" if problems else "")))


def leading_spaces(s: str) -> int:
    return len(s) - len(s.lstrip(" "))


def template(s: str) -> str:
    """句型去掉佔位後的樣板本身。"""
    return PLACEHOLDER.sub("", s)


def placeholder_problem(r: dict, langs: list[str]) -> str | None:
    en = PLACEHOLDER.findall(r["en"])
    if not en:
        return "原文沒有佔位"
    if len(set(en)) != len(en):
        return "原文佔位重複"
    if re.search(r"\{[#@]?[1-9]\}\{[#@]?[1-9]\}", r["en"]):
        return "原文佔位相鄰"
    # 只有空白、標點夾著佔位的樣板（{1} {2}、{1}, {2}）會套到幾乎所有字串。有英文字、有限定佔位
    # （{#n} 只收數字、{@n} 只收字典字詞），或開頭、結尾是固定字（外框 -{1}-、後綴 {1}, ♂）時，套得到的字串有限
    lits = PLACEHOLDER.split(r["en"])  # 佔位前後的固定字；只有空白不算（` {1} {2}` 一樣會套到幾乎所有字串）
    edges = bool(lits[0].strip() or lits[-1].strip())
    if not re.search(r"[A-Za-z]", template(r["en"])) and not re.search(r"\{[#@][1-9]\}", r["en"]) and not edges:
        return "原文樣板沒有英文字、限定佔位或頭尾固定字，會套到幾乎所有字串"
    for lang in langs:
        if r[lang] and sorted(PLACEHOLDER.findall(r[lang])) != sorted(en):
            return f"{lang} 佔位和原文不一致"
    return None


def main() -> int:
    config = load_config()
    langs = [item["lang"] for item in config["items"]]
    data: dict[str, list[dict]] = {}
    for kind in KINDS:
        files = sorted((ROOT / "data" / kind).glob("*.tsv"))
        headers = [f"{kind}/{f.name}" for f in files if f.read_text(encoding="utf-8").split("\n", 1)[0].split("\t") != HEADER]
        rows = [r for r in read_rows(kind) if f"{kind}/{r['file']}" not in headers]  # 欄名錯的檔案（含 BOM）其餘檢查無從判斷
        bad_cols = [f"{kind}/{r['file']}: {r.get('en')!r}" for r in rows if None in r.values() or None in r]
        check(not headers and not bad_cols, f"{kind} 欄位（{len(files)} 檔、{len(rows)} 條）", headers + bad_cols)
        data[kind] = [{**r, "file": f"{kind}/{r['file']}"} for r in rows if None not in r.values() and None not in r]
    rows = [r for kind in KINDS for r in data[kind]]
    bad_screens = [r for r in sorted({r["file"] for r in data["screens"]}) if not re.fullmatch(r"screens/[A-Za-z_][\w.]*\.tsv", r)]
    check(not bad_screens, "data/screens 檔名是畫面（英數、底線，. 代表 /）", bad_screens)

    seen, dupes, bad_src, bad_align = set(), [], [], []
    for r in rows:
        kind = r["file"].split("/")[0]
        key = (r["file"] if kind == "screens" else kind, r["en"])
        if key in seen:
            dupes.append(f"{r['file']}: {r['en']!r}")
        seen.add(key)
        if not r["en"] or not encodable(r["en"]):
            bad_src.append(f"{r['file']}: {r['en']!r}")
        if r["align"] not in ("", "c", "r"):
            bad_align.append(f"{r['file']}: {r['en']!r} align={r['align']!r}")
    check(not dupes, "原文不重複", dupes)
    check(not bad_src, "原文能用 CP437 表示", bad_src)
    check(not bad_align, "align 只能是空、c、r", bad_align)
    bad_ph = [f"{r['file']}: {r['en']!r} {p}" for r in data["rules"] if (p := placeholder_problem(r, langs))]
    check(not bad_ph, f"句型佔位寫法（{len(data['rules'])} 條）", bad_ph)

    unpaired = [f"{r['file']}: {r['en']!r}" for r in rows if bool(r["zh-Hant"].strip()) != bool(r["zh-Hans"].strip())]
    check(not unpaired, "繁簡兩欄同時有譯文或同時留空", unpaired)
    indent = [f"{r['file']}: {r['en']!r} [{lang}]" for r in rows for lang in langs
              if r[lang] and leading_spaces(r[lang]) != leading_spaces(r["en"])]
    check(not indent, "譯文開頭空白與原文相同", indent)
    def room(r: dict) -> int:
        en = cells(template(r["en"]))
        return max(en, 6) if r["file"].startswith("strings/names_") else en

    wide = [f"{r['file']}: {r['en']!r} [{lang}] {cells(template(r[lang]))}>{room(r)}"
            for r in rows for lang in langs if r[lang] and cells(template(r[lang])) > room(r)]
    check(not wide, "譯文格數不超過原文", wide)
    marks = [f"{r['file']}: {r['en'][:60]!r} [{lang}]" for r in rows for lang in langs
             if r[lang] and sorted(MARKUP.findall(r[lang])) != sorted(MARKUP.findall(r["en"]))]
    check(not marks, "段落標記（[R]、[B]、[P]、[C:…]）與原文一致", marks)

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
        used |= set(item.get("mascot_label", "") + config["author"]) - {" "}  # 名牌由引擎用字型畫，英數也要有
        missing = sorted(c for c in used if not any(ord(c) in cmap for cmap in cmaps))
        check(not missing, f"{item['lang']} 字型涵蓋 {len(used)} 個字（含吉祥物名牌）", missing)

    mascot = ROOT / "art/mascot.png"
    with mascot.open("rb") as fh:  # PNG IHDR：寬、高各 4 bytes，再來是 bit depth 與 color type
        head = fh.read(26)
    rgba8 = head[:8] == b"\x89PNG\r\n\x1a\n" and head[24] == 8 and head[25] == 6
    labels = [f"{item['lang']}: {item.get('mascot_label')!r}" for item in config["items"]
              if not item.get("mascot_label") or any(c in BYTE_OF for c in item["mascot_label"])]
    check(rgba8 and not labels, "art/mascot.png 是 8-bit RGBA PNG；每包都有全形字的 mascot_label", labels)

    meta = [f"{item['lang']}.{k}" for item in config["items"] for k in ("id", "name", "description")
            if not item[k].isascii()] + ([] if config["author"].isascii() and config["author"] else ["author"])
    check(sorted(langs) == ["zh-Hans", "zh-Hant"] and not meta, "workshop.json 繁簡各一項、info.txt 欄位為 ASCII", meta)

    patterns = {line.split("\t")[0] for line in (ROOT / "engine/patterns.tsv").read_text(encoding="utf-8").splitlines()
                if line and not line.startswith("#")}
    check(all(h in patterns for h in HOOKS), f"engine/patterns.tsv 含 {len(HOOKS)} 個 hook 函式", [h for h in HOOKS if h not in patterns])

    for item in config["items"]:
        out = DIST / item["folder"]
        if not out.is_dir():
            results.append((True, f"{item['lang']} dist 未建置，略過"))
            continue
        suffix = item["lang"].lower().replace("-", "_")
        required = ["info.txt", f"scripts_modinstalled/dflang_{suffix}.lua", "scripts_modinstalled/dflang.lua",
                    "dflang/dflang.dll", *(f"dflang/{kind}.tsv" for kind in KINDS), "dflang/glyphs.txt", "dflang/patterns.tsv",
                    "dflang/LICENSE.txt", "dflang/LICENSE-fonts.txt", "dflang/CREDITS.md", "dflang/mascot.png"]
        lacking = [p for p in required if not (out / p).is_file()] + ([] if list((out / "dflang/fonts").glob("*.otf")) else ["dflang/fonts/*.otf"])
        stray = [p.relative_to(out).as_posix() for p in out.rglob("*") if p.is_file() and p.suffix not in PACKAGE_SUFFIXES]
        stale = []
        for kind in KINDS:
            built = out / f"dflang/{kind}.tsv"
            expected = dict_lines(kind, data[kind], item["lang"])
            if built.is_file() and built.read_text(encoding="utf-8") != expected:
                stale.append(f"dflang/{kind}.tsv 過期，重跑 build.py")
        check(not lacking and not stray and not stale, f"{item['lang']} dist 檔案齊全、無雜檔、字典與句型和 data/ 一致",
              lacking + stray + stale)

    for ok, msg in results:
        print(("PASS " if ok else "FAIL ") + msg)
    failed = sum(not ok for ok, _ in results)
    print(f"{len(results) - failed}/{len(results)} PASS")
    return 1 if failed else 0


if __name__ == "__main__":
    sys.exit(main())
