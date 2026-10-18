# /// script
# requires-python = ">=3.11"
# dependencies = ["fonttools>=4.50"]
# ///
"""發版前驗證閘門（repo 根目錄：uv run scripts/verify_mod.py），任一 FAIL 即 exit 1。

1. data/strings/*.tsv（整句）、data/rules/*.tsv（句型）與 data/screens/*.tsv（依畫面套用的整句，檔名是畫面）：
   欄位 en, zh-Hant, zh-Hans, align, note（screens 在 align 後多一欄 row：限定第幾列，空白或非負整數）；
   原文能用 CP437 表示（遊戲只會畫 CP437），同一類不重複（screens 是同一個畫面檔裡不重複）；align 只能是空、c、r（句型與字串另可寫 inner：只用在佔位裡；
   tail：事件句尾的原因片語，引擎去掉再翻；字串另可寫 title：書名用詞，只給書名句型的 {$n} 用，自成一類；key：按鍵名，按鍵名選項開著才用；
   screens 另可寫 music：配樂曲名，選項關著時照原文）；data/rules/_events.tsv、zz_events.tsv 與 data/events 一致（由 scripts/gen_events.py 產生）；
   screens 的檔名只用英數、底線、點
2. 句型佔位：原文至少一個 {1}～{9}、每個只出現一次、不能相鄰；樣板要有英文字、限定佔位（{#n}、{@n}、{%n}，書名的 {$n}、{&n}、{=n}）或頭尾固定字，
   否則會套到幾乎所有字串；兩種譯文用的佔位都是原文有的、各最多一次，除了 {@n} 都要用到
3. 繁簡同步：同一條兩欄同時有譯文或同時留空（繁簡分開翻，但包裡兩種語言要涵蓋同一批句子）
4. 開頭空白：譯文開頭的空白數與原文相同（那是遊戲的縮排）；只套在佔位裡的 inner 句型與書名用詞（title）不算（不會單獨畫出）；
   置中（align c）的不算：引擎會重新置中，中文字兩格寬、原文格數是單數時多半格只能偏一邊，可以用開頭空白往右挪一格
5. 寬度：譯文格數不超過原文（CP437 字一格，其他字兩格），超出會蓋到旁邊的介面；句型只算樣板本身，
   佔位填入後超寬的情況由引擎在執行時退回英文；align 寫 inner、title 的只接進佔位、不單獨畫，不限；key 的按鍵名由引擎照字型實際字寬比，
   放不下照英文，也不限。名稱檔（strings/names_*.tsv）至少可用 6 格：
   短英文名（ogre、hydra、kiwi）的通行中文名多半是三個字。sentences_*、ann_*、speech_*、combat_*、desc_*、health_*、事件句（_events、zz_events、events_*）不限：
   人物頁、訪客說明、公告、說話句、戰報、外觀與健康、傳說模式的段落由引擎依框寬重新換行
6. 文字檔 UTF-8、無 BOM、LF（遊戲端逐位元組讀，BOM 與 \\r 會黏進原文或譯文）
7. 字型：workshop.json 每種語言的 fonts 正好有 bold、regular 兩種字重；每種字重都要能畫該語言譯文、名字音譯表與吉祥物名牌（語言標籤、作者名）用到的字
   （該字重的字型清單 temp/fonts/ 至少一份有）
8. 吉祥物：art/mascot.png 是 8-bit RGBA PNG（引擎只讀這種）；每種語言的 mascot_label 是全形字
9. scripts/workshop.json：languages 正好是繁、簡兩種；ID、名稱、說明、作者只用 ASCII（DF 以 CP437 讀 info.txt）；
   數字版本號（version.numeric）是 9 開頭的四位數：DFHack 挑同一個 MOD 的哪一版執行時把它當字串比（'10' < '9'），
   遊戲留在 mods/ 的舊版副本（0.5.1 以前是個位數）位數不同就可能贏過新版，玩家更新後照樣跑舊版
10. engine/patterns.tsv 有引擎要 hook 的函式（四個畫字函式、段落排版、錨定貼圖、自動換行、彩色文字框、世界生成名稱與原名）
11. dist/ 已建置時：包裡檔案齊全（共用檔、兩個載入器與 dflang 指令、每種語言的 dflang/<語言>/，含 fonts/bold/、regular/ 各至少一份字型、
    fonts/ 底下沒有其他東西）、dflang/ 底下除了各語言沒有別的資料夾、沒有雜檔，且每種語言的字典、句型、名字音譯表與 data/ 一致，
    info.txt（版本號）與 workshop.json 一致：改了版本號沒重建就上傳，包裡還是舊版號（沒建置則略過）
12. 名字音譯表 data/translit/translit.tsv：欄位 en, zh-Hant, zh-Hans, note；音節只有小寫英文字母、不重複，繁簡都有
"""
import re
import sys
from pathlib import Path

from fontTools.ttLib import TTFont

from build import (DIST, FONTS, KINDS, LOADER, RETIRED_LOADER, WEIGHTS, dict_lines, info_txt, load_config, name_lines, read_rows,
                   translit_lines)
from gen_events import build as build_events
from cp437 import BYTE_OF, cells, encodable

ROOT = Path(__file__).resolve().parents[1]
HEADER = ["en", "zh-Hant", "zh-Hans", "align", "note"]
# data/screens 多一欄 row：只在畫面第幾列套用（空白是任何一列），同畫面同字不同意思時用
SCREEN_HEADER = ["en", "zh-Hant", "zh-Hans", "align", "row", "note"]
NAME_HEADER = ["word", "pos", "en", "zh-Hant", "zh-Hans", "note"]
TRANSLIT_HEADER = ["en", "zh-Hant", "zh-Hans", "note"]
HOOKS = ("addst", "addst_flag", "top_addst", "addcoloredst", "mtb_set_width", "add_anchored_tile", "wrap_text", "wrap_lines",
         "ctb_add_string", "translate_name", "native_name", "string_resize", "adv_appearance", "adv_personality", "adv_log", "adv_talk_log")
PACKAGE_SUFFIXES = {".txt", ".tsv", ".lua", ".dll", ".otf", ".md", ".png"}
# {1}～{9}；{#1} 只收數字、{@1} 只收字典裡的字詞、{%1} 只收翻得出來的片段、{~1} 同 {1} 但譯文可省略、{^1} 只收大寫開頭的名字、
# {*1} 只收每個字大寫、第一個字不在字典裡的人名、{?1} 同 {1} 但遊戲組的英文名拆回中文、{!1} 同 {1} 但原名在名字音譯開著時音譯；
# 書名句型的 {$1} 收書名的主題、{&1}、{=1} 收 raws 字表的名詞、形容詞；{_1} 收 raws 字表名詞的小寫詞形（程序生成的職位名）；
# 譯文可寫 {1+時}（佔位譯文沒有時間、原因助詞結尾時補字）
PLACEHOLDER = re.compile(r"\{[#@%~^$&=*?!_]?([1-9])(?:\+([^{}]+))?\}")
MARKUP = re.compile(r"\[(?:[RBP]|C:\d+:\d+:\d+)\]")  # 段落的換行與換色標記
# 由引擎依框寬重新換行的檔案（檔名開頭）：譯文寬度不限
WRAPPED = ("sentences_", "ann_", "speech_", "combat_", "desc_", "health_", "_events", "zz_events", "events_", "legends_", "zz_legends_",
           "adv_speech", "zz_adv_speech")

results: list[tuple[bool, str]] = []


def check(ok: bool, msg: str, problems=()) -> None:
    problems = list(problems)
    results.append((ok and not problems, msg + (f" {problems[:8]}" if problems else "")))


def leading_spaces(s: str) -> int:
    return len(s) - len(s.lstrip(" "))


def template(s: str) -> str:
    """句型去掉佔位後的樣板本身（補字照算格數）。"""
    return PLACEHOLDER.sub(lambda m: m.group(2) or "", s)


def placeholder_problem(r: dict, langs: list[str]) -> str | None:
    en = [m.group(1) for m in PLACEHOLDER.finditer(r["en"])]
    if not en:
        return "原文沒有佔位"
    if any(m.group(2) for m in PLACEHOLDER.finditer(r["en"])):
        return "原文佔位不能帶補字（+…）"
    if len(set(en)) != len(en):
        return "原文佔位重複"
    if re.search(r"\{[#@%~^$&=*?!_]?[1-9]\}\{[#@%~^$&=*?!_]?[1-9]\}", r["en"]):
        return "原文佔位相鄰"
    # 只有空白、標點夾著佔位的樣板（{1} {2}、{1}, {2}）會套到幾乎所有字串。有英文字、有限定佔位
    # （{#n} 只收數字、{@n} 只收字典字詞、{%n} 只收翻得出來的片段），或開頭、結尾是固定字（外框 -{1}-、後綴 {1}, ♂）時，套得到的字串有限
    lits = PLACEHOLDER.split(r["en"])  # 佔位前後的固定字；只有空白不算（` {1} {2}` 一樣會套到幾乎所有字串）
    edges = bool(lits[0].strip() or lits[-1].strip())
    if not re.search(r"[A-Za-z]", template(r["en"])) and not re.search(r"\{[#@%^$&=*_][1-9]\}", r["en"]) and not edges:
        return "原文樣板沒有英文字、限定佔位或頭尾固定字，會套到幾乎所有字串"
    # 譯文每個佔位最多一次、都要是原文有的；原文的佔位除了 {@n}、{~n} 都要用到（中文常省略重複的代名詞主詞、攻擊者自己的部位的主人）
    optional = set(re.findall(r"\{[@~]([1-9])\}", r["en"]))
    for lang in langs:
        zh = [m.group(1) for m in PLACEHOLDER.finditer(r[lang])]
        if r[lang] and (len(set(zh)) != len(zh) or not set(zh) <= set(en) or not set(en) - optional <= set(zh)):
            return f"{lang} 佔位和原文不一致"
    return None


def main() -> int:
    config = load_config()
    langs = [language["lang"] for language in config["languages"]]
    data: dict[str, list[dict]] = {}
    for kind in KINDS:
        files = sorted((ROOT / "data" / kind).glob("*.tsv"))
        want = SCREEN_HEADER if kind == "screens" else HEADER
        headers = [f"{kind}/{f.name}" for f in files if f.read_text(encoding="utf-8").split("\n", 1)[0].split("\t") != want]
        rows = [r for r in read_rows(kind) if f"{kind}/{r['file']}" not in headers]  # 欄名錯的檔案（含 BOM）其餘檢查無從判斷
        bad_cols = [f"{kind}/{r['file']}: {r.get('en')!r}" for r in rows if None in r.values() or None in r]
        check(not headers and not bad_cols, f"{kind} 欄位（{len(files)} 檔、{len(rows)} 條）", headers + bad_cols)
        data[kind] = [{**r, "file": f"{kind}/{r['file']}"} for r in rows if None not in r.values() and None not in r]
    rows = [r for kind in KINDS for r in data[kind]]
    bad_screens = [r for r in sorted({r["file"] for r in data["screens"]}) if not re.fullmatch(r"screens/[A-Za-z_][\w.-]*\.tsv", r)]
    check(not bad_screens, "data/screens 檔名是畫面（英數、底線、連字號，. 代表 /）", bad_screens)
    bad_rows = [f"{r['file']}: {r['en']!r} row={r['row']!r}" for r in data["screens"] if not re.fullmatch(r"\d*", r["row"])]
    check(not bad_rows, "data/screens 的 row 是空白或非負整數", bad_rows)
    # 世界生成名稱的字表（見引擎 names.rs）：一個字的一個詞性一列，繁簡都要有
    name_files = sorted((ROOT / "data/names").glob("*.tsv"))
    bad_name_headers = [f"names/{f.name}" for f in name_files if f.read_text(encoding="utf-8").split("\n", 1)[0].split("\t") != NAME_HEADER]
    names = [r for r in read_rows("names") if f"names/{r['file']}" not in bad_name_headers]
    seen_names: set[tuple[str, str]] = set()
    bad_names = []
    for r in names:
        key = (r.get("word"), r.get("pos"))
        if None in r.values() or None in r or r["pos"] not in ("NOUN", "ADJ", "PREFIX", "VERB") or key in seen_names \
                or any(not r[lang].strip() for lang in langs):
            bad_names.append(f"names/{r['file']}: {key}")
        seen_names.add(key)
    check(not bad_name_headers and not bad_names, f"names 字表（{len(name_files)} 檔、{len(names)} 條：代號＋詞性不重複、繁簡都有）",
          bad_name_headers + bad_names)
    # 名字音譯表（見引擎 translit.rs）：音節是小寫英文字母，繁簡都要有
    translit_files = sorted((ROOT / "data/translit").glob("*.tsv"))
    bad_translit_headers = [f"translit/{f.name}" for f in translit_files
                            if f.read_text(encoding="utf-8").split("\n", 1)[0].split("\t") != TRANSLIT_HEADER]
    translit = [r for r in read_rows("translit") if f"translit/{r['file']}" not in bad_translit_headers]
    seen_syllables: set[str] = set()
    bad_translit = []
    for r in translit:
        if None in r.values() or None in r or not re.fullmatch(r"[a-z]+", r["en"]) or r["en"] in seen_syllables \
                or any(not r[lang].strip() for lang in langs):
            bad_translit.append(f"translit/{r['file']}: {r.get('en')!r}")
        seen_syllables.add(r["en"])
    check(translit_files and not bad_translit_headers and not bad_translit,
          f"名字音譯表（{len(translit)} 個音節：小寫英文字母、不重複、繁簡都有）", bad_translit_headers + bad_translit)

    seen: dict[tuple, dict] = {}
    dupes, bad_src, bad_align = [], [], []
    for r in rows:
        kind = r["file"].split("/")[0]
        key = (r["file"] if kind == "screens" else kind, r["en"], r["align"] if r["align"] in ("tail", "title", "key") else "")  # tail 片語、書名用詞、按鍵名自成一類（引擎另外存）
        first = seen.get(key)
        # 同一條句型一份只給佔位用（inner）、一份整句也用，譯文相同時不衝突（傳說事件短語與冒險日誌標題共用）
        same_rule = first is not None and kind == "rules" and (first["align"] == "inner") != (r["align"] == "inner") \
            and all(first[lang] == r[lang] for lang in langs)
        if first is not None and not same_rule:
            dupes.append(f"{r['file']}: {r['en']!r}")
        seen.setdefault(key, r)
        if not r["en"] or not encodable(r["en"]):
            bad_src.append(f"{r['file']}: {r['en']!r}")
        if r["align"] not in ("", "c", "r") and not (kind in ("rules", "strings") and r["align"] in ("inner", "tail")) \
                and not (kind == "strings" and r["align"] in ("title", "key")) and not (kind == "screens" and r["align"] == "music"):
            bad_align.append(f"{r['file']}: {r['en']!r} align={r['align']!r}")
    check(not dupes, "原文不重複", dupes)
    check(not bad_src, "原文能用 CP437 表示", bad_src)
    check(not bad_align, "align 只能是空、c、r（句型與字串另可寫 inner、tail，字串另可寫 title、key，依畫面譯文另可寫 music）", bad_align)
    bad_ph = [f"{r['file']}: {r['en']!r} {p}" for r in data["rules"] if (p := placeholder_problem(r, langs))]
    check(not bad_ph, f"句型佔位寫法（{len(data['rules'])} 條）", bad_ph)
    events, _ = build_events()
    stale_events = [p.name for p, text in events.items() if not p.is_file() or p.read_text(encoding="utf-8") != text]
    check(not stale_events, "data/rules/_events.tsv、zz_events.tsv 與 data/events 一致（改了 data/events 要重跑 uv run scripts/gen_events.py）",
          stale_events)

    unpaired = [f"{r['file']}: {r['en']!r}" for r in rows if bool(r["zh-Hant"].strip()) != bool(r["zh-Hans"].strip())]
    check(not unpaired, "繁簡兩欄同時有譯文或同時留空", unpaired)
    indent = [f"{r['file']}: {r['en']!r} [{lang}]" for r in rows for lang in langs
              if r[lang] and r.get("align") not in ("inner", "title", "c") and leading_spaces(r[lang]) != leading_spaces(r["en"])]
    check(not indent, "譯文開頭空白與原文相同（inner、置中除外）", indent)
    def room(r: dict) -> int | None:
        """譯文最多幾格；None 是不限：WRAPPED 開頭的檔案由引擎依框寬重新換行；inner 只接進句型的佔位，不單獨畫；key 的按鍵名由引擎
        照字型實際字寬比、放不下照英文；note 寫「寬度例外」的條目已在實機確認右邊有空位（欄名「Adv」只有 3 格，「冒險」要 4 格）"""
        if r["file"].split("/")[1].startswith(WRAPPED) or r.get("align") in ("inner", "title", "key") or "寬度例外" in (r.get("note") or ""):
            return None
        en = cells(template(r["en"]))
        return max(en, 6) if r["file"].startswith("strings/names_") else en

    wide = [f"{r['file']}: {r['en']!r} [{lang}] {cells(template(r[lang]))}>{room(r)}"
            for r in rows for lang in langs if r[lang] and room(r) is not None and cells(template(r[lang])) > room(r)]
    check(not wide, "譯文格數不超過原文（引擎重新換行的檔案不限）", wide)
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

    for language in config["languages"]:
        lang = language["lang"]
        if sorted(language.get("fonts", {})) != sorted(WEIGHTS):
            check(False, f"{lang} 字型：workshop.json 的 fonts 要正好有 {'、'.join(WEIGHTS)}（lua/loader.lua 照這些資料夾名找）")
            continue
        used = {c for r in [*rows, *translit] for c in r[lang] if c not in BYTE_OF}
        used |= set(language.get("mascot_label", "") + config["author"]) - {" "}  # 名牌由引擎用字型畫，英數也要有
        for weight in WEIGHTS:
            fonts = [FONTS / f for f in language["fonts"][weight]]
            if not fonts or not all(f.is_file() for f in fonts):
                check(False, f"{lang} {weight} 字型：temp/fonts/ 缺字型，先跑 uv run scripts/build.py --fetch-fonts")
                continue
            cmaps = [TTFont(f).getBestCmap() for f in fonts]
            missing = sorted(c for c in used if not any(ord(c) in cmap for cmap in cmaps))
            check(not missing, f"{lang} {weight} 字型涵蓋 {len(used)} 個字（含吉祥物名牌）", missing)

    mascot = ROOT / "art/mascot.png"
    with mascot.open("rb") as fh:  # PNG IHDR：寬、高各 4 bytes，再來是 bit depth 與 color type
        head = fh.read(26)
    rgba8 = head[:8] == b"\x89PNG\r\n\x1a\n" and head[24] == 8 and head[25] == 6
    labels = [f"{language['lang']}: {language.get('mascot_label')!r}" for language in config["languages"]
              if not language.get("mascot_label") or any(c in BYTE_OF for c in language["mascot_label"])]
    check(rgba8 and not labels, "art/mascot.png 是 8-bit RGBA PNG；每種語言都有全形字的 mascot_label", labels)

    meta = [k for k in ("id", "name", "description") if not config[k].isascii()] \
        + ([] if config["author"].isascii() and config["author"] else ["author"])
    check(sorted(langs) == ["zh-Hans", "zh-Hant"] and not meta, "workshop.json 的 languages 正好是繁、簡兩種、info.txt 欄位為 ASCII", meta)
    numeric = config["version"]["numeric"]
    check(isinstance(numeric, int) and 9000 <= numeric <= 9999, f"workshop.json 的數字版本號 {numeric} 是 9 開頭的四位數（DFHack 照字串比版本）")

    patterns = {line.split("\t")[0] for line in (ROOT / "engine/patterns.tsv").read_text(encoding="utf-8").splitlines()
                if line and not line.startswith("#")}
    check(all(h in patterns for h in HOOKS), f"engine/patterns.tsv 含 {len(HOOKS)} 個 hook 函式", [h for h in HOOKS if h not in patterns])

    out = DIST / config["folder"]
    if not out.is_dir():
        results.append((True, "dist 未建置，略過"))
    else:
        required = ["info.txt", f"scripts_modinstalled/{LOADER}", f"scripts_modinstalled/{RETIRED_LOADER}", "scripts_modinstalled/dflang.lua",
                    "dflang/dflang.dll", "dflang/patterns.tsv", "dflang/LICENSE.txt", "dflang/LICENSE-fonts.txt",
                    "dflang/THIRD-PARTY-LICENSES.txt", "dflang/CREDITS.md", "dflang/mascot.png"]
        built_files = {"info.txt": info_txt(config)}
        for lang in langs:
            folder = f"dflang/{lang}"
            required += [*(f"{folder}/{kind}.tsv" for kind in KINDS), f"{folder}/names.tsv", f"{folder}/translit.tsv", f"{folder}/glyphs.txt"]
            built_files |= {f"{folder}/{kind}.tsv": dict_lines(kind, data[kind], lang) for kind in KINDS}
            built_files[f"{folder}/names.tsv"] = name_lines(names, lang)
            built_files[f"{folder}/translit.tsv"] = translit_lines(translit, lang)
        lacking = [p for p in required if not (out / p).is_file()] + [f"dflang/{lang}/fonts/{w}/*.otf" for lang in langs for w in WEIGHTS
                                                                      if not list((out / "dflang" / lang / "fonts" / w).glob("*.otf"))]
        stray = [p.relative_to(out).as_posix() for p in out.rglob("*") if p.is_file() and p.suffix not in PACKAGE_SUFFIXES]
        stray += [p.relative_to(out).as_posix() for lang in langs for p in (out / "dflang" / lang / "fonts").glob("*") if p.name not in WEIGHTS]
        stray += [p.relative_to(out).as_posix() for p in (out / "dflang").glob("*") if p.is_dir() and p.name not in langs]
        stale = [f"{path} 過期，重跑 build.py" for path, expected in built_files.items()
                 if (out / path).is_file() and (out / path).read_text(encoding="utf-8") != expected]
        check(not lacking and not stray and not stale, "dist 檔案齊全、無雜檔，各語言的字典、句型、名字音譯表與版本和 data/、workshop.json 一致",
              lacking + stray + stale)

    for ok, msg in results:
        print(("PASS " if ok else "FAIL ") + msg)
    failed = sum(not ok for ok, _ in results)
    print(f"{len(results) - failed}/{len(results)} PASS")
    return 1 if failed else 0


if __name__ == "__main__":
    sys.exit(main())
