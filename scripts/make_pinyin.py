# /// script
# requires-python = ">=3.11"
# ///
"""拼音搜尋用的讀音表：data/ 譯文用到的每個漢字的普通話讀音，寫到 engine/pinyin.tsv（引擎編進 DLL，見 engine/src/pinyin.rs）。

讀音取自 Unicode Unihan 資料庫（Unihan_Readings.txt）的 kMandarin（最常用的讀音）、kTGHZ2013（《通用規範漢字字典》）、
kXHC1983（《現代漢語詞典》）三欄的聯集，多音字（行 xing／hang、長 chang／zhang）全收；三欄都沒有才用 kHanyuPinyin
（它收了很多罕用讀音，平常不用，免得搜尋多中不相干的項目）。聲調去掉，ü 寫成 v（引擎比對時 v 也當 u）。
Unihan 依 Unicode License V3 授權，全文在 engine/LICENSE-unicode.txt，建置時併進包裡的 THIRD-PARTY-LICENSES.txt。

改了譯文、用到新的漢字後重跑（verify_mod.py 檢查 data/ 用到的漢字都在表裡）：
  uv run scripts/make_pinyin.py        # Unihan.zip 快取在 temp/unihan/，沒有才下載
"""
import io
import sys
import unicodedata
import urllib.request
import zipfile
from pathlib import Path

ROOT = Path(__file__).resolve().parents[1]
OUT = ROOT / "engine/pinyin.tsv"
CACHE = ROOT / "temp/unihan/Unihan.zip"
URL = "https://www.unicode.org/Public/UCD/latest/ucd/Unihan.zip"
FIELDS = ("kMandarin", "kTGHZ2013", "kXHC1983")
FALLBACK = "kHanyuPinyin"
LANGS = ("zh-Hant", "zh-Hans")


def is_han(c: str) -> bool:
    return unicodedata.name(c, "").startswith("CJK UNIFIED IDEOGRAPH") or unicodedata.name(c, "").startswith("CJK COMPATIBILITY IDEOGRAPH")


def used_chars() -> set[str]:
    """data/ 底下所有 TSV 的 zh-Hant、zh-Hans 欄用到的漢字"""
    chars: set[str] = set()
    for path in sorted((ROOT / "data").rglob("*.tsv")):
        lines = path.read_text(encoding="utf-8").split("\n")
        header = lines[0].split("\t")
        cols = [header.index(lang) for lang in LANGS if lang in header]
        for line in lines[1:]:
            cells = line.split("\t")
            chars.update(c for i in cols if i < len(cells) for c in cells[i] if is_han(c))
    return chars


def toneless(reading: str) -> str:
    """tiě → tie、lǜ → lv：拆開附加符號，u 加分音符寫成 v，其他符號丟掉"""
    out = []
    for c in unicodedata.normalize("NFD", reading.lower()):
        if c == "\u0308" and out and out[-1] == "u":
            out[-1] = "v"
        elif c.isascii() and c.isalpha():
            out.append(c)
    return "".join(out)


def readings(path: Path) -> dict[str, list[str]]:
    """字 → 讀音（不帶聲調，常用的在前）。繁體字另外併上簡化字的讀音：兩部現代字典只收簡化字形，繁體字的多音常只剩一個
    （「調」只有 diao，「调」有 diao、tiao）"""
    if not path.is_file():
        path.parent.mkdir(parents=True, exist_ok=True)
        print(f"下載 {URL}")
        path.write_bytes(urllib.request.urlopen(URL, timeout=120).read())
    with zipfile.ZipFile(path) as z:
        text = io.TextIOWrapper(z.open("Unihan_Readings.txt"), encoding="utf-8").read()
        variants = io.TextIOWrapper(z.open("Unihan_Variants.txt"), encoding="utf-8").read()
    main: dict[str, list[str]] = {}
    fallback: dict[str, list[str]] = {}
    for line in text.splitlines():
        if line.startswith("#") or not line.strip():
            continue
        code, field, value = line.split("\t", 2)
        if field not in FIELDS and field != FALLBACK:
            continue
        char = chr(int(code[2:], 16))
        # kTGHZ2013、kXHC1983、kHanyuPinyin 的值是「頁碼.位置:讀音,讀音」，以空白分隔多筆；kMandarin 是空白分隔的讀音
        found = [toneless(r) for item in value.split() for r in item.rsplit(":", 1)[-1].split(",")]
        target = fallback if field == FALLBACK else main
        for r in found:
            if r and r not in target.setdefault(char, []):
                target[char].append(r)
    for line in variants.splitlines():
        if "\tkSimplifiedVariant\t" not in line:
            continue
        code, _, value = line.split("\t", 2)
        char = chr(int(code[2:], 16))
        for simple in (chr(int(v[2:], 16)) for v in value.split()):
            if simple != char:
                main.setdefault(char, []).extend(r for r in main.get(simple, []) if r not in main[char])
    return {c: main.get(c) or fallback.get(c, []) for c in set(main) | set(fallback)}


def main() -> int:
    table = readings(CACHE)
    chars = sorted(used_chars())
    missing = [c for c in chars if not table.get(c)]
    lines = ["# 字\t讀音（不帶聲調，ü 寫 v，常用的在前）。scripts/make_pinyin.py 從 Unicode Unihan 產生（Unicode License V3，見 engine/LICENSE-unicode.txt），別手改",
             *(f"{c}\t{' '.join(table[c])}" for c in chars if table.get(c))]
    if missing:
        lines.append(f"# 沒有讀音：{''.join(missing)}")
    OUT.write_text("\n".join(lines) + "\n", encoding="utf-8", newline="\n")
    print(f"{len(chars) - len(missing)} 字 -> {OUT.relative_to(ROOT)}" + (f"；沒有讀音 {len(missing)} 字：{''.join(missing)}" if missing else ""))
    return 0


if __name__ == "__main__":
    sys.exit(main())
