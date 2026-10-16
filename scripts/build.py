# /// script
# requires-python = ">=3.11"
# dependencies = ["fonttools>=4.50"]
# ///
"""組出繁、簡兩個工作坊包到 dist/<folder>/（內容就是上傳到工作坊的資料夾）。

每包內容：info.txt、scripts_modinstalled/（載入器與 dflang 指令）、dflang/（引擎 DLL、整句字典 strings.tsv、
句型規則 rules.tsv、依畫面套用的整句 screens.tsv、字型子集、特徵碼、標題畫面吉祥物 mascot.png、授權與致謝）。字型是 workshop.json 指定的
思源黑體（Noto Sans CJK）Bold 與 Regular 兩種字重（fonts 的 bold、regular，各自 dflang/fonts/<字重>/），都只留本包譯文與吉祥物標籤
（mascot_label）用到的非 CP437 字；每種字重的第一份字型沒有的字依清單順序補成第二、第三份字型，任一字重有字全部都沒有就建置失敗。
引擎在遊戲啟動時依當下的文字格大小把玩家選的字重（dfhack-config/dflang.json 的 bold，預設粗體）畫成字圖集，所以字型檔就是字圖的唯一來源。

用法（repo 根目錄）：
  uv run scripts/build.py --fetch-fonts   # 首次：下載思源黑體與授權到 temp/fonts/
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
from fontTools.ttLib import TTFont

from cp437 import BYTE_OF

ROOT = Path(__file__).resolve().parents[1]
DIST = ROOT / "dist"
KINDS = ("strings", "rules", "screens")
# workshop.json 每包的 fonts 要有的字重，也是 dist 的 dflang/fonts/ 底下的資料夾名（lua/loader.lua 照這兩個名字找）
WEIGHTS = ("bold", "regular")
FONTS = ROOT / "temp/fonts"
FONT_LICENSE_URL = "https://raw.githubusercontent.com/notofonts/noto-cjk/main/Sans/LICENSE"
PACKAGE_LICENSE = """程式（dflang/dflang.dll、scripts_modinstalled/*.lua）：MIT License，全文附在下方。
dflang.dll 靜態連結的開源 Rust 套件（字型解析、PNG、函式 hook 等）：各自的授權與著作權聲明見 THIRD-PARTY-LICENSES.txt。
譯文（dflang/strings.tsv、dflang/rules.tsv、dflang/screens.tsv）：CC BY 4.0，https://creativecommons.org/licenses/by/4.0/
字型（dflang/fonts/）：SIL Open Font License 1.1，全文見 LICENSE-fonts.txt。
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
    return ("dflang.dll 相依的開源 Rust 套件（cargo metadata 的相依圖，可能多列幾個實際沒連結進去的）。本檔由 scripts/build.py 產生。\n\n"
            + "\n".join(listing) + "\n\n" + "\n".join(blocks))


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
    """dist 的 dflang/<kind>.tsv：原文<TAB>譯文<TAB>對齊；screens 前面多一欄畫面、後面多一欄限定列。沒有譯文的列不放；
    screens 原文寫 `*` 的列（這個畫面的全小寫字串照原文）不看譯文欄，一律放。"""
    def line(r: dict) -> str:
        if kind == "screens":
            return f"{screen_of(r['file'])}\t{r['en']}\t{r[lang]}\t{r['align']}\t{r['row']}\n"
        return f"{r['en']}\t{r[lang]}\t{r['align']}\n"
    return "".join(line(r) for r in rows if r[lang].strip() or (kind == "screens" and r["en"] == "*"))


def name_lines(rows: list[dict], lang: str) -> str:
    """dist 的 dflang/names.tsv：字的代號<TAB>詞性<TAB>中文<TAB>英文詞形（世界生成名稱的字表；英文詞形給書名的隨機字用，見引擎 names.rs）。"""
    return "".join(f"{r['word']}\t{r['pos']}\t{r[lang]}\t{r['en']}\n" for r in rows if r[lang].strip())


def fetch_fonts(sources: dict[str, str]) -> None:
    """workshop.json 的 font_sources：字型檔名 → 下載網址；已下載的略過。"""
    FONTS.mkdir(parents=True, exist_ok=True)
    for name, url in [*sorted(sources.items()), ("LICENSE", FONT_LICENSE_URL)]:
        dest = FONTS / name
        if not dest.is_file():
            print(f"downloading {name} ...")
            urllib.request.urlretrieve(url, dest)


def subset_fonts(chars: list[str], fonts: list[str], out_dir: Path) -> list[str]:
    """依序讓每份字型負責它有、前面字型沒有的字，各存成 <序號>-<原檔名>；回傳沒有字型能畫的字。"""
    out_dir.mkdir(parents=True, exist_ok=True)
    remaining = list(chars)
    for i, name in enumerate(fonts):
        font = TTFont(FONTS / name)
        cmap = font.getBestCmap()
        take = [c for c in remaining if ord(c) in cmap]
        if not take:
            continue
        options = subset.Options()
        options.name_IDs = ["*"]
        options.notdef_outline = True
        subsetter = subset.Subsetter(options)
        subsetter.populate(unicodes=[ord(c) for c in take])
        subsetter.subset(font)
        font.save(out_dir / f"{i}-{name}")
        remaining = [c for c in remaining if c not in take]
    return remaining


def build_engine() -> Path:
    subprocess.run(["cargo", "build", "--release", "--manifest-path", str(ROOT / "engine/Cargo.toml")], check=True)
    return ROOT / "engine/target/release/dflang.dll"


def info_txt(item: dict, config: dict) -> str:
    version = config["version"]
    return "".join(f"[{k}:{v}]\n" for k, v in [
        ("ID", item["id"]),
        ("NUMERIC_VERSION", version["numeric"]),
        ("DISPLAYED_VERSION", version["displayed"]),
        ("EARLIEST_COMPATIBLE_NUMERIC_VERSION", version["earliest_compatible_numeric"]),
        ("EARLIEST_COMPATIBLE_DISPLAYED_VERSION", version["earliest_compatible_displayed"]),
        ("AUTHOR", config["author"]),
        ("NAME", item["name"]),
        ("DESCRIPTION", item["description"]),
    ])


def write_text(path: Path, text: str) -> None:
    path.parent.mkdir(parents=True, exist_ok=True)
    path.write_text(text, encoding="utf-8", newline="\n")


def build_item(item: dict, config: dict, texts: dict[str, list[dict]], names: list[dict], dll: Path, notices: str) -> int:
    lang = item["lang"]
    chars = sorted({c for rows in [*texts.values(), names] for r in rows for c in r[lang] if c not in BYTE_OF})
    out = DIST / item["folder"]
    try:
        if out.exists():
            shutil.rmtree(out)
    except PermissionError as e:
        print(f"FAIL {lang}: 無法清除 {out}（遊戲執行中會鎖住 dflang.dll，先關遊戲）：{e}", file=sys.stderr)
        return 1
    data = out / "dflang"
    data.mkdir(parents=True)

    # 標題畫面吉祥物的名牌（語言標籤、作者名）由引擎在遊戲裡用粗體子集畫（不跟著字重設定），字型子集也要有這些字（含英數）；
    # 整句貼圖把遊戲畫的英數、空白、CP437 的拉丁字母（矮人名字的 ë、ü…）、♂♀ 與譯名的間隔號 ·
    # 也用玩家選的字重畫，和引擎 overlay.rs 的 is_text 同一組。兩種字重用同一份字集，切換字重不會少字
    ascii = "".join(map(chr, range(0x20, 0x7F))) + "ÇüéâäàåçêëèïîìÄÅÉæÆôöòûùÿÖÜáíóúñÑ♂♀·"
    tag_chars = [c for c in dict.fromkeys(item["mascot_label"] + config["author"] + ascii) if c not in chars]
    for weight in WEIGHTS:
        missing = subset_fonts(chars + tag_chars, item["fonts"][weight], data / "fonts" / weight)
        if missing:
            print(f"FAIL {lang}: {weight} 沒有字型能畫 {''.join(missing)}", file=sys.stderr)
            return 1
    write_text(data / "glyphs.txt", "".join(chars) + "\n")
    for kind in KINDS:
        write_text(data / f"{kind}.tsv", dict_lines(kind, texts[kind], lang))
    write_text(data / "names.tsv", name_lines(names, lang))
    patterns = (ROOT / "engine/patterns.tsv").read_text(encoding="utf-8").splitlines()
    write_text(data / "patterns.tsv", "".join(f"{line}\n" for line in patterns if line and not line.startswith("#")))
    shutil.copy2(dll, data / "dflang.dll")
    write_text(data / "LICENSE.txt", PACKAGE_LICENSE + (ROOT / "LICENSE").read_text(encoding="utf-8"))
    write_text(data / "THIRD-PARTY-LICENSES.txt", notices)
    shutil.copy2(FONTS / "LICENSE", data / "LICENSE-fonts.txt")
    shutil.copy2(ROOT / "CREDITS.md", data / "CREDITS.md")
    shutil.copy2(ROOT / "art/mascot.png", data / "mascot.png")

    suffix = lang.lower().replace("-", "_")
    loader = ((ROOT / "lua/loader.lua").read_text(encoding="utf-8")
              .replace("@LANG@", lang).replace("@MOD_ID@", item["id"]).replace("@VERSION@", config["version"]["displayed"])
              .replace("@MASCOT_LABEL@", item["mascot_label"]).replace("@AUTHOR@", config["author"]))
    write_text(out / f"scripts_modinstalled/dflang_{suffix}.lua", loader)
    shutil.copy2(ROOT / "lua/dflang.lua", out / "scripts_modinstalled/dflang.lua")
    write_text(out / "info.txt", info_txt(item, config))
    counts = "、".join(f"{sum(1 for r in texts[kind] if r[lang].strip())} 條{name}"
                      for kind, name in zip(KINDS, ("譯文", "句型", "依畫面譯文")))
    print(f"{lang}: {counts}、{sum(1 for r in names if r[lang].strip())} 個名稱用字、{len(chars)} 個字 -> {out.relative_to(ROOT)}")
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
    notices = third_party_licenses()
    return max(build_item(item, config, texts, names, dll, notices) for item in config["items"])


if __name__ == "__main__":
    sys.exit(main())
