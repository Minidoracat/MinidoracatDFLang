# /// script
# requires-python = ">=3.11"
# ///
"""譯文回歸比對（repo 根目錄）：用離線工具 t.exe（engine/examples/t.rs）翻同一批樣本，比較某個 git 版本與目前工作目錄的譯文。

  uv run scripts/regress.py [--base REF] [--show N]
  uv run scripts/regress.py --clean

為什麼：改句型或字典時，新的樣板可能切開別的句子、讓原本翻得出來的句子變成「缺」或譯文走樣，
只看新加的句子發現不了。發版前也可以和上一版的 tag 比（--base v0.5.2）。

REF 預設 HEAD（任何 commit、tag 都可以）；N 是每類最多列幾筆（預設 40）。
base 版本檢出成 temp/regress/<sha 前 12 碼> 的 worktree（t.exe 編譯時寫死讀自己那份 data/，所以要另一份檢出），
兩邊各建一次 t.exe。只留一個快取 worktree，其他 sha 的會移掉；--clean 全部移掉。
樣本：temp/round6/corpus.txt（介面字串，-w）與 temp/round*/samples_*.txt
（檔名 .w.txt／.m.txt／.s.txt 加 -w／-m／-s，.d.txt 是預設模式；*.zh-Hant.txt、*.zh-Hans.txt 是紀錄的輸出，跳過）。繁簡都跑。
其他檔名（例如 samples_x_m.txt）會列出來並 exit 2，不建置。
分類：fixed 舊的缺、新的有譯文；broke 舊的有、新的缺；其他不同的算 changed（依差異去重）。
有 broke 或某檔輸出行數和輸入不同時 exit 1；建置失敗 exit 2。
"""
import argparse
import difflib
import re
import shutil
import subprocess
import sys
import time
from collections import defaultdict
from concurrent.futures import ThreadPoolExecutor
from pathlib import Path

ROOT = Path(__file__).resolve().parents[1]
REGRESS = ROOT / "temp/regress"
LANGS = ("zh-Hant", "zh-Hans")
T = Path("engine/target/release/examples/t.exe")


def git(*args: str) -> subprocess.CompletedProcess:
    return subprocess.run(["git", *args], cwd=ROOT, capture_output=True, text=True, encoding="utf-8", errors="replace")


def remove_worktrees(keep: Path | None = None) -> None:
    for d in REGRESS.glob("*"):
        if d.is_dir() and d != keep:
            git("worktree", "remove", "--force", str(d))
            shutil.rmtree(d, ignore_errors=True)  # 沒登記成 worktree 的殘留資料夾
    git("worktree", "prune")


def worktree(ref: str) -> tuple[str, Path]:
    r = git("rev-parse", "--verify", "--quiet", f"{ref}^{{commit}}")
    if r.returncode != 0:
        sys.exit(f"找不到版本：{ref}")
    sha = r.stdout.strip()
    path = REGRESS / sha[:12]
    remove_worktrees(keep=path)
    if not (path / ".git").exists():
        shutil.rmtree(path, ignore_errors=True)
        REGRESS.mkdir(parents=True, exist_ok=True)
        # 關掉 hooks：post-checkout 會跑 GitNexus 自動索引，不要讓它索引這個拋棄式 worktree
        r = git("-c", f"core.hooksPath={REGRESS / 'no-hooks'}", "worktree", "add", "--detach", str(path), sha)
        if r.returncode != 0:
            sys.exit(f"建 worktree 失敗：\n{r.stderr}")
    return sha, path


def build(root: Path) -> Path:
    r = subprocess.run(["cargo", "build", "--release", "--manifest-path", str(root / "engine/Cargo.toml"), "--example", "t"],
                       capture_output=True, text=True, encoding="utf-8", errors="replace")
    if r.returncode != 0:
        print(f"建置失敗：{root}\n{r.stderr}", file=sys.stderr)
        sys.exit(2)
    return root / T


def files() -> list[tuple[Path, str | None]]:
    fs, bad = [], []
    corpus = ROOT / "temp/round6/corpus.txt"
    if corpus.is_file():
        fs.append((corpus, "-w"))
    else:
        print(f"沒有 {corpus.relative_to(ROOT).as_posix()}，跳過介面字串")
    for f in sorted(ROOT.glob("temp/round*/samples_*.txt")):
        if re.search(r"\.zh-Han[st]\.txt$", f.name):
            continue
        m = re.search(r"\.([wmsd])\.txt$", f.name)
        if not m:
            bad.append(f.relative_to(ROOT).as_posix())
        else:
            fs.append((f, None if m.group(1) == "d" else "-" + m.group(1)))
    if bad:
        print("樣本檔名要以 .w.txt／.m.txt／.s.txt／.d.txt（預設模式）結尾：\n  " + "\n  ".join(bad), file=sys.stderr)
        sys.exit(2)
    return fs


def run(exe: Path, f: Path, mode: str | None, lang: str) -> tuple[list[str], list[str], str]:
    lines = [l for l in f.read_text(encoding="utf-8").split("\n") if l.strip()]
    r = subprocess.run([str(exe), lang, *([mode] if mode else [])], input="\n".join(lines) + "\n",
                       capture_output=True, encoding="utf-8", errors="replace", cwd=ROOT)
    out = r.stdout.split("\n")
    if out and out[-1] == "":
        out.pop()
    return lines, out, r.stderr.strip() if r.returncode else ""


def short_diff(a: str, b: str) -> str:
    sm = difflib.SequenceMatcher(None, a, b)
    out = []
    for op, i1, i2, j1, j2 in sm.get_opcodes():
        if op != "equal":
            out.append(f"[{a[max(0, i1 - 8):i2 + 8]}]→[{b[max(0, j1 - 8):j2 + 8]}]")
    return " ".join(out)[:300]


def more(n: int, show: int) -> None:
    if n > show:
        print(f"  …另有 {n - show} 筆")


def main() -> int:
    ap = argparse.ArgumentParser(description="比較某個 git 版本與目前工作目錄的離線譯文")
    ap.add_argument("--base", default="HEAD", help="比較的版本（commit、tag），預設 HEAD")
    ap.add_argument("--show", type=int, default=40, help="每類最多列幾筆，預設 40")
    ap.add_argument("--clean", action="store_true", help="移掉 temp/regress/ 底下所有 worktree 後結束")
    args = ap.parse_args()
    if args.clean:
        remove_worktrees()
        print("已移掉 temp/regress/ 底下的 worktree")
        return 0

    sample_files = files()
    start = time.perf_counter()
    sha, base_root = worktree(args.base)
    old_exe, new_exe = build(base_root), build(ROOT)
    built = time.perf_counter() - start

    jobs = [(f, mode, lang) for f, mode in sample_files for lang in LANGS]
    with ThreadPoolExecutor(12) as pool:
        olds = pool.map(lambda j: run(old_exe, *j), jobs)
        news = pool.map(lambda j: run(new_exe, *j), jobs)
        results = list(zip(jobs, olds, news))

    miss = lambda s: s.startswith("缺")
    fixed, broke, mismatch = [], [], []
    changed = defaultdict(list)
    total = 0
    for (f, mode, lang), (lines, old, old_err), (_, new, new_err) in results:
        name = f.relative_to(ROOT).as_posix()
        total += len(lines)
        bad = [f"{tag} {len(out)} 行" + (f"（{err.splitlines()[-1]}）" if err else "")
               for tag, out, err in (("舊", old, old_err), ("新", new, new_err)) if len(out) != len(lines) or err]
        if bad:
            mismatch.append(f"{name} {lang}：輸入 {len(lines)} 行，" + "、".join(bad))
            continue
        for src, a, b in zip(lines, old, new):
            if a == b:
                continue
            if miss(a) and not miss(b):
                fixed.append((name, lang, src, b))
            elif miss(b) and not miss(a):
                broke.append((name, lang, src, a, b))
            else:
                changed[short_diff(a, b)].append(src)

    n_changed = sum(map(len, changed.values()))
    print(f"base {sha[:12]}（{args.base}）：{len(jobs) // len(LANGS)} 個檔、{total} 行 × {len(LANGS)} 語言，"
          f"耗時 {time.perf_counter() - start:.1f} 秒（建置 {built:.1f} 秒）")
    print(f"fixed {len(fixed)}　broke {len(broke)}　changed {n_changed}（{len(changed)} 種差異）"
          + (f"　行數不符 {len(mismatch)} 檔" if mismatch else ""))
    show = args.show
    if mismatch:
        print("\n== 行數不符 ==")
        for m in mismatch[:show]:
            print(f"  {m}")
        more(len(mismatch), show)
    if broke:
        print("\n== broke（舊的有譯文、新的缺）==")
        for name, lang, src, a, b in broke[:show]:
            print(f"  {name} {lang}\n    原文：{src}\n    舊：{a}\n    新：{b}")
        more(len(broke), show)
    if changed:
        print("\n== changed（依差異去重，多的在前）==")
        ranked = sorted(changed.items(), key=lambda kv: -len(kv[1]))
        for diff, srcs in ranked[:show]:
            print(f"  {len(srcs)} 行 {diff}\n    例：{srcs[0]}")
        more(len(ranked), show)
    if fixed:
        print("\n== fixed 例子（舊的缺、新的有譯文）==")
        for name, lang, src, b in fixed[:show]:
            print(f"  {name} {lang}：{src}\n    → {b}")
        more(len(fixed), show)
    return 1 if broke or mismatch else 0


if __name__ == "__main__":
    sys.exit(main())
