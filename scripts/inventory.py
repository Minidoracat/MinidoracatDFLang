# /// script
# requires-python = ">=3.11"
# ///
"""全文盤點：把反編譯快照裡的遊戲文字分類，對照已翻譯與實機看過的字串，排出待翻清單。

用法（repo 根目錄）：uv run scripts/inventory.py

輸入
- 最新快照的 exe/strings.tsv（執行檔字串）與 raws_text.tsv（raws、TEXT_SET、程序生成 Lua）
- 實機未翻譯紀錄：temp/misses/*.tsv（dev_game.py down 另存）與遊戲目錄 dfhack-config/dflang-misses.tsv
- 已翻譯：data/strings（整句，原文完全相同）、data/screens（依畫面的整句）與 data/rules（句型，整句符合樣板）

輸出（temp/inventory/，不進 git：內容是遊戲原文）
- summary.txt：各類的數量、已翻、實機看過
- todo.tsv：待翻清單，欄位 priority, category, seen, text, context（exe 的 RVA 或 raws 物件）。
  依 priority 排：0 實機看過 → 1 介面標籤 → 2 介面句子 → 3 生物物品名稱 → 4 片段與單字 → 5 描述與文字範本 → 6 其餘。
  同一優先序內照執行檔位址或 raws 檔案順序，同一畫面的字串通常相鄰。
- composed.tsv：實機看過、但執行檔與 raws 都沒有整句的字串（執行時拼出來的），要寫句型規則
- seen.tsv：實機看過、還沒翻譯的字串，照第一次看到的順序（同一畫面的字串相鄰），分輪翻譯從這裡取
分類只是排序與翻法的提示：「片段」也可能被遊戲單獨畫出（例如 "Range: "、" to "），
實機看過的就照整句翻；沒看過的先翻標籤與句子。
"""
import csv
import re
import sys
from collections import Counter
from pathlib import Path

from dfpaths import DF, latest_snapshot

ROOT = Path(__file__).resolve().parents[1]
OUT = ROOT / "temp/inventory"
PLACEHOLDER = re.compile(r"\\\{(\\\#|@|%)?([1-9])\\\}")  # 已經 re.escape 過的 {1}／{#1}／{@1}／{%1}（{%1} 盤點時當任意片段）
CODE = re.compile(r"::|%[-+ #0-9.]*[sdifxXcpul]|\\|/|\.(txt|png|lua|dll|ttf|otf|wav|ogg|mp3|json|ini|exe|bmp)\b|\b0x|\(\)"
                  r"|^[A-Za-z]+_[A-Za-z0-9_]+$|^[a-z]+[A-Z][A-Za-z]*$|^[A-Z][a-z]+[A-Z][A-Za-z]*$")
# 執行期錯誤、除錯訊息：玩家幾乎看不到，排最後
DEBUG = re.compile(r"\b(exception|assert(ion)?|null|debug|abort(ing)?|fatal|overflow|out of memory|bad (alloc|optional|function|cast)|"
                   r"out of range|invalid argument|not implemented|unknown error|failed to|could not (open|load|create)|SDL|OpenGL|FMOD)\b", re.I)
MARKUP = re.compile(r"\[(C:\d|B\]|R\]|P\]|CHAR:|LPAGE|HL:|IHL:|VAR:)")
# raws：這些 token 的值不是顯示文字（內部編輯指令、反應材料、語言翻譯詞）
RAWS_SKIP = {"T_WORD", "REAGENT", "PRODUCT", "PRODUCT_TOKEN", "CVCT_TARGET", "CVCT_REPLACEMENT", "CV_NEW_TAG", "ATTACK"}
RAWS_NAMES = {"NAME", "CASTE_NAME", "NAME_PLURAL", "GENERAL_CHILD_NAME", "STATE_NAME", "STATE_NAME_ADJ", "STATE_ADJ",
              "GROWTH_NAME", "TISSUE_NAME", "PROFESSION_NAME", "MEAT_NAME", "SEED", "BP", "ALL_NAMES", "IS_GEM",
              "TLCM_NOUN", "APP_MOD_NOUN", "INDIVIDUAL_NAME", "SYN_NAME"}
RAWS_PROSE = {"DESCRIPTION", "TEXT", "PREFSTRING", "LUA"}
PRIORITY = {"label": 1, "sentence": 2, "raws-name": 3, "fragment": 4, "word": 4, "raws-prose": 5, "debug": 7}


def read_tsv(path: Path) -> list[dict]:
    with path.open(encoding="utf-8", newline="") as fh:
        return list(csv.DictReader(fh, delimiter="\t", quoting=csv.QUOTE_NONE))


def classify(text: str) -> str:
    letters = sum(c.isalpha() for c in text)
    if letters < 2 or letters < len(text.strip()) / 2 or not re.search(r"[aeiouyAEIOUY]", text):
        return "noise"
    if CODE.search(text) or re.fullmatch(r"[A-Z0-9_ ]+", text):
        return "code"
    if DEBUG.search(text):
        return "debug"
    if MARKUP.search(text):
        return "markup"
    first = text[0]
    if first == " " or text.endswith(" ") and not text.endswith(": ") or not (first.isupper() or first in "\"'"):
        return "word" if re.fullmatch(r"[a-z][a-z' -]*", text) else "fragment"
    if len(text.split()) >= 6 or text.rstrip().endswith((".", "!", "?")):
        return "sentence"
    return "label"


def raws_parts(row: dict) -> list[str]:
    """一列 raws 的顯示文字：值以冒號分隔，留下含小寫字母、不是純 token 的部分。"""
    if row["token"] == "LUA" or row["token"] == "TEXT" or row["token"] == "DESCRIPTION":
        return [row["text"]]
    return [p for p in row["text"].split(":") if re.search(r"[a-z]", p) and not re.fullmatch(r"[A-Z0-9_]+", p)]


def main() -> int:
    snap = latest_snapshot()
    if snap is None:
        print("沒有反編譯快照；先跑 uv run scripts/df_snapshot.py snapshot", file=sys.stderr)
        return 1

    words = {r["en"] for p in sorted((ROOT / "data/strings").glob("*.tsv")) for r in read_tsv(p)}
    done = words | {r["en"] for p in sorted((ROOT / "data/screens").glob("*.tsv")) for r in read_tsv(p)}
    kinds = {"\\#": r"(-?[\d,]+)"}  # {#1} 數字；{@1} 先當任意片段，比對後再查字典
    rules = []
    for p in sorted((ROOT / "data/rules").glob("*.tsv")):
        for r in read_tsv(p):
            pattern = re.escape(r["en"])
            at = [i + 1 for i, m in enumerate(PLACEHOLDER.finditer(pattern)) if m.group(1) == "@"]
            rules.append((re.compile(PLACEHOLDER.sub(lambda m: kinds.get(m.group(1), "(.+?)"), pattern) + r"\Z"), at))

    def named(text: str) -> bool:
        """引擎的整句查詞與 {@n}：查整句字典，含大寫時退回全部小寫的寫法（全大寫的不退）"""
        return text in words or text != text.lower() and text != text.upper() and text.lower() in words

    def translated(text: str) -> bool:
        return text in done or named(text) or any(
            (m := rule.match(text)) and all(named(m.group(i)) for i in at) for rule, at in rules)

    # 實機紀錄：依時間順序保留第一次看到的樣子（同一畫面的字串會相鄰）。沒翻譯的段落整段記一筆（mtb），
    # 段落裡的單字由引擎照畫英文、不記
    logs = [log for log in [*sorted((ROOT / "temp/misses").glob("*.tsv")), DF / "dfhack-config/dflang-misses.tsv"] if log.is_file()]
    seen: Counter[str] = Counter()
    order: dict[str, str] = {}  # 原文 → 第一次攔到的函式
    for log in logs:
        session = set()
        for fn, text in (line.split("\t", 1) for line in log.read_text(encoding="utf-8").splitlines() if "\t" in line):
            order.setdefault(text, fn)
            session.add(text)
        seen.update(session)

    items: dict[str, tuple[str, str]] = {}  # 原文 → (分類, 出處)；同一句只算一次，第一次出現的出處
    for r in read_tsv(snap / "exe/strings.tsv"):
        if r["kind"] == "text" and r["text"] not in items:
            items[r["text"]] = (classify(r["text"]), r["rva"])
    for r in read_tsv(snap / "raws_text.tsv"):
        if r["token"] in RAWS_SKIP:
            continue
        word = r["object"].startswith("WORD:")
        kind = "language" if word else "raws-name" if r["token"] in RAWS_NAMES else "raws-prose" if r["token"] in RAWS_PROSE else "raws-other"
        for part in raws_parts(r):
            items.setdefault(part, (kind, f"{r['object'] or r['file']} {r['token']}"))

    stats: dict[str, Counter] = {}
    todo = []
    for text, (kind, context) in items.items():
        st = stats.setdefault(kind, Counter())
        st["total"] += 1
        st["seen"] += text in seen
        if translated(text):
            st["done"] += 1
            continue
        # 程式字串的判斷會誤抓 Pets/Livestock 這類含斜線的標籤；實機畫出來過的就是顯示文字，照樣列入
        if kind == "noise" or (kind == "code" and text not in seen):
            continue
        priority = 0 if text in seen else PRIORITY.get(kind, 6)
        todo.append((priority, kind, int(text in seen), text, context))
    todo.sort(key=lambda t: t[0])  # sort 是穩定的：同優先序保留原本的位址／檔案順序
    composed = sorted((n, t) for t, n in seen.items() if t not in items and not translated(t))

    OUT.mkdir(parents=True, exist_ok=True)
    with (OUT / "todo.tsv").open("w", encoding="utf-8", newline="\n") as fh:
        fh.write("priority\tcategory\tseen\ttext\tcontext\n")
        fh.writelines(f"{p}\t{k}\t{s}\t{t}\t{c}\n" for p, k, s, t, c in todo)
    with (OUT / "composed.tsv").open("w", encoding="utf-8", newline="\n") as fh:
        fh.write("sessions\ttext\n")
        fh.writelines(f"{n}\t{t}\n" for n, t in sorted(composed, key=lambda x: -x[0]))
    # 實機看過、還沒翻譯的字串，照第一次看到的順序（同一畫面相鄰），一輪一輪分批翻用
    with (OUT / "seen.tsv").open("w", encoding="utf-8", newline="\n") as fh:
        fh.write("function\tsource\ttext\n")
        fh.writelines(f"{fn}\t{'exe/raws' if t in items else 'composed'}\t{t}\n"
                      for t, fn in order.items() if not translated(t) and items.get(t, ("",))[0] != "noise")
    # 只靠大小寫退回翻到的執行檔字串：介面標籤剛好和 raws 名稱同字，意思可能不同（Back 返回、back 後部），要逐條確認
    with (OUT / "fallback.tsv").open("w", encoding="utf-8", newline="\n") as fh:
        fh.write("text\tname\n")
        fh.writelines(f"{t}\t{t.lower()}\n" for t, (kind, _) in items.items()
                      if kind not in ("raws-name", "language", "raws-prose", "raws-other") and t not in done and named(t))
    lines = [f"快照 {snap.name}；實機紀錄 {len(logs)} 份、{len(seen)} 種字串",
             f"{'分類':<12}{'總數':>8}{'已翻':>8}{'實機看過':>10}"]
    for kind, st in sorted(stats.items(), key=lambda kv: -kv[1]["total"]):
        lines.append(f"{kind:<12}{st['total']:>8}{st['done']:>8}{st['seen']:>10}")
    lines.append(f"待翻 {len(todo)} 條（不含 noise、code）；執行時拼出的字串 {len(composed)} 種 → temp/inventory/")
    (OUT / "summary.txt").write_text("\n".join(lines) + "\n", encoding="utf-8", newline="\n")
    print("\n".join(lines))
    return 0


if __name__ == "__main__":
    sys.exit(main())
