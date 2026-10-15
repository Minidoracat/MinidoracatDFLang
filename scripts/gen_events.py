"""傳說模式的歷史事件句：把 data/events/*.tsv 編成 data/rules/_events.tsv 與 zz_events.tsv。改了 data/events 之後重跑：

    uv run scripts/gen_events.py

同一類句型照檔案順序比對、先寫的優先（引擎 dict.rs 的 load_rules_tsv）。事件句分很多檔寫，又要和既有的規則檔共存：
- 事件句之間：一檔的通用子句（`{1} destroyed {2}.`）會搶走另一檔的專門子句（`{1} was destroyed by {2}.`），
  所以合成一份，比較通用的（樣板套得上另一條的原文）排在專門的後面，其餘照原順序。
- 和既有規則檔之間：事件句多半比既有的通用外框（死因的 `In {#1}, {2} {%3} in {4}.`）專門，放在最前面的 _events.tsv；
  比既有某條通用的（`{@1} by {2}` 對想法的 `being attacked by {1}`）放在最後面的 zz_events.tsv，讓既有的先比。

data/events/ 的欄位同 data/rules：
- common.tsv：共用句型（連結人名清單）。
- 其他：各種事件的子句與短語。align 寫 inner、以句號結尾的是完整句去掉日期的子句（`{1} settled in {2}.`），只當樣板：
  這裡替它產生帶日期的整句（In 3, …／In the midsummer of 3, …／In a time before time, …），子句本身不收；
  沒有句號的 inner 是短語（the founding of {1}），照收並加上結尾的日期（… in 3）。其他照抄。
  事件的原因（for an insurrection…）由引擎在句尾去掉再翻（align 寫 tail）。
"""
import csv
import heapq
import re
import sys
from pathlib import Path

ROOT = Path(__file__).resolve().parent.parent
SRC = ROOT / "data/events"
RULES = ROOT / "data/rules"
EARLY = RULES / "_events.tsv"
LATE = RULES / "zz_events.tsv"
OUTS = (EARLY, LATE)
HEADER = ["en", "zh-Hant", "zh-Hans", "align", "note"]
PH = re.compile(r"\{([#@%~^]?)([1-9])((?:\+[^{}]+)?)\}")

# 完整句開頭的日期（FUN_140b92640 寫年份、季節或 a time before time）：原文前綴、繁中、簡中、前綴用掉幾個佔位
DATES = [
    ("In {#1}, ", "{1}年，", "{1}年，", 1),
    ("In the {@1} of {#2}, ", "{2}年{1}，", "{2}年{1}，", 2),
    ("In a time before time, ", "太古之時，", "太古之时，", 0),
]
# 審訊紀錄裡的陰謀把開頭的 In 換成 Subject revealed that in（agreement_formed 的 FUN_1402aaf70，pseudo-C L380005）
REVEALED_FN = "FUN_1402aaf70"
REVEALED = [(en.replace("In ", "Subject revealed that in ", 1), "受審者透露，" + h, "受审者透露，" + s, n) for en, h, s, n in DATES]


def read(path: Path) -> list[dict]:
    with path.open(encoding="utf-8", newline="") as fh:
        return [{**r, "file": path.name} for r in csv.DictReader(fh, delimiter="\t", quoting=csv.QUOTE_NONE)]


def shift(text: str, n: int) -> str:
    return PH.sub(lambda m: "{%s%d%s}" % (m.group(1), int(m.group(2)) + n, m.group(3)), text)


def top(text: str) -> int:
    return max((int(n) for n in re.findall(r"\{[#@%~^]?(\d+)", text)), default=0)


class Shapes:
    """一批句型的樣板形狀，用來判斷誰比較通用：A 的樣板套得上 B 的原文（佔位當成任意片段，{#n} 只對數字）時 A 比 B 通用。"""

    def __init__(self, rows: list[dict]):
        parts = [re.split(r"(\{[#@%~^]?[1-9]\})", r["en"]) for r in rows]
        # 原文樣板：數字佔位寫成 \1，其他佔位寫成 \0
        self.tpl = [re.sub(r"\{#[1-9]\}", "\1", re.sub(r"\{[@%~^]?[1-9]\}", "\0", r["en"])) for r in rows]
        self.rx = [re.compile("".join((r"(?:\x01|\d[\d,]*)" if p.startswith("{#") else "(?s:.+?)") if k % 2 else re.escape(p)
                                      for k, p in enumerate(ps))) for ps in parts]
        self.words = [set(re.findall(r"[A-Za-z]+", "".join(ps[::2]))) for ps in parts]
        self.holders: dict[str, set[int]] = {}
        for i, ws in enumerate(self.words):
            for w in ws:
                self.holders.setdefault(w, set()).add(i)

    def candidates(self, words: set[str]):
        """含這些固定字的句型（從最少見的字找起）；沒有固定字的樣板比全部。"""
        return min((self.holders.get(w, set()) for w in words), key=len) if words else range(len(self.tpl))


def broader(a: Shapes, i: int, b: Shapes, j: int) -> bool:
    """a 的第 i 條比 b 的第 j 條通用：套得上對方的原文，對方套不上自己的。"""
    return bool(a.rx[i].fullmatch(b.tpl[j])) and not b.rx[j].fullmatch(a.tpl[i])


def specific_first(rows: list[dict]) -> list[dict]:
    """照原順序，但比較通用的（`{1} destroyed {2}.` 對 `{1} was destroyed by {2}.`）排在專門的後面：
    專門的往前提到通用的那條前面，通用的不往後挪（短語結尾的日期 `{%1} in {#2}` 要留在各種短語前面）。"""
    s = Shapes(rows)
    later = [[] for _ in rows]  # later[b]：要排在 b 後面的較通用句型
    waiting = [0] * len(rows)
    for a in range(len(rows)):
        for b in s.candidates(s.words[a]):
            if a != b and broader(s, a, s, b):
                later[b].append(a)
                waiting[a] += 1
    # 位置：自己與所有要排在自己後面的句型裡最前的原位置
    place = list(range(len(rows)))
    for i in sorted(range(len(rows)), key=lambda i: -len(later[i])):
        stack = list(later[i])
        seen = set()
        while stack:
            a = stack.pop()
            if a not in seen:
                seen.add(a)
                place[i] = min(place[i], a)
                stack.extend(later[a])
    ready = [(place[i], i) for i in range(len(rows)) if not waiting[i]]
    heapq.heapify(ready)
    order = []
    while ready:
        _, i = heapq.heappop(ready)
        order.append(i)
        for a in later[i]:
            waiting[a] -= 1
            if not waiting[a]:
                heapq.heappush(ready, (place[a], a))
    order += [i for i in range(len(rows)) if waiting[i]]  # 互相等待（不該發生）照原順序放最後
    return [rows[i] for i in order]


def build() -> tuple[dict[Path, str], list[str]]:
    """回傳 ({輸出檔: 內容}, 略過的條目說明)。"""
    common = read(SRC / "common.tsv")
    sources = [r for p in sorted(SRC.glob("*.tsv")) if p.name != "common.tsv" for r in read(p)]
    dated = []
    for c in (r for r in sources if r["align"] == "inner" and r["en"].endswith(".")):
        note = f"自動產生（scripts/gen_events.py）：{c['file']} {c['en']}"
        for en, hant, hans, n in DATES + (REVEALED if REVEALED_FN in c["note"] else []):
            dated.append({"en": en + shift(c["en"], n), "zh-Hant": hant + shift(c["zh-Hant"], n), "zh-Hans": hans + shift(c["zh-Hans"], n),
                          "align": "", "note": note})
    # 短語（沒有句號的 inner）結尾的日期：the founding of X in 3 →「3年X的建立」。寫成每條短語自己的句型，
    # 不用通用的 `{%1} in {#2}`：通用的會先吃掉外層（the story of … in 3）的日期，輪不到日期寫在句型裡的
    for c in (r for r in sources if r["align"] == "inner" and not r["en"].endswith(".")):
        k, note = top(c["en"]), f"自動產生（scripts/gen_events.py）：{c['file']} {c['en']} 加上結尾的日期"
        for en, zh in ((f" in {{#{k + 1}}}", f"{{{k + 1}}}年"), (f" in the {{@{k + 1}}} of {{#{k + 2}}}", f"{{{k + 2}}}年{{{k + 1}}}"),
                       (" in a time before time", "太古之時")):
            dated.append({"en": c["en"] + en, "zh-Hant": zh + c["zh-Hant"], "zh-Hans": zh.replace("時", "时") + c["zh-Hans"],
                          "align": "inner", "note": note})

    # 子句只當樣板：遊戲的事件句一定帶日期，去掉日期的子句收進規則檔只會在佔位裡搶走不相干的句子
    # （介面的「(Vehicle moved to next stop…)」被 `{1} moved to {2}.` 翻成半中半英）
    # 其他規則檔已有同一條原文時不收（verify_mod 要求同一類不重複）
    others = [r for p in sorted(RULES.glob("*.tsv")) if p not in OUTS for r in read(p)]
    key = lambda r: (r["en"], r["align"] == "tail")  # tail 片語自成一類（引擎另外存）
    taken = {key(r) for r in others}
    seen, rows, skipped = set(), [], []
    for r in [r for r in sources if not (r["align"] == "inner" and r["en"].endswith("."))] + dated:
        if key(r) in taken or key(r) in seen:
            skipped.append(f"{r.get('file', '產生')}: {r['en']}（{'其他規則檔已有' if key(r) in taken else '重複'}）")
        elif top(r["en"]) > 9:
            skipped.append(f"{r.get('file', '產生')}: {r['en']}（佔位超過 9 個）")
        else:
            seen.add(key(r))
            rows.append(r)

    # 比其他規則檔的某條通用的（`{@1} by {2}` 對想法的 `being attacked by {1}`）放後面的檔，讓那一條先比。
    # 帶日期的整句都放前面：死因的外框（`In {#1}, {2} {%3}.`）會把事件句的動詞切進 {2}（「X purchased標準裝備」）
    # inner 短語只在佔位裡比，對方是以句號結尾的整句（冒險日誌的 `It was last seen held by {1}.`）時不會互搶，不算
    mine, theirs = Shapes(rows), Shapes([r for r in others if r["align"] != "tail"])
    late = {i for i, r in enumerate(rows) if r["align"] != "tail" and any(
        broader(mine, i, theirs, j) for j in theirs.candidates(mine.words[i])
        if not (r["align"] == "inner" and theirs.tpl[j].endswith(".")))}
    files = {EARLY: specific_first(common + [r for i, r in enumerate(rows) if i not in late]),
             LATE: specific_first([r for i, r in enumerate(rows) if i in late])}
    return {p: "\n".join("\t".join(r[h] for h in HEADER) for r in [dict(zip(HEADER, HEADER))] + rs) + "\n" for p, rs in files.items()}, skipped


def main() -> int:
    texts, skipped = build()
    for path, text in texts.items():
        path.write_bytes(text.encode("utf-8"))
        print(f"{text.count(chr(10)) - 1} 條 -> {path.relative_to(ROOT).as_posix()}")
    for s in skipped:
        print("略過：", s)
    return 0


if __name__ == "__main__":
    sys.exit(main())
