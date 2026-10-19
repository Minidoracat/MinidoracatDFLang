# /// script
# requires-python = ">=3.11"
# dependencies = ["fonttools>=4.50"]
# ///
"""組出繁、簡兩種語言合在一起的工作坊包到 dist/<folder>/（內容就是上傳到工作坊的資料夾）；遊戲裡由載入器挑語言
（第一次啟動照系統判斷，設定視窗可以隨時切換）。

包的內容：info.txt、scripts_modinstalled/（載入器、停用簡中項目的同名腳本與 dflang 指令）、dflang/（兩種語言共用的引擎 DLL、特徵碼、
標題畫面吉祥物 mascot.png、授權與致謝；每種語言各一個 dflang/<語言>/：整句字典 strings.tsv、句型規則 rules.tsv、依畫面套用的整句 screens.tsv、
名稱字表 names.tsv、名字音譯表 translit.tsv、字型子集）。字型是 workshop.json 每種語言指定的思源黑體（Noto Sans CJK）Bold 與 Regular 兩種字重
（fonts 的 bold、regular，各自 dflang/<語言>/fonts/<字重>/），都只留該語言譯文、名字音譯表與吉祥物標籤（mascot_label）用到的非 CP437 字；
每種字重的第一份字型沒有的字依清單順序補成第二、第三份字型（品質記號 ☼ 由 Noto Sans Symbols 2 補），任一字重有字全部都沒有就建置失敗。
引擎在遊戲啟動時依當下的文字格大小把玩家選的字重（dfhack-config/dflang.json 的 bold，預設粗體）畫成字圖集，所以字型檔就是字圖的唯一來源。

用法（repo 根目錄）：
  uv run scripts/build.py --fetch-fonts   # 首次：下載字型與授權到 temp/fonts/（已下載的略過）
  uv run scripts/build.py                 # 改了譯文、引擎或 Lua 後重跑（先關遊戲：執行中會鎖住 DLL）
  uv run scripts/build.py --skip-engine   # 沿用 engine/target 已編好的 DLL
"""
import argparse
import csv
import json
import shutil
import subprocess
import sys
import urllib.request
from pathlib import Path

from fontTools import subset
from fontTools.pens.boundsPen import BoundsPen
from fontTools.pens.transformPen import TransformPen
from fontTools.pens.ttGlyphPen import TTGlyphPen
from fontTools.ttLib import TTFont

from cp437 import BYTE_OF

ROOT = Path(__file__).resolve().parents[1]
DIST = ROOT / "dist"
KINDS = ("strings", "rules", "screens")
# workshop.json 每種語言的 fonts 要有的字重，也是 dist 的 dflang/<語言>/fonts/ 底下的資料夾名（lua/loader.lua 照這兩個名字找）
WEIGHTS = ("bold", "regular")
FONTS = ROOT / "temp/fonts"
# 字型的授權全文：temp/fonts/ 的檔名 → (包裡 LICENSE-fonts.txt 的段落標題, 下載網址)
FONT_LICENSES = {
    "LICENSE": ("Noto Sans CJK", "https://raw.githubusercontent.com/notofonts/noto-cjk/main/Sans/LICENSE"),
    "OFL-NotoSansSymbols2.txt": ("Noto Sans Symbols 2", "https://raw.githubusercontent.com/notofonts/symbols/main/OFL.txt"),
}
# 沿用原繁中包的載入器檔名：DFHack overlay 小工具名稱與玩家存下的位置（dflang_zh_hant.mascot、.hint）不變
LOADER = "dflang_zh_hant.lua"
# 和停用的簡中項目的載入器同名：DFHack 同名腳本只執行腳本路徑上的第一份，本包排在前面，停用項目的載入器就不會一起跑
RETIRED_LOADER = "dflang_zh_hans.lua"
PACKAGE_LICENSE = """程式（dflang/dflang.dll、scripts_modinstalled/*.lua）：MIT License，全文附在下方。
dflang.dll 靜態連結的開源 Rust 套件（字型解析、PNG、函式 hook 等）：各自的授權與著作權聲明見 THIRD-PARTY-LICENSES.txt。
譯文（dflang/<語言>/strings.tsv、rules.tsv、screens.tsv）：CC BY 4.0，https://creativecommons.org/licenses/by/4.0/
字型（dflang/<語言>/fonts/）：SIL Open Font License 1.1，全文見 LICENSE-fonts.txt。
吉祥物圖（dflang/mascot.png）：© Minidoracat，保留所有權利，不適用上述授權。

"""
MIT_TEXT = """Permission is hereby granted, free of charge, to any person obtaining a copy of this software and associated
documentation files (the "Software"), to deal in the Software without restriction, including without limitation the
rights to use, copy, modify, merge, publish, distribute, sublicense, and/or sell copies of the Software, and to permit
persons to whom the Software is furnished to do so, subject to the following conditions:

The above copyright notice and this permission notice shall be included in all copies or substantial portions of the
Software.

THE SOFTWARE IS PROVIDED "AS IS", WITHOUT WARRANTY OF ANY KIND, EXPRESS OR IMPLIED, INCLUDING BUT NOT LIMITED TO THE
WARRANTIES OF MERCHANTABILITY, FITNESS FOR A PARTICULAR PURPOSE AND NONINFRINGEMENT. IN NO EVENT SHALL THE AUTHORS OR
COPYRIGHT HOLDERS BE LIABLE FOR ANY CLAIM, DAMAGES OR OTHER LIABILITY, WHETHER IN AN ACTION OF CONTRACT, TORT OR
OTHERWISE, ARISING FROM, OUT OF OR IN CONNECTION WITH THE SOFTWARE OR THE USE OR OTHER DEALINGS IN THE SOFTWARE."""
# 套件裡另外帶著別人的 C 原始碼、授權寫在原始檔開頭的（libudis86-sys 內含 udis86，BSD 2-Clause）
BUNDLED_NOTICES = {"libudis86-sys": "libudis86/udis86.c"}


def third_party_licenses() -> str:
    """dflang.dll 靜態連結的 Rust 套件（不含只在編譯時執行的 proc-macro 與 build 相依）：名稱、版本、授權、來源與授權檔全文。
    同樣的授權全文只寫一次，前面列出用它的套件。套件沒附授權檔時照 Cargo.toml 的作者寫 MIT 聲明"""
    meta = json.loads(subprocess.run(
        ["cargo", "metadata", "--format-version", "1", "--manifest-path", str(ROOT / "engine/Cargo.toml"),
         "--filter-platform", "x86_64-pc-windows-msvc"], check=True, capture_output=True, text=True, encoding="utf-8").stdout)
    packages = {p["id"]: p for p in meta["packages"]}
    nodes = {n["id"]: n for n in meta["resolve"]["nodes"]}
    linked, todo = set(), [meta["resolve"]["root"]]
    while todo:
        for dep in nodes[todo.pop()]["deps"]:
            pkg = packages[dep["pkg"]]
            normal = any(k["kind"] is None for k in dep["dep_kinds"])
            proc_macro = any("proc-macro" in t["kind"] for t in pkg["targets"])
            if normal and not proc_macro and dep["pkg"] not in linked:
                linked.add(dep["pkg"])
                todo.append(dep["pkg"])
    texts: dict[str, list[str]] = {}
    listing = []
    for p in sorted((packages[i] for i in linked), key=lambda p: (p["name"], p["version"])):
        crate = f"{p['name']} {p['version']}"
        listing.append(f"{crate}  授權：{p['license']}  來源：{p.get('repository') or 'https://crates.io/crates/' + p['name']}")
        folder = Path(p["manifest_path"]).parent
        files = sorted(f for f in folder.iterdir() if f.is_file() and f.name.upper().startswith(("LICENSE", "LICENCE", "COPYING", "NOTICE")))
        found = [f.read_text(encoding="utf-8", errors="replace").strip() for f in files]
        if not found:
            found = [f"Copyright (c) {', '.join(a.split(' <')[0] for a in p['authors'])}\n\n{MIT_TEXT}"] if "MIT" in (p["license"] or "") else []
        if p["name"] in BUNDLED_NOTICES:
            header = (folder / BUNDLED_NOTICES[p["name"]]).read_text(encoding="utf-8").split("*/")[0]
            found.append("\n".join(line.strip(" */") for line in header.splitlines()).strip())
        if not found:
            raise SystemExit(f"FAIL {crate}：找不到授權檔，也沒辦法照 Cargo.toml 寫聲明")
        for text in found:
            texts.setdefault(text, []).append(crate)
    blocks = [f"=== {'、'.join(crates)} ===\n\n{text}\n" for text, crates in texts.items()]
    # 拼音搜尋的讀音表（engine/pinyin.tsv）編進 DLL，來源是 Unicode 的 Unihan 資料庫
    blocks.append(f"=== 讀音表 engine/pinyin.tsv（Unicode Unihan） ===\n\n{(ROOT / 'engine/LICENSE-unicode.txt').read_text(encoding='utf-8').strip()}\n")
    return ("dflang.dll 相依的開源 Rust 套件（cargo metadata 的相依圖，可能多列幾個實際沒連結進去的），以及編進 DLL 的 Unicode Unihan 讀音資料。"
            "本檔由 scripts/build.py 產生。\n\n" + "\n".join(listing) + "\n\n" + "\n".join(blocks))


def load_config() -> dict:
    return json.loads((ROOT / "scripts/workshop.json").read_text(encoding="utf-8"))


def read_rows(folder: str) -> list[dict]:
    """data/<folder>/*.tsv 的所有列，附上來源檔名。strings 是整句字典，rules 是句型規則，screens 是依畫面套用的整句。"""
    rows = []
    for path in sorted((ROOT / "data" / folder).glob("*.tsv")):
        with path.open(encoding="utf-8", newline="") as fh:
            for row in csv.DictReader(fh, delimiter="\t", quoting=csv.QUOTE_NONE):
                rows.append({**row, "file": path.name})
    return rows


def screen_of(file: str) -> str:
    """data/screens 的檔名就是畫面（DFHack 焦點字串的開頭），. 代表 /：dwarfmode.Info.tsv → dwarfmode/Info。"""
    return Path(file).stem.replace(".", "/")


def dict_lines(kind: str, rows: list[dict], lang: str) -> str:
    """dist 的 dflang/<語言>/<kind>.tsv：原文<TAB>譯文<TAB>對齊；screens 前面多一欄畫面、後面多一欄限定列。沒有譯文的列不放；
    screens 原文寫 `*` 的列（這個畫面的全小寫字串照原文）不看譯文欄，一律放。"""
    def line(r: dict) -> str:
        if kind == "screens":
            return f"{screen_of(r['file'])}\t{r['en']}\t{r[lang]}\t{r['align']}\t{r['row']}\n"
        return f"{r['en']}\t{r[lang]}\t{r['align']}\n"
    return "".join(line(r) for r in rows if r[lang].strip() or (kind == "screens" and r["en"] == "*"))


def name_lines(rows: list[dict], lang: str) -> str:
    """dist 的 dflang/<語言>/names.tsv：字的代號<TAB>詞性<TAB>中文<TAB>英文詞形（世界生成名稱的字表；英文詞形給書名的隨機字用，見引擎 names.rs）。"""
    return "".join(f"{r['word']}\t{r['pos']}\t{r[lang]}\t{r['en']}\n" for r in rows if r[lang].strip())


def translit_lines(rows: list[dict], lang: str) -> str:
    """dist 的 dflang/<語言>/translit.tsv：音節<TAB>中文（原生語言名字的音譯表，設定視窗的名字音譯開著才載入，見引擎 translit.rs）。"""
    return "".join(f"{r['en']}\t{r[lang]}\n" for r in rows if r[lang].strip())


def text_counts(texts: dict[str, list[dict]], names: list[dict], lang: str) -> list[int]:
    """KINDS 各有幾條有譯文的列，最後再加名稱用字數；建置印出的數字和 README 那一行（publish_workshop.py 檢查）都照這個算。"""
    return [sum(1 for r in rows if r[lang].strip()) for rows in [*(texts[kind] for kind in KINDS), names]]


def fetch_fonts(sources: dict[str, str]) -> None:
    """workshop.json 的 font_sources：字型檔名 → 下載網址；已下載的略過。"""
    FONTS.mkdir(parents=True, exist_ok=True)
    for name, url in [*sorted(sources.items()), *((name, url) for name, (_, url) in FONT_LICENSES.items())]:
        dest = FONTS / name
        if not dest.is_file():
            print(f"downloading {name} ...")
            urllib.request.urlretrieve(url, dest)


def match_metrics(font: TTFont, primary: TTFont) -> None:
    """補字字型的垂直度量改成第一份字型的：引擎照每份字型自己的 ascent、descent 放基線（engine/src/atlas.rs 的 draw_scaled），
    不一樣時補的字會上下跑掉"""
    if font["head"].unitsPerEm != primary["head"].unitsPerEm:
        raise ValueError(f"補字字型的 unitsPerEm 和第一份字型不同：{font['head'].unitsPerEm} ≠ {primary['head'].unitsPerEm}")
    for table, fields in (("hhea", ("ascent", "descent", "lineGap")),
                          ("OS/2", ("sTypoAscender", "sTypoDescender", "sTypoLineGap", "usWinAscent", "usWinDescent"))):
        for field in fields:
            setattr(font[table], field, getattr(primary[table], field))
    use_typo = 1 << 7
    font["OS/2"].fsSelection = font["OS/2"].fsSelection & ~use_typo | primary["OS/2"].fsSelection & use_typo


def fit_symbol(font: TTFont, ch: str, primary: TTFont) -> None:
    """補字字型的 CP437 符號（☼：Symbols 2 的比中文字還大）縮到和第一份字型的大寫字母一樣高、放在大寫字母高度的正中間，
    字寬照縮過的字身，左右各留一點空，和數字接在一起（「2664☼」）。Symbols 2 只有細體：第一份字型比 Regular 粗多少，
    就往右、往上各疊一份字形加粗多少（字重每多 100 加 10 單位，Bold 加 30）"""
    name = font.getBestCmap()[ord(ch)]
    glyphs = font.getGlyphSet()
    bounds = BoundsPen(glyphs)
    glyphs[name].draw(bounds)
    x0, y0, x1, y1 = bounds.bounds
    cap = primary["OS/2"].sCapHeight
    k = cap / (y1 - y0)
    side = round(cap * 0.08)
    bold = max(0, primary["OS/2"].usWeightClass - 400) // 10
    pen = TTGlyphPen(glyphs)
    for dx, dy in {(dx, dy) for dx in (0, bold) for dy in (0, bold)}:
        glyphs[name].draw(TransformPen(pen, (k, 0, 0, k, side + dx - x0 * k, cap / 2 + dy - bold / 2 - (y0 + y1) / 2 * k)))
    font["glyf"][name] = pen.glyph()
    font["hmtx"][name] = (round((x1 - x0) * k) + 2 * side + bold, side)


def subset_fonts(chars: list[str], fonts: list[str], out_dir: Path) -> list[str]:
    """依序讓每份字型負責它有、前面字型沒有的字，各存成 <序號>-<原檔名>；回傳沒有字型能畫的字。
    補字字型照第一份字型的垂直度量（match_metrics）；補到 CP437 符號的照 fit_symbol 改字形，字形改過就不留 hinting（對不上了）"""
    out_dir.mkdir(parents=True, exist_ok=True)
    primary = TTFont(FONTS / fonts[0])
    remaining = list(chars)
    for i, name in enumerate(fonts):
        font = TTFont(FONTS / name)
        cmap = font.getBestCmap()
        take = [c for c in remaining if ord(c) in cmap]
        if not take:
            continue
        symbols = [c for c in take if c in BYTE_OF] if i else []
        options = subset.Options()
        options.name_IDs = ["*"]
        options.notdef_outline = True
        options.hinting = not symbols
        subsetter = subset.Subsetter(options)
        subsetter.populate(unicodes=[ord(c) for c in take])
        subsetter.subset(font)
        if i:
            match_metrics(font, primary)
        for c in symbols:
            fit_symbol(font, c, primary)
        font.save(out_dir / f"{i}-{name}")
        remaining = [c for c in remaining if c not in take]
    return remaining


def build_engine() -> Path:
    subprocess.run(["cargo", "build", "--release", "--manifest-path", str(ROOT / "engine/Cargo.toml")], check=True)
    return ROOT / "engine/target/release/dflang.dll"


def info_txt(config: dict) -> str:
    version = config["version"]
    return "".join(f"[{k}:{v}]\n" for k, v in [
        ("ID", config["id"]),
        ("NUMERIC_VERSION", version["numeric"]),
        ("DISPLAYED_VERSION", version["displayed"]),
        ("EARLIEST_COMPATIBLE_NUMERIC_VERSION", version["earliest_compatible_numeric"]),
        ("EARLIEST_COMPATIBLE_DISPLAYED_VERSION", version["earliest_compatible_displayed"]),
        ("AUTHOR", config["author"]),
        ("NAME", config["name"]),
        ("DESCRIPTION", config["description"]),
    ])


def write_text(path: Path, text: str) -> None:
    path.parent.mkdir(parents=True, exist_ok=True)
    path.write_text(text, encoding="utf-8", newline="\n")


def build_language(language: dict, config: dict, texts: dict[str, list[dict]], names: list[dict], translit: list[dict],
                   data: Path) -> int:
    """一種語言的 dflang/<語言>/：字典、句型、名稱字表、音譯表與字型子集。"""
    lang = language["lang"]
    chars = sorted({c for rows in [*texts.values(), names, translit] for r in rows for c in r[lang] if c not in BYTE_OF})
    # 標題畫面吉祥物的名牌（語言標籤、作者名）由引擎在遊戲裡用粗體子集畫（不跟著字重設定），字型子集也要有這些字（含英數）；
    # 整句貼圖把遊戲畫的英數、空白、CP437 的拉丁字母（矮人名字的 ë、ü…）、♂♀、譯名的間隔號 ·、品質記號 ≡ ☼ 與裝飾外框 «»
    # 也用玩家選的字重畫（「神殿，2664☼」「被≡鋼盾牌≡擋下了」），和引擎 overlay.rs 的 is_text 同一組，字型子集也要有。
    # 兩種字重用同一份字集，切換字重不會少字
    ascii = "".join(map(chr, range(0x20, 0x7F))) + "ÇüéâäàåçêëèïîìÄÅÉæÆôöòûùÿÖÜáíóúñÑ♂♀·≡☼«»"
    tag_chars = [c for c in dict.fromkeys(language["mascot_label"] + config["author"] + ascii) if c not in chars]
    for weight in WEIGHTS:
        missing = subset_fonts(chars + tag_chars, language["fonts"][weight], data / "fonts" / weight)
        if missing:
            print(f"FAIL {lang}: {weight} 沒有字型能畫 {''.join(missing)}", file=sys.stderr)
            return 1
    write_text(data / "glyphs.txt", "".join(chars) + "\n")
    for kind in KINDS:
        write_text(data / f"{kind}.tsv", dict_lines(kind, texts[kind], lang))
    write_text(data / "names.tsv", name_lines(names, lang))
    write_text(data / "translit.tsv", translit_lines(translit, lang))
    *kinds, words = text_counts(texts, names, lang)
    counts = "、".join(f"{n} 條{name}" for n, name in zip(kinds, ("譯文", "句型", "依畫面譯文")))
    print(f"{lang}: {counts}、{words} 個名稱用字、{len(chars)} 個字 -> {data.relative_to(ROOT)}")
    return 0


def build(config: dict, texts: dict[str, list[dict]], names: list[dict], translit: list[dict], dll: Path, notices: str) -> int:
    out = DIST / config["folder"]
    try:
        if out.exists():
            shutil.rmtree(out)
    except PermissionError as e:
        print(f"FAIL: 無法清除 {out}（遊戲執行中會鎖住 dflang.dll，先關遊戲）：{e}", file=sys.stderr)
        return 1
    data = out / "dflang"
    data.mkdir(parents=True)
    for language in config["languages"]:
        if build_language(language, config, texts, names, translit, data / language["lang"]):
            return 1

    patterns = (ROOT / "engine/patterns.tsv").read_text(encoding="utf-8").splitlines()
    write_text(data / "patterns.tsv", "".join(f"{line}\n" for line in patterns if line and not line.startswith("#")))
    shutil.copy2(dll, data / "dflang.dll")
    write_text(data / "LICENSE.txt", PACKAGE_LICENSE + (ROOT / "LICENSE").read_text(encoding="utf-8"))
    write_text(data / "THIRD-PARTY-LICENSES.txt", notices)
    write_text(data / "LICENSE-fonts.txt", "\n\n".join(f"== {title} ==\n\n{(FONTS / name).read_text(encoding='utf-8').strip()}\n"
                                                     for name, (title, _) in FONT_LICENSES.items()))
    shutil.copy2(ROOT / "CREDITS.md", data / "CREDITS.md")
    shutil.copy2(ROOT / "art/mascot.png", data / "mascot.png")

    langs = ", ".join(f"{{lang = '{x['lang']}', label = '{x['mascot_label']}'}}" for x in config["languages"])
    loader = ((ROOT / "lua/loader.lua").read_text(encoding="utf-8")
              .replace("@MOD_ID@", config["id"]).replace("@VERSION@", config["version"]["displayed"])
              .replace("@AUTHOR@", config["author"]).replace("@LANGS@", f"{{{langs}}}"))
    write_text(out / "scripts_modinstalled" / LOADER, loader)
    shutil.copy2(ROOT / "lua/retired_zh_hans.lua", out / "scripts_modinstalled" / RETIRED_LOADER)
    shutil.copy2(ROOT / "lua/dflang.lua", out / "scripts_modinstalled/dflang.lua")
    write_text(out / "info.txt", info_txt(config))
    return 0


def main() -> int:
    ap = argparse.ArgumentParser()
    ap.add_argument("--fetch-fonts", action="store_true")
    ap.add_argument("--skip-engine", action="store_true")
    args = ap.parse_args()
    config = load_config()
    if args.fetch_fonts:
        fetch_fonts(config["font_sources"])
    dll = ROOT / "engine/target/release/dflang.dll" if args.skip_engine else build_engine()
    texts = {kind: read_rows(kind) for kind in KINDS}
    names = read_rows("names")
    translit = read_rows("translit")
    return build(config, texts, names, translit, dll, third_party_licenses())


if __name__ == "__main__":
    sys.exit(main())
