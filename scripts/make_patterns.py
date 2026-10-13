# /// script
# requires-python = ">=3.11"
# dependencies = ["pefile>=2023.2.7", "capstone>=5.0"]
# ///
"""從目前安裝的 exe 為反編譯快照裡已知的函式產生特徵碼，寫到 engine/patterns.tsv。
引擎執行時用它找 hook 位置；df_snapshot.py 也用它在新版本 exe 標出同一批函式。

函式位址取自最新快照 exe/labels.tsv 的 fn_ 標籤。從函式開頭逐條反組譯，
RIP 相對位移、call／jmp 的 rel32、以 rsp／rbp 為基底的位移、堆疊框大小都換成 ??（改版時最常變），
指令一條條加長，直到整段在 .text 只出現一次。

用法（repo 根目錄）：
  uv run scripts/make_patterns.py           # 重新產生
  uv run scripts/make_patterns.py --check   # 只驗證 patterns.tsv 每條在目前 exe 恰好命中一次
"""
import argparse
import re
import sys
from pathlib import Path

import capstone
import pefile

from dfpaths import DF

ROOT = Path(__file__).resolve().parents[1]
SNAPSHOTS = ROOT.parent / "df-decompiled-reference/snapshots"
PATTERNS = ROOT / "engine/patterns.tsv"
MIN_BYTES, MAX_INSNS = 16, 48


def text_section(pe: pefile.PE, data: bytes) -> tuple[int, bytes]:
    sec = next(s for s in pe.sections if s.Name.rstrip(b"\0") == b".text")
    return pe.OPTIONAL_HEADER.ImageBase + sec.VirtualAddress, data[sec.PointerToRawData:sec.PointerToRawData + sec.SizeOfRawData]


def to_regex(pattern: list[str]) -> re.Pattern:
    return re.compile(b"".join(b"." if t == "??" else re.escape(bytes([int(t, 16)])) for t in pattern), re.S)


def count(blob: bytes, pattern: list[str], limit: int = 2) -> int:
    n = 0
    for _ in to_regex(pattern).finditer(blob):
        n += 1
        if n >= limit:
            break
    return n


def wildcard_mask(insn) -> set[int]:
    """這條指令裡要換成 ?? 的位元組索引。"""
    mask = set()
    x86 = capstone.x86
    for op in insn.operands:
        if op.type == x86.X86_OP_MEM and insn.disp_size and (
                op.mem.base in (x86.X86_REG_RIP, x86.X86_REG_RSP, x86.X86_REG_RBP) or op.mem.base == 0):
            mask |= set(range(insn.disp_offset, insn.disp_offset + insn.disp_size))
        if op.type == x86.X86_OP_IMM and insn.imm_size and insn.group(capstone.CS_GRP_BRANCH_RELATIVE):
            mask |= set(range(insn.imm_offset, insn.imm_offset + insn.imm_size))
    if insn.mnemonic == "sub" and insn.op_str.startswith("rsp,") and insn.imm_size:
        mask |= set(range(insn.imm_offset, insn.imm_offset + insn.imm_size))  # 堆疊框大小
    return mask


def make_pattern(md, blob: bytes, base: int, va: int) -> list[str] | None:
    pattern: list[str] = []
    off = va - base
    for i, insn in enumerate(md.disasm(blob[off:off + 512], va)):
        mask = wildcard_mask(insn)
        pattern += ["??" if k in mask else f"{b:02x}" for k, b in enumerate(insn.bytes)]
        if len(pattern) >= MIN_BYTES and count(blob, pattern) == 1:
            return pattern
        if i >= MAX_INSNS:
            break
    return None


def latest_labels() -> dict[str, int]:
    snaps = sorted(p for p in SNAPSHOTS.iterdir() if (p / "exe/labels.tsv").is_file())
    if not snaps:
        sys.exit("找不到反編譯快照；先跑 uv run scripts/df_snapshot.py snapshot")
    rows = (line.split("\t") for line in (snaps[-1] / "exe/labels.tsv").read_text(encoding="utf-8").splitlines()[1:])
    return {name[3:]: int(va, 16) for va, name, *_ in rows if name.startswith("fn_")}


def read_patterns() -> list[tuple[str, str]]:
    lines = PATTERNS.read_text(encoding="utf-8").splitlines() if PATTERNS.is_file() else []
    return [tuple(line.split("\t")[:2]) for line in lines if line and not line.startswith("#")]


def main() -> int:
    ap = argparse.ArgumentParser()
    ap.add_argument("--check", action="store_true")
    args = ap.parse_args()
    data = (DF / "Dwarf Fortress.exe").read_bytes()
    pe = pefile.PE(data=data, fast_load=True)
    base, blob = text_section(pe, data)

    if args.check:
        bad = [(name, n) for name, pat in read_patterns() if (n := count(blob, pat.split(), limit=3)) != 1]
        for name, n in bad:
            print(f"FAIL {name}: {n} matches")
        print("patterns OK" if not bad else f"{len(bad)} pattern(s) broken")
        return 1 if bad else 0

    labels = latest_labels()
    wanted = sorted(labels)
    md = capstone.Cs(capstone.CS_ARCH_X86, capstone.CS_MODE_64)
    md.detail = True
    out = ["# name\tpattern\tva_at_generation（由 scripts/make_patterns.py 產生）"]
    for name in wanted:
        pat = make_pattern(md, blob, base, labels[name])
        if pat is None:
            print(f"FAIL {name}: no unique pattern within {MAX_INSNS} instructions", file=sys.stderr)
            return 1
        out.append(f"{name}\t{' '.join(pat)}\t0x{labels[name]:x}")
        print(f"{name:32} {len(pat):3} bytes  0x{labels[name]:x}")
    PATTERNS.write_text("\n".join(out) + "\n", encoding="utf-8", newline="\n")
    print(f"-> {PATTERNS.relative_to(ROOT)}")
    return 0


if __name__ == "__main__":
    sys.exit(main())
