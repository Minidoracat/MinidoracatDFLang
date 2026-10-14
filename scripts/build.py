# /// script
# requires-python = ">=3.11"
# dependencies = ["fonttools>=4.50"]
# ///
"""組出繁、簡兩個工作坊包到 dist/<folder>/（內容就是上傳到工作坊的資料夾）。

每包內容：info.txt、scripts_modinstalled/（載入器與 dflang 指令）、dflang/（引擎 DLL、整句字典 strings.tsv、
句型規則 rules.tsv、字型子集、特徵碼、標題畫面吉祥物 mascot.png、授權與致謝）。字型是 workshop.json 指定的
思源黑體（Noto Sans CJK），只留本包譯文與吉祥物標籤（mascot_label）用到的非 CP437 字；主字型沒有的字依
fallback_fonts 依序補成第二、第三份字型，全部都沒有就建置失敗。
引擎在遊戲啟動時依當下的文字格大小把這些字畫成字圖集，所以字型檔就是字圖的唯一來源。

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
FONTS = ROOT / "temp/fonts"
FONT_LICENSE_URL = "https://raw.githubusercontent.com/notofonts/noto-cjk/main/Sans/LICENSE"
PACKAGE_LICENSE = """程式（dflang/dflang.dll、scripts_modinstalled/*.lua）：MIT License，全文附在下方。
譯文（dflang/strings.tsv、dflang/rules.tsv）：CC BY 4.0，https://creativecommons.org/licenses/by/4.0/
字型（dflang/fonts/）：SIL Open Font License 1.1，全文見 LICENSE-fonts.txt。
吉祥物圖（dflang/mascot.png）：© Minidoracat，保留所有權利，不適用上述授權。

"""


def load_config() -> dict:
    return json.loads((ROOT / "scripts/workshop.json").read_text(encoding="utf-8"))


def read_rows(folder: str) -> list[dict]:
    """data/<folder>/*.tsv 的所有列，附上來源檔名。strings 是整句字典，rules 是句型規則。"""
    rows = []
    for path in sorted((ROOT / "data" / folder).glob("*.tsv")):
        with path.open(encoding="utf-8", newline="") as fh:
            for row in csv.DictReader(fh, delimiter="\t", quoting=csv.QUOTE_NONE):
                rows.append({**row, "file": path.name})
    return rows


def read_strings() -> list[dict]:
    return read_rows("strings")


def read_rules() -> list[dict]:
    return read_rows("rules")


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


def build_item(item: dict, config: dict, rows: list[dict], rules: list[dict], dll: Path) -> int:
    lang = item["lang"]
    pairs = [(r["en"], r[lang], r["align"]) for r in rows if r[lang].strip()]
    rule_pairs = [(r["en"], r[lang], r["align"]) for r in rules if r[lang].strip()]
    chars = sorted({c for _, zh, _ in pairs + rule_pairs for c in zh if c not in BYTE_OF})
    out = DIST / item["folder"]
    try:
        if out.exists():
            shutil.rmtree(out)
    except PermissionError as e:
        print(f"FAIL {lang}: 無法清除 {out}（遊戲執行中會鎖住 dflang.dll，先關遊戲）：{e}", file=sys.stderr)
        return 1
    data = out / "dflang"
    data.mkdir(parents=True)

    # 標題畫面吉祥物的名牌（語言標籤、作者名）由引擎在遊戲裡用同一套字型畫，字型子集也要有這些字（含英數）
    tag_chars = [c for c in dict.fromkeys(item["mascot_label"] + config["author"]) if c != " " and c not in chars]
    missing = subset_fonts(chars + tag_chars, [item["font"], *item["fallback_fonts"]], data / "fonts")
    if missing:
        print(f"FAIL {lang}: 沒有字型能畫 {''.join(missing)}", file=sys.stderr)
        return 1
    write_text(data / "glyphs.txt", "".join(chars) + "\n")
    write_text(data / "strings.tsv", "".join(f"{en}\t{zh}\t{align}\n" for en, zh, align in pairs))
    write_text(data / "rules.tsv", "".join(f"{en}\t{zh}\t{align}\n" for en, zh, align in rule_pairs))
    patterns = (ROOT / "engine/patterns.tsv").read_text(encoding="utf-8").splitlines()
    write_text(data / "patterns.tsv", "".join(f"{line}\n" for line in patterns if line and not line.startswith("#")))
    shutil.copy2(dll, data / "dflang.dll")
    write_text(data / "LICENSE.txt", PACKAGE_LICENSE + (ROOT / "LICENSE").read_text(encoding="utf-8"))
    shutil.copy2(FONTS / "LICENSE", data / "LICENSE-fonts.txt")
    shutil.copy2(ROOT / "CREDITS.md", data / "CREDITS.md")
    shutil.copy2(ROOT / "art/mascot.png", data / "mascot.png")

    suffix = lang.lower().replace("-", "_")
    loader = ((ROOT / "lua/loader.lua").read_text(encoding="utf-8")
              .replace("@LANG@", lang).replace("@MOD_ID@", item["id"])
              .replace("@MASCOT_LABEL@", item["mascot_label"]).replace("@AUTHOR@", config["author"]))
    write_text(out / f"scripts_modinstalled/dflang_{suffix}.lua", loader)
    shutil.copy2(ROOT / "lua/dflang.lua", out / "scripts_modinstalled/dflang.lua")
    write_text(out / "info.txt", info_txt(item, config))
    print(f"{lang}: {len(pairs)} 條譯文、{len(rule_pairs)} 條句型、{len(chars)} 個字 -> {out.relative_to(ROOT)}")
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
    rows, rules = read_strings(), read_rules()
    return max(build_item(item, config, rows, rules, dll) for item in config["items"])


if __name__ == "__main__":
    sys.exit(main())
