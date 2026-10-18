# /// script
# requires-python = ">=3.11"
# dependencies = ["opencc-python-reimplemented>=0.1.7"]
# ///
"""術語與兩岸用語檢查（repo 根目錄：uv run scripts/terms.py check [--since REF]）。

check：
  1. glossary/terms.tsv 格式；同一英文術語（不分大小寫）同類別不重複
  2. 術語一致：原文含術語（整詞、可帶複數 s/es）時，譯文必須含該術語的譯名（多個譯名以 | 分隔，含其一即可）
     approved 不符 → FAIL；draft 不符 → WARN（draft 是代理提案，使用者確認後改 approved）
    同字不同義的列在 note 寫「術語例外：pet」（多個用、分隔），該列不檢查那個術語（動詞 pet 撫摸、嬰兒語 Bin...）；兩岸用語的 avoid 字詞也一樣（软体动物的「软体」）
     說明要用「；」和術語隔開（術語例外：guide；牽車，不是說明指南）
  3. 例外詞：note 的每個例外詞必須是 terms.tsv 的 en（不分大小寫）或 locale.tsv 的 avoid 字詞，否則 FAIL
     （寫成「術語例外：guide（牽車）」會把括號一起抓進去，例外靜靜失效、WARN 照報）
  4. 兩岸用語：glossary/locale.tsv 的 avoid 字詞出現在該語言譯文 → level=error 為 FAIL、warn 為 WARN
  5. 字形混用：繁中譯文含簡化字、簡中譯文含繁體字 → WARN（OpenCC 只拿來偵測，不產生或改寫譯文）
  --since REF：只列 REF 之後新增或改過的列（git diff REF -- data 的新增行，加上 data/ 底下未追蹤的新檔）的 FAIL／WARN，
     結束碼也只看這些；terms.tsv 格式照舊全查。術語表在 REF 之後有改時會提醒不加 --since 再跑一次（舊句子可能受影響）
impact <英文術語>：列出原文含該術語的條目，改譯名前先看會影響哪些句子
FAIL 時 exit 1；WARN 只列出待人工判斷。
"""
import csv
import re
import subprocess
import sys
from pathlib import Path

import opencc

from gen_events import OUTS as EVENT_FILES

ROOT = Path(__file__).resolve().parents[1]
LANGS = ("zh-Hant", "zh-Hans")


def read_tsv(path: Path) -> list[dict]:
    with path.open(encoding="utf-8", newline="") as fh:
        return list(csv.DictReader(fh, delimiter="\t", quoting=csv.QUOTE_NONE))


def strings() -> list[dict]:
    """整句字典與句型規則的所有列；file 欄是 strings/xxx.tsv 或 rules/xxx.tsv。"""
    rows = []
    for path in sorted((ROOT / "data").glob("*/*.tsv")):
        if path in EVENT_FILES:  # 由 data/events 產生（scripts/gen_events.py），查來源就好
            continue
        rows += [{**r, "file": f"{path.parent.name}/{path.name}"} for r in read_tsv(path)]
    return rows


def term_re(en: str) -> re.Pattern:
    return re.compile(rf"(?<![A-Za-z]){re.escape(en)}(?:e?s)?(?![A-Za-z])", re.I)


def exempt(row: dict) -> set[str]:
    """note 裡「術語例外：a、b」列出的術語（小寫）"""
    return {t.strip().lower() for m in re.findall(r"術語例外：([^；;。]+)", row.get("note") or "") for t in m.split("、")}


def git(*args: str) -> list[str]:
    return subprocess.run(["git", "-c", "core.quotepath=off", *args], cwd=ROOT, capture_output=True,
                          encoding="utf-8", check=True).stdout.splitlines()


def file_key(path: str) -> str:
    """repo 相對路徑 → strings() 的 file 欄（data/rules/x.tsv → rules/x.tsv）"""
    p = Path(path)
    return f"{p.parent.name}/{p.name}"


def changed_since(ref: str) -> tuple[set[tuple[str, str]], set[str]]:
    """REF 之後新增或改過的列 {(file, en)}，和 data/ 底下未追蹤的新檔 {file}"""
    changed, path = set(), None
    for line in git("diff", "-U0", ref, "--", "data"):
        if line.startswith("+++ "):
            path = line[6:] if line.startswith("+++ b/") and line.endswith(".tsv") else None  # +++ /dev/null：刪檔
            if path:  # 名稱字表第一欄是 word，en 欄位置照表頭找
                with (ROOT / path).open(encoding="utf-8") as fh:
                    col = fh.readline().rstrip("\r\n").split("\t").index("en")
        elif line.startswith("+") and path:
            changed.add((file_key(path), line[1:].split("\t")[col]))
    return changed, {file_key(p) for p in git("ls-files", "--others", "--exclude-standard", "--", "data")}


def check(since: str | None = None) -> int:
    terms = read_tsv(ROOT / "glossary/terms.tsv")
    locale = read_tsv(ROOT / "glossary/locale.tsv")
    rows = strings()
    fails, warns = [], []  # (列, 訊息)；列是 None 表示 terms.tsv 本身的格式問題

    seen = set()
    for t in terms:
        key = (t["en"].lower(), t["category"])
        if not t["en"] or not t["zh-Hant"] or not t["zh-Hans"] or t["status"] not in ("approved", "draft"):
            fails.append((None, f"terms.tsv: 欄位不完整或 status 不是 approved/draft：{t}"))
        if key in seen:
            fails.append((None, f"terms.tsv: 重複術語 {t['en']}（{t['category']}）"))
        seen.add(key)

    known = {t["en"].lower() for t in terms} | {rule["avoid"].lower() for rule in locale}
    for r in rows:
        for word in sorted(exempt(r) - known):
            fails.append((r, f"{r['file']} {r['en']!r} 術語例外「{word}」不是術語也不是兩岸用語的 avoid 字詞"
                             f"（說明要用「；」和術語隔開）：{r['note']}"))

    # 世界生成名稱的字表（data/names）是組名字用的詞素（TEMPLE 太陽穴、TAME 馴），名字音譯表（data/translit）是音節，不套介面術語；
    # 兩岸用語與字形照查
    for t in terms:
        pattern = term_re(t["en"])
        for r in rows:
            if r["file"].startswith(("names/", "translit/")) or not pattern.search(r["en"]) or t["en"].lower() in exempt(r):
                continue
            for lang in LANGS:
                text = r[lang]
                if text and not any(alt in text for alt in t[lang].split("|")):
                    msg = f"{r['file']} {r['en']!r} [{lang}] 應含「{t[lang]}」（術語 {t['en']}）：{text}"
                    (fails if t["status"] == "approved" else warns).append((r, msg))

    for rule in locale:
        for r in rows:
            text = r[rule["lang"]]
            if text and rule["avoid"] in text and rule["avoid"].lower() not in exempt(r):
                msg = f"{r['file']} {r['en']!r} [{rule['lang']}] 用了「{rule['avoid']}」，建議「{rule['use']}」：{text}"
                (fails if rule["level"] == "error" else warns).append((r, msg))

    to_trad, to_simp = opencc.OpenCC("s2twp"), opencc.OpenCC("t2s")
    for r in rows:
        if r["zh-Hant"] and (fixed := to_trad.convert(r["zh-Hant"])) != r["zh-Hant"]:
            warns.append((r, f"{r['file']} {r['en']!r} [zh-Hant] 可能有簡化字或大陸詞（OpenCC s2twp 會改成「{fixed}」）：{r['zh-Hant']}"))
        if r["zh-Hans"] and (fixed := to_simp.convert(r["zh-Hans"])) != r["zh-Hans"]:
            warns.append((r, f"{r['file']} {r['en']!r} [zh-Hans] 可能有繁體字（OpenCC t2s 會改成「{fixed}」）：{r['zh-Hans']}"))

    scope = ""
    if since:
        changed, new_files = changed_since(since)
        picked = {id(r) for r in rows if r["file"] in new_files or (r["file"], r["en"]) in changed}
        fails = [(r, m) for r, m in fails if r is None or id(r) in picked]
        warns = [(r, m) for r, m in warns if id(r) in picked]
        scope = f"（--since {since}：只列 {len(picked)} 條新增或改過的列）"

    for _, m in fails:
        print("FAIL", m)
    for _, m in warns:
        print("WARN", m)
    if since and (git("diff", "--name-only", since, "--", "glossary") or git("ls-files", "--others", "--exclude-standard", "--", "glossary")):
        print(f"術語表也改了（{since} 之後 glossary/ 有變動）：不加 --since 再跑一次，舊句子可能受影響")
    print(f"{scope}{len(rows)} 條譯文、{len(terms)} 個術語、{len(locale)} 條用語規則：{len(fails)} FAIL、{len(warns)} WARN")
    return 1 if fails else 0


def impact(en: str) -> int:
    pattern = term_re(en)
    hits = [r for r in strings() if pattern.search(r["en"])]
    for r in hits:
        print(f"{r['file']}\t{r['en']}\t{r['zh-Hant']}\t{r['zh-Hans']}")
    print(f"{len(hits)} 條原文含「{en}」")
    return 0


def main() -> int:
    if sys.argv[1:] == ["check"]:
        return check()
    if len(sys.argv) == 4 and sys.argv[1:3] == ["check", "--since"]:
        return check(sys.argv[3])
    if len(sys.argv) >= 3 and sys.argv[1] == "impact":
        return impact(" ".join(sys.argv[2:]))
    print(__doc__)
    return 2


if __name__ == "__main__":
    sys.exit(main())
