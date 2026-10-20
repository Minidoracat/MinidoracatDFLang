# /// script
# requires-python = ">=3.11"
# ///
"""批次跑離線工具 t.exe（engine/examples/t.rs），輸出 TSV（repo 根目錄）。

  uv run scripts/t.py [-w|-m|-s] [--lang zh-Hant|zh-Hans] [--miss] [--max-bytes N] [FILE ...]
  printf 'He feels joy.\\nShe is sad.\\n' | uv run scripts/t.py

為什麼：每次各寫一個 subprocess 包裝容易忘了先建置、一批太大卡住、或被一段超長原文拖到逾時。
沒給 FILE 就讀標準輸入。空白行跳過（和 regress.py 一樣）；-w／-m／-s 直接傳給 t.exe，沒給是預設模式。
每次先 cargo build release 版 t.exe，讓它和目前原始碼、data/ 一致（t.exe 編譯時寫死讀 repo 的 data/）。
每 200 行一批送 t.exe（逾時 120 秒）；某批逾時就逐行重跑（每行 20 秒），逾時的格子寫「逾時」。
超過 --max-bytes（UTF-8 位元組，預設 100000）的行不送，寫「略過：N bytes」。
輸出：每行輸入一列「原文<TAB>zh-Hant<TAB>zh-Hans」（--lang 只輸出那一欄），順序同輸入；
--miss 只列有「缺：」、逾時、略過的列。stderr 印一行統計（格數）。
建置失敗，或 t.exe 回傳行數和送進去的不同時 exit 2；其他情況 exit 0。
"""
import argparse
import subprocess
import sys
from concurrent.futures import ThreadPoolExecutor
from pathlib import Path

ROOT = Path(__file__).resolve().parent.parent
T = ROOT / "engine/target/release/examples/t.exe"
LANGS = ("zh-Hant", "zh-Hans")
CHUNK, CHUNK_TIMEOUT, LINE_TIMEOUT = 200, 120, 20


def t_exe(lang: str, mode: str | None, lines: list[str], timeout: float) -> list[str]:
    r = subprocess.run([str(T), lang, *([mode] if mode else [])], input="".join(l + "\n" for l in lines),
                       capture_output=True, encoding="utf-8", errors="replace", timeout=timeout)
    out = r.stdout.split("\n")
    if out and out[-1] == "":
        out.pop()
    if len(out) != len(lines):
        print(f"t.exe {lang} 送 {len(lines)} 行、回 {len(out)} 行（exit {r.returncode}）：{r.stderr.strip()[-500:]}", file=sys.stderr)
        sys.exit(2)
    return out


def chunk(lang: str, mode: str | None, lines: list[str]) -> list[str]:
    try:
        return t_exe(lang, mode, lines, CHUNK_TIMEOUT)
    except subprocess.TimeoutExpired:
        out = []
        for l in lines:
            try:
                out += t_exe(lang, mode, [l], LINE_TIMEOUT)
            except subprocess.TimeoutExpired:
                out.append("逾時")
        return out


def main() -> int:
    for s in (sys.stdin, sys.stdout, sys.stderr):
        s.reconfigure(encoding="utf-8")
    ap = argparse.ArgumentParser(description="批次跑離線工具 t.exe，輸出 TSV")
    g = ap.add_mutually_exclusive_group()
    for flag in ("-w", "-m", "-s"):
        g.add_argument(flag, dest="mode", action="store_const", const=flag)
    ap.add_argument("--lang", choices=LANGS, help="只跑一種語言，預設繁簡都跑")
    ap.add_argument("--miss", action="store_true", help="只列有缺、逾時、略過的列")
    ap.add_argument("--max-bytes", type=int, default=100000, help="超過這個長度（UTF-8 位元組）的行不送，預設 100000")
    ap.add_argument("files", nargs="*", type=Path, help="樣本檔，一行一句；沒給就讀標準輸入")
    args = ap.parse_args()

    r = subprocess.run(["cargo", "build", "-q", "--release", "--manifest-path", str(ROOT / "engine/Cargo.toml"), "--example", "t"],
                       capture_output=True, encoding="utf-8", errors="replace")
    if r.returncode:
        print(f"建置 t.exe 失敗：\n{r.stderr}", file=sys.stderr)
        return 2

    text = "".join(f.read_text(encoding="utf-8") + "\n" for f in args.files) if args.files else sys.stdin.read()
    lines = [l.rstrip("\r") for l in text.split("\n") if l.strip()]
    sizes = [len(l.encode("utf-8")) for l in lines]
    send = [l for l, n in zip(lines, sizes) if n <= args.max_bytes]
    langs = [args.lang] if args.lang else list(LANGS)
    jobs = [(lang, send[i:i + CHUNK]) for lang in langs for i in range(0, len(send), CHUNK)]
    with ThreadPoolExecutor(8) as pool:
        done = list(pool.map(lambda j: chunk(j[0], args.mode, j[1]), jobs))
    outs = {lang: iter([o for (jl, _), part in zip(jobs, done) if jl == lang for o in part]) for lang in langs}

    bad = ("缺：", "逾時", "略過")
    counts = dict.fromkeys(bad, 0)
    for l, n in zip(lines, sizes):
        cells = [f"略過：{n} bytes" if n > args.max_bytes else next(outs[lang]) for lang in langs]
        hit = [b for c in cells for b in bad if c.startswith(b)]
        for b in hit:
            counts[b] += 1
        if hit or not args.miss:
            print("\t".join([l, *cells]))
    print(f"{len(lines)} 句 × {len(langs)} 語言：缺 {counts['缺：']}、逾時 {counts['逾時']}、略過 {counts['略過']}（格數）", file=sys.stderr)
    return 0


if __name__ == "__main__":
    sys.exit(main())
