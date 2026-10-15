# /// script
# requires-python = ">=3.11"
# dependencies = ["opencc-python-reimplemented>=0.1.7"]
# ///
"""術語與兩岸用語檢查（repo 根目錄：uv run scripts/terms.py check）。

check：
  1. glossary/terms.tsv 格式；同一英文術語（不分大小寫）同類別不重複
  2. 術語一致：原文含術語（整詞、可帶複數 s/es）時，譯文必須含該術語的譯名（多個譯名以 | 分隔，含其一即可）
     approved 不符 → FAIL；draft 不符 → WARN（draft 是代理提案，使用者確認後改 approved）
    同字不同義的列在 note 寫「術語例外：pet」（多個用、分隔），該列不檢查那個術語（動詞 pet 撫摸、嬰兒語 Bin...）；兩岸用語的 avoid 字詞也一樣（软体动物的「软体」）
  3. 兩岸用語：glossary/locale.tsv 的 avoid 字詞出現在該語言譯文 → level=error 為 FAIL、warn 為 WARN
  4. 字形混用：繁中譯文含簡化字、簡中譯文含繁體字 → WARN（OpenCC 只拿來偵測，不產生或改寫譯文）
impact <英文術語>：列出原文含該術語的條目，改譯名前先看會影響哪些句子
FAIL 時 exit 1；WARN 只列出待人工判斷。
"""
import csv
import re
import sys
from pathlib import Path

import opencc

ROOT = Path(__file__).resolve().parents[1]
LANGS = ("zh-Hant", "zh-Hans")


def read_tsv(path: Path) -> list[dict]:
    with path.open(encoding="utf-8", newline="") as fh:
        return list(csv.DictReader(fh, delimiter="\t", quoting=csv.QUOTE_NONE))


def strings() -> list[dict]:
    """整句字典與句型規則的所有列；file 欄是 strings/xxx.tsv 或 rules/xxx.tsv。"""
    rows = []
    for path in sorted((ROOT / "data").glob("*/*.tsv")):
        rows += [{**r, "file": f"{path.parent.name}/{path.name}"} for r in read_tsv(path)]
    return rows


def term_re(en: str) -> re.Pattern:
    return re.compile(rf"(?<![A-Za-z]){re.escape(en)}(?:e?s)?(?![A-Za-z])", re.I)


def exempt(row: dict) -> set[str]:
    """note 裡「術語例外：a、b」列出的術語（小寫）"""
    return {t.strip().lower() for m in re.findall(r"術語例外：([^；;。]+)", row.get("note") or "") for t in m.split("、")}


def check() -> int:
    terms = read_tsv(ROOT / "glossary/terms.tsv")
    locale = read_tsv(ROOT / "glossary/locale.tsv")
    rows = strings()
    fails, warns = [], []

    seen = set()
    for t in terms:
        key = (t["en"].lower(), t["category"])
        if not t["en"] or not t["zh-Hant"] or not t["zh-Hans"] or t["status"] not in ("approved", "draft"):
            fails.append(f"terms.tsv: 欄位不完整或 status 不是 approved/draft：{t}")
        if key in seen:
            fails.append(f"terms.tsv: 重複術語 {t['en']}（{t['category']}）")
        seen.add(key)

    # 世界生成名稱的字表（data/names）是組名字用的詞素（TEMPLE 太陽穴、TAME 馴），不套介面術語；兩岸用語與字形照查
    for t in terms:
        pattern = term_re(t["en"])
        for r in rows:
            if r["file"].startswith("names/") or not pattern.search(r["en"]) or t["en"].lower() in exempt(r):
                continue
            for lang in LANGS:
                text = r[lang]
                if text and not any(alt in text for alt in t[lang].split("|")):
                    msg = f"{r['file']} {r['en']!r} [{lang}] 應含「{t[lang]}」（術語 {t['en']}）：{text}"
                    (fails if t["status"] == "approved" else warns).append(msg)

    for rule in locale:
        for r in rows:
            text = r[rule["lang"]]
            if text and rule["avoid"] in text and rule["avoid"].lower() not in exempt(r):
                msg = f"{r['file']} {r['en']!r} [{rule['lang']}] 用了「{rule['avoid']}」，建議「{rule['use']}」：{text}"
                (fails if rule["level"] == "error" else warns).append(msg)

    to_trad, to_simp = opencc.OpenCC("s2twp"), opencc.OpenCC("t2s")
    for r in rows:
        if r["zh-Hant"] and (fixed := to_trad.convert(r["zh-Hant"])) != r["zh-Hant"]:
            warns.append(f"{r['file']} {r['en']!r} [zh-Hant] 可能有簡化字或大陸詞（OpenCC s2twp 會改成「{fixed}」）：{r['zh-Hant']}")
        if r["zh-Hans"] and (fixed := to_simp.convert(r["zh-Hans"])) != r["zh-Hans"]:
            warns.append(f"{r['file']} {r['en']!r} [zh-Hans] 可能有繁體字（OpenCC t2s 會改成「{fixed}」）：{r['zh-Hans']}")

    for m in fails:
        print("FAIL", m)
    for m in warns:
        print("WARN", m)
    print(f"{len(rows)} 條譯文、{len(terms)} 個術語、{len(locale)} 條用語規則：{len(fails)} FAIL、{len(warns)} WARN")
    return 1 if fails else 0


def impact(en: str) -> int:
    pattern = term_re(en)
    hits = [r for r in strings() if pattern.search(r["en"])]
    for r in hits:
        print(f"{r['file']}\t{r['en']}\t{r['zh-Hant']}\t{r['zh-Hans']}")
    print(f"{len(hits)} 條原文含「{en}」")
    return 0


def main() -> int:
    if len(sys.argv) >= 2 and sys.argv[1] == "check":
        return check()
    if len(sys.argv) >= 3 and sys.argv[1] == "impact":
        return impact(" ".join(sys.argv[2:]))
    print(__doc__)
    return 2


if __name__ == "__main__":
    sys.exit(main())
