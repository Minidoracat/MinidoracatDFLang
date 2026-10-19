# /// script
# requires-python = ">=3.11"
# dependencies = ["pefile>=2023.2.7"]
# ///
"""Dwarf Fortress 反編譯快照管理，產物寫到 ../df-decompiled-reference/（比照 pz-decompiled-reference）。

L1（秒級，遊戲或 DFHack 更新後必跑）：snapshot
L2（53.16 實測約 40 分鐘，按需）：decompile，Ghidra headless 匯入 exe、套標籤、分析、匯出全函式 pseudo-C

用法（repo 根目錄）：
  uv run scripts/df_snapshot.py snapshot [--version 53.16] [--date YYYYMMDD]
  uv run scripts/df_snapshot.py verify [VERSION]
  uv run scripts/df_snapshot.py diff OLD NEW
  uv run scripts/df_snapshot.py decompile [VERSION] [--timeout 秒] [--export-only]
  uv run scripts/df_snapshot.py redecompile [VERSION] [--timeout 秒] [--max-instructions N] [--only FUN…]
                                                     全量匯出時失敗的函式，用較長逾時與指令上限重做並併回（--only 先做指定的）
  uv run scripts/df_snapshot.py status | where
  uv run scripts/df_snapshot.py find REGEX [--max 50] [--version V]     pseudo-C 逐行搜尋：行號、所在函式、內容
  uv run scripts/df_snapshot.py fn NAME [--max-lines 3000] [--version V] 印出整個函式（含行號）
  uv run scripts/df_snapshot.py callers NAME [--version V]              呼叫 NAME( 的位置，依所在函式分組
路徑：遊戲與 DFHack 由 scripts/dfpaths.py 從 Steam 找（DF_PATH／DFHACK_PATH 可覆寫）；
Ghidra 用 GHIDRA_HOME，沒設就找 ../df-decompiled-reference/tools/ghidra。
"""
import argparse
import collections
import datetime
import hashlib
import json
import os
import re
import shutil
import subprocess
import sys
import xml.etree.ElementTree as ET
from pathlib import Path

import pefile

from dfpaths import DF, DFHACK, STEAMAPPS, WORKSHOP

ROOT = Path(__file__).resolve().parents[1]
REF = ROOT.parent / "df-decompiled-reference"
SNAPSHOTS = REF / "snapshots"
EXE = DF / "Dwarf Fortress.exe"
GHIDRA_ROOTS = [Path(p) for p in filter(None, [os.environ.get("GHIDRA_HOME")])] + [REF / "tools/ghidra"]

OWN_PATTERNS = ROOT / "engine/patterns.tsv"
# 本 repo 的特徵碼還沒涵蓋時才用的外部來源（只做函式定位的研究參考）：
# DFI18n PR #2（CooperZhuang/dfi18n@dc44eaf，configs/functions.lua）；
# 53.15 起單位描述與想法句走這兩個組字函式，不經 addst，也不寫 gps.screen。
EXTRA_PATTERNS = {
    "description_composer": "83 f9 20 0f 87 ff 17 00 00 53 48 83 ec 20 48 63 c1 49 8b d8 4c 8d 05 ?? ?? ?? ??",
    "thought_composer": "48 89 5c 24 18 55 56 57 41 56 41 57 48 8d ac 24 70 ff ff ff 48 81 ec 90 01 00 00 48 8b 05 ?? ?? ?? ?? 48 33 c4 48 89 85 80 00 00 00",
}
# raws 物件標頭：OBJECT 類型與物件 token 名稱不同者的對照；ITEM／BUILDING 用「類型_」前綴，其餘與類型同名
# （CREATURE_TILE 之類同前綴的屬性 token 不是標頭，所以前綴規則只給這兩類）
OBJECT_HEADERS = {
    "LANGUAGE": {"WORD", "SYMBOL", "TRANSLATION"},
    "BODY": {"BODY", "BODYGLOSS"},
    "DESCRIPTOR_COLOR": {"COLOR"},
    "DESCRIPTOR_SHAPE": {"SHAPE"},
    "DESCRIPTOR_PATTERN": {"COLOR_PATTERN"},
}
PREFIX_HEADERS = ("ITEM", "BUILDING")
TOKEN_RE = re.compile(r"\[([A-Z0-9_]+)((?::[^\]\[]*)*)\]")
LUA_STR_RE = re.compile(r'"((?:[^"\\\n]|\\.)*)"|\'((?:[^\'\\\n]|\\.)*)\'')


def sha256(path: Path) -> str:
    h = hashlib.sha256()
    with path.open("rb") as f:
        for block in iter(lambda: f.read(1 << 20), b""):
            h.update(block)
    return h.hexdigest()


def esc(s: str) -> str:
    return s.replace("\\", "\\\\").replace("\t", "\\t").replace("\n", "\\n")


def write_tsv(path: Path, header: str, rows) -> int:
    path.parent.mkdir(parents=True, exist_ok=True)
    n = 0
    with path.open("w", encoding="utf-8", newline="\n") as f:
        f.write(header + "\n")
        for row in rows:
            f.write("\t".join(esc(str(c)) for c in row) + "\n")
            n += 1
    return n


def read_tsv(path: Path) -> list[list[str]]:
    lines = path.read_text(encoding="utf-8").splitlines()[1:]
    return [line.split("\t") for line in lines if line]


def acf_buildid(appid: int) -> str | None:
    acf = STEAMAPPS / f"appmanifest_{appid}.acf"
    m = acf.is_file() and re.search(r'"buildid"\s+"(\d+)"', acf.read_text(encoding="utf-8"))
    return m.group(1) if m else None


def symbol_table(pe_timestamp: int):
    """回傳 DFHack symbols.xml 中 binary-timestamp 相符的 symbol-table 元素（DFHack 未支援此版時為 None）。"""
    xml = DFHACK / "hack/symbols.xml"
    if not xml.is_file():
        return None
    for table in ET.parse(xml).getroot().iter("symbol-table"):
        ts = table.find("binary-timestamp")
        if ts is not None and int(ts.get("value"), 16) == pe_timestamp:
            return table
    return None


def dfhack_version() -> str | None:
    dll = DFHACK / "hack/dfhack.dll"
    if not dll.is_file():
        return None
    hits = collections.Counter(re.findall(rb"\b(\d{2}\.\d{2}-r\d+(?:\.\d+)?)\b", dll.read_bytes()))
    return hits.most_common(1)[0][0].decode() if hits else None


def string_kind(t: str) -> str:
    if t.startswith((".?A", "?", "@")) or "::" in t or re.search(r"[\\/][\w.\-]+\.\w{2,4}$", t):
        return "code"
    if re.fullmatch(r"[A-Z0-9_:]+", t):
        return "token"
    if re.search(r"[a-z]{2}", t):
        return "text"
    return "other"


def exe_strings(pe: pefile.PE, data: bytes):
    for sec in pe.sections:
        name = sec.Name.rstrip(b"\0").decode()
        if name not in (".rdata", ".data"):
            continue
        blob = data[sec.PointerToRawData:sec.PointerToRawData + sec.SizeOfRawData]
        for m in re.finditer(rb"[\x20-\x7e]{3,}(?=\x00)", blob):
            text = m.group().decode("ascii")
            yield f"0x{sec.VirtualAddress + m.start():08x}", name, string_kind(text), text


def pattern_re(pattern: str) -> re.Pattern:
    return re.compile(b"".join(b"." if x == "??" else re.escape(bytes([int(x, 16)])) for x in pattern.split()), re.S)


def text_patterns() -> dict[str, tuple[str, str]]:
    """名稱 -> (特徵碼, 出處)。優先用本 repo 的 engine/patterns.tsv（scripts/make_patterns.py 產生）；
    缺的名稱才從已訂閱的 DFI18n configs/functions.lua 與 EXTRA_PATTERNS 補。"""
    found = {}
    if OWN_PATTERNS.is_file():
        for line in OWN_PATTERNS.read_text(encoding="utf-8").splitlines():
            if line and not line.startswith("#"):
                name, pat = line.split("\t")[:2]
                found[name] = (pat, "engine/patterns.tsv")
    entry = re.compile(r'\{\s*"(\w+)",\s*"Dwarf Fortress\.exe",\s*"pattern",\s*"([0-9a-fA-F? ]+)"\s*\}')
    for lua in sorted(WORKSHOP.glob("*/scripts_modinstalled/configs/functions.lua")):
        windows = lua.read_text(encoding="utf-8").split("LINUX_MEMORY_SEARCH")[0]
        for name, pat in entry.findall(windows):
            found.setdefault(name, (pat, f"workshop {lua.parts[-4]} functions.lua"))
    for name, pat in EXTRA_PATTERNS.items():
        found.setdefault(name, (pat, "DFI18n PR #2"))
    return found


def exe_labels(pe: pefile.PE, data: bytes, table) -> tuple[list[tuple[str, str, str]], list[str]]:
    labels, warnings = [], []
    if table is not None:
        for el in table:
            if el.tag == "global-address":
                labels.append((el.get("value").lower(), f"g_{el.get('name')}", "dfhack symbols.xml"))
            elif el.tag == "vtable-address":
                labels.append((el.get("value").lower(), f"vt_{el.get('name')}", "dfhack symbols.xml"))
    text = next(s for s in pe.sections if s.Name.rstrip(b"\0") == b".text")
    blob = data[text.PointerToRawData:text.PointerToRawData + text.SizeOfRawData]
    base = pe.OPTIONAL_HEADER.ImageBase + text.VirtualAddress
    for name, (pat, source) in sorted(text_patterns().items()):
        hits = [m.start() for m in pattern_re(pat).finditer(blob)]
        if len(hits) == 1:
            labels.append((f"0x{base + hits[0]:x}", f"fn_{name}", source))
        else:
            warnings.append(f"pattern {name} ({source}): {len(hits)} matches, not labelled")
    return labels, warnings


def object_header(obj_type: str | None, token: str) -> bool:
    if obj_type is None:
        return False
    if obj_type in OBJECT_HEADERS:
        return token in OBJECT_HEADERS[obj_type]
    if obj_type in PREFIX_HEADERS:
        return token.startswith(obj_type + "_")
    return token == obj_type


def raws_text(vanilla: Path):
    """(檔案, 物件, token, 文字)：raws 中含小寫字母的 token 參數、TEXT_SET 的文字行、程序生成 Lua 的字串字面值。"""
    for f in sorted(vanilla.glob("*/objects/*.txt")):
        rel = f.relative_to(vanilla).as_posix()
        obj_type, obj = None, ""
        for line in f.read_text(encoding="cp437").splitlines():
            s = line.strip()
            # TEXT_SET 的每行是句型範本，可夾 [NAME] 之類的插槽；括號外有字就是一句
            if obj_type == "TEXT_SET" and obj and TOKEN_RE.sub("", s).strip():
                yield rel, obj, "TEXT", s
                continue
            tokens = TOKEN_RE.findall(s)
            for name, args in tokens:
                if name == "OBJECT":
                    obj_type, obj = args[1:], ""
                elif object_header(obj_type, name):
                    obj = f"{name}{args}"
                elif re.search(r"[a-z]", args) and not re.fullmatch(r":'.'", args):
                    yield rel, obj, name, args[1:]
    for f in sorted(vanilla.glob("*/scripts/**/*.lua")):
        rel = f.relative_to(vanilla).as_posix()
        for m in LUA_STR_RE.finditer(f.read_text(encoding="utf-8", errors="replace")):
            s = m.group(1) if m.group(1) is not None else m.group(2)
            if " " in s.strip() and re.search(r"[a-z]{2}", s):
                yield rel, "", "LUA", s


def copy_raws(dest: Path) -> int:
    src = DF / "data/vanilla"
    n = 0
    for f in src.rglob("*"):
        if f.suffix.lower() in (".txt", ".lua") and f.is_file():
            out = dest / "vanilla" / f.relative_to(src)
            out.parent.mkdir(parents=True, exist_ok=True)
            shutil.copy2(f, out)
            n += 1
    return n


def mod_info(mod_dir: Path) -> dict[str, str]:
    info = mod_dir / "info.txt"
    text = info.read_text(encoding="utf-8", errors="replace") if info.is_file() else ""
    return dict(re.findall(r"^\[([A-Z_]+):([^\]]*)\]", text, re.M))


def rust_modules(raw: bytes) -> list[str]:
    """panic 位置字串裡的專案原始碼路徑（<crate>/src/...rs），排除 cargo registry 與 rustc 標準庫。
    這些字串在 DLL 裡首尾相連，crate 名稱前常黏著前一段字（如 "valuedfi18n"），取最短的已知 crate 名稱後綴校正。"""
    paths = [m.decode().replace("\\", "/") for m in re.findall(rb"[\w\-.:\\/]+[\\/]src[\\/][\w\\/]+\.rs", raw)]
    paths = [p for p in paths if not re.search(r"\.cargo/|/rustc/|/library/", p)]
    hits = [(head.rsplit("/", 1)[-1], tail) for head, tail in (p.rsplit("/src/", 1) for p in paths)]
    names = {c for c, _ in hits}
    fixed = {(min((n for n in names if c.endswith(n)), key=len), tail) for c, tail in hits}
    return sorted(f"{c}/src/{tail}" for c, tail in fixed)


def analyze_native_mods(dest: Path) -> list[str]:
    """已訂閱且帶 .dll 的 MOD：匯出、匯入、Rust crate、原始碼模組清單，外加腳本與設定副本（不含 DLL 與翻譯資料）。"""
    done = []
    for mod_dir in sorted(p for p in WORKSHOP.iterdir() if p.is_dir()):
        dlls = sorted(mod_dir.rglob("*.dll"))
        if not dlls:
            continue
        info = mod_info(mod_dir)
        out = dest / f"{info.get('ID', mod_dir.name)}-{info.get('DISPLAYED_VERSION', 'unknown')}"
        libs = []
        for dll in dlls:
            raw = dll.read_bytes()
            pe = pefile.PE(data=raw)
            libs.append({
                "file": dll.relative_to(mod_dir).as_posix(),
                "sha256": hashlib.sha256(raw).hexdigest(),
                "pe_timestamp": pe.FILE_HEADER.TimeDateStamp,
                "exports": [e.name.decode() for e in pe.DIRECTORY_ENTRY_EXPORT.symbols if e.name]
                if hasattr(pe, "DIRECTORY_ENTRY_EXPORT") else [],
                "imports": [i.dll.decode() for i in getattr(pe, "DIRECTORY_ENTRY_IMPORT", [])],
                "rust_crates": sorted({m.decode() for m in re.findall(rb"index\.crates\.io-[0-9a-f]+[\\/]([A-Za-z0-9_\-]+?)-\d+\.\d+", raw)}),
                "rust_modules": rust_modules(raw),
            })
        for f in mod_dir.rglob("*"):
            if f.is_file() and f.suffix in (".lua", ".toml", ".txt") and "local-data" not in f.parts:
                target = out / "files" / f.relative_to(mod_dir)
                target.parent.mkdir(parents=True, exist_ok=True)
                shutil.copy2(f, target)
        out.mkdir(parents=True, exist_ok=True)
        (out / "analysis.json").write_text(json.dumps({"workshop_id": mod_dir.name, "info": info, "libs": libs},
                                                      ensure_ascii=False, indent=2), encoding="utf-8")
        done.append(out.name)
    return done


def snap_dir(version: str, date: str) -> Path:
    return SNAPSHOTS / f"{version}-{date}"


def find_snapshot(name: str | None) -> Path | None:
    """接受完整目錄名或裸版本號（取該版最新快照）；None 取全部最新。"""
    if not SNAPSHOTS.is_dir():
        return None
    snaps = sorted(p for p in SNAPSHOTS.iterdir() if (p / "VERSION.txt").is_file())
    if name:
        snaps = [p for p in snaps if p.name == name or p.name.startswith(name + "-")]
    return snaps[-1] if snaps else None


def load_meta(snap: Path) -> dict:
    return json.loads((snap / "VERSION.txt").read_text(encoding="utf-8"))


def cmd_snapshot(args) -> int:
    data = EXE.read_bytes()
    pe = pefile.PE(data=data, fast_load=True)
    table = symbol_table(pe.FILE_HEADER.TimeDateStamp)
    table_name = table.get("name") if table is not None else None
    version = args.version or (re.search(r"v0\.(\d+\.\d+)", table_name or "") or [None, None])[1]
    if not version:
        print("DFHack symbols.xml 沒有這個 exe 的 symbol table；請用 --version 指定版本號", file=sys.stderr)
        return 1
    snap = snap_dir(version, args.date or datetime.date.today().strftime("%Y%m%d"))
    # 同日重跑只清 L1 產物；ghidra/ 是約 40 分鐘的 L2 結果，保留
    for old in ("exe", "raws", "mods"):
        shutil.rmtree(snap / old, ignore_errors=True)
    (snap / "exe").mkdir(parents=True)

    n_str = write_tsv(snap / "exe/strings.tsv", "rva\tsection\tkind\ttext", exe_strings(pe, data))
    labels, warnings = exe_labels(pe, data, table)
    n_lab = write_tsv(snap / "exe/labels.tsv", "va\tname\tsource", labels)
    if table is not None:
        (snap / "exe/symbols.xml").write_text(ET.tostring(table, encoding="unicode"), encoding="utf-8")
    n_raw = copy_raws(snap / "raws")
    n_txt = write_tsv(snap / "raws_text.tsv", "file\tobject\ttoken\ttext", raws_text(DF / "data/vanilla"))
    mods = analyze_native_mods(snap / "mods")

    meta = {
        "df_version": version,
        "steam_buildid": acf_buildid(975370),
        "exe_sha256": hashlib.sha256(data).hexdigest(),
        "exe_size": len(data),
        "pe_timestamp": f"0x{pe.FILE_HEADER.TimeDateStamp:08x}",
        "image_base": f"0x{pe.OPTIONAL_HEADER.ImageBase:x}",
        "dfhack_symbol_table": table_name,
        "dfhack_version": dfhack_version(),
        "dfhack_buildid": acf_buildid(2346660),
        "counts": {"exe_strings": n_str, "labels": n_lab, "raw_files": n_raw, "raws_text": n_txt},
        "native_mods": mods,
        "warnings": warnings,
        "created": datetime.datetime.now().isoformat(timespec="seconds"),
    }
    (snap / "VERSION.txt").write_text(json.dumps(meta, ensure_ascii=False, indent=2), encoding="utf-8")
    print(f"snapshot -> {snap}")
    print(json.dumps(meta["counts"], ensure_ascii=False), "| symbol table:", table_name, "| native mods:", mods)
    for w in warnings:
        print("WARN", w)
    return 0


def cmd_verify(args) -> int:
    snap = find_snapshot(args.version)
    if snap is None:
        print("找不到快照；先跑 snapshot", file=sys.stderr)
        return 1
    meta = load_meta(snap)
    labels = {row[1]: row[0] for row in read_tsv(snap / "exe/labels.tsv")}
    texts = {row[3] for row in read_tsv(snap / "exe/strings.tsv") if row[2] == "text"}
    raws = {(row[2], row[3]) for row in read_tsv(snap / "raws_text.tsv")}
    checks = [
        ("DFHack symbol table matched", meta["dfhack_symbol_table"] is not None),
        ("global g_gps labelled", "g_gps" in labels),
        ("text hook fn_addst labelled", "fn_addst" in labels),
        ("title string 'Create new world' in exe", "Create new world" in texts),
        ("creature name 'dwarf:dwarves:dwarven' in raws", ("NAME", "dwarf:dwarves:dwarven") in raws),
    ]
    dec = snap / "ghidra/Dwarf Fortress.exe.c"
    if dec.is_file():
        head = f"// ============ fn_addst @ {labels.get('fn_addst')} "
        with dec.open(encoding="utf-8", errors="replace") as f:
            checks.append(("decompiled fn_addst present", any(line.startswith(head) for line in f)))
    ok = True
    for name, passed in checks:
        print(("PASS " if passed else "FAIL ") + name)
        ok &= passed
    print(f"{snap.name}: {'ALL PASS' if ok else 'FAILED'}")
    return 0 if ok else 1


def cmd_diff(args) -> int:
    a, b = find_snapshot(args.old), find_snapshot(args.new)
    if a is None or b is None:
        print("找不到指定快照", file=sys.stderr)
        return 1
    ma, mb = load_meta(a), load_meta(b)
    print(f"{a.name} -> {b.name}  exe {'same' if ma['exe_sha256'] == mb['exe_sha256'] else 'CHANGED'}")
    report = []
    for title, rel, key in (("exe text strings", "exe/strings.tsv", lambda r: r[3] if r[2] == "text" else None),
                            ("raws text", "raws_text.tsv", lambda r: "\t".join(r))):
        sa = {k for r in read_tsv(a / rel) if (k := key(r))}
        sb = {k for r in read_tsv(b / rel) if (k := key(r))}
        added, removed = sorted(sb - sa), sorted(sa - sb)
        print(f"{title}: +{len(added)} -{len(removed)}")
        report += [f"## {title} added"] + added + [f"## {title} removed"] + removed
    out = b / f"diff-from-{a.name}.txt"
    out.write_text("\n".join(report) + "\n", encoding="utf-8")
    print(f"details -> {out}")
    return 0


def find_ghidra() -> Path | None:
    for root in GHIDRA_ROOTS:
        if (root / "support").is_dir():
            return root
        hits = sorted(root.glob("ghidra_*/support")) if root.is_dir() else []
        if hits:
            return hits[-1].parent
    return None


def cmd_decompile(args) -> int:
    snap = find_snapshot(args.version)
    ghidra = find_ghidra()
    if snap is None or ghidra is None:
        print("缺快照（先跑 snapshot）或找不到 Ghidra（設 GHIDRA_HOME）", file=sys.stderr)
        return 1
    if sha256(EXE) != load_meta(snap)["exe_sha256"]:
        print(f"REFUSING: 目前的 exe 與 {snap.name} 的 sha256 不符（遊戲已更新？先跑 snapshot）", file=sys.stderr)
        return 1
    out_dir = snap / "ghidra"
    proj = out_dir / "project"
    proj.mkdir(parents=True, exist_ok=True)
    out_c = out_dir / "Dwarf Fortress.exe.c"
    headless = ghidra / "support" / ("analyzeHeadless.bat" if sys.platform == "win32" else "analyzeHeadless")
    scripts = ["-scriptPath", str(ROOT / "scripts"), "-postScript", "GhidraExportDecomp.java", str(out_c)]
    if args.export_only:
        cmd = [str(headless), str(proj), "df", "-process", EXE.name, "-noanalysis", *scripts]
    else:
        cmd = [str(headless), str(proj), "df", "-import", str(EXE), "-overwrite",
               "-preScript", "GhidraPrepare.java", str(snap / "exe/labels.tsv"), *scripts,
               "-analysisTimeoutPerFile", str(args.timeout)]
    # Ghidra 12 的 analyzeHeadless 讀 GHIDRA_HEADLESS_MAXMEM（預設 2G，25 MB 的 exe 會 OOM）
    env = dict(os.environ, GHIDRA_HEADLESS_MAXMEM=os.environ.get("GHIDRA_HEADLESS_MAXMEM", "32G"))
    print("running:", " ".join(cmd), flush=True)
    rc = subprocess.run(cmd, env=env).returncode
    if rc == 0 and out_c.is_file() and out_c.stat().st_size > 0:
        print(f"decompiled -> {out_c} ({out_c.stat().st_size >> 20} MiB)")
        return 0
    print("DECOMPILE FAILED", file=sys.stderr)
    return rc or 1


FAILED_RE = re.compile(r"// ============ (\S+) @ 0x([0-9a-f]+) ============\n// DECOMPILE FAILED[^\n]*\n\n")


def cmd_redecompile(args) -> int:
    snap = find_snapshot(args.version)
    ghidra = find_ghidra()
    out_c = snap / "ghidra" / "Dwarf Fortress.exe.c" if snap else None
    if out_c is None or not out_c.is_file() or ghidra is None:
        print("缺 L2 快照（先跑 decompile）或找不到 Ghidra（設 GHIDRA_HOME）", file=sys.stderr)
        return 1
    text = out_c.read_text(encoding="utf-8")
    failed = {m.group(2): m for m in FAILED_RE.finditer(text)}
    if args.only:
        failed = {a: m for a, m in failed.items() if f"FUN_{a}" in args.only or f"0x{a}" in args.only}
    if not failed:
        print("沒有反編譯失敗的函式")
        return 0
    redo = out_c.with_name("redecompiled.c")
    headless = ghidra / "support" / ("analyzeHeadless.bat" if sys.platform == "win32" else "analyzeHeadless")
    cmd = [str(headless), str(snap / "ghidra" / "project"), "df", "-process", EXE.name, "-noanalysis",
           "-scriptPath", str(ROOT / "scripts"), "-postScript", "GhidraRedecompile.java", str(redo), str(args.timeout),
           str(args.max_instructions), *(f"0x{a}" for a in failed)]
    env = dict(os.environ, GHIDRA_HEADLESS_MAXMEM=os.environ.get("GHIDRA_HEADLESS_MAXMEM", "32G"))
    print(f"重做 {len(failed)} 個函式（每個最多 {args.timeout} 秒）", flush=True)
    if subprocess.run(cmd, env=env).returncode != 0 or not redo.is_file():
        print("REDECOMPILE FAILED", file=sys.stderr)
        return 1
    bodies = {}
    for chunk in re.split(r"(?=// ============ \S+ @ 0x[0-9a-f]+ ============\n)", redo.read_text(encoding="utf-8")):
        m = re.match(r"// ============ \S+ @ 0x([0-9a-f]+) ============\n", chunk)
        if m and "// DECOMPILE FAILED" not in chunk[:m.end() + 40]:
            bodies[m.group(1)] = chunk
    merged = FAILED_RE.sub(lambda m: bodies.get(m.group(2), m.group(0)), text)
    out_c.write_text(merged, encoding="utf-8", newline="\n")
    redo.unlink()
    print(f"併回 {len(bodies)}／{len(failed)} 個函式 -> {out_c}")
    return 0


def cmd_status(_args) -> int:
    snaps = sorted(p for p in SNAPSHOTS.glob("*") if (p / "VERSION.txt").is_file()) if SNAPSHOTS.is_dir() else []
    for p in snaps:
        m = load_meta(p)
        l2 = "L2" if (p / "ghidra/Dwarf Fortress.exe.c").is_file() else "L1"
        print(f"{p.name}  {l2}  buildid={m['steam_buildid']}  dfhack={m['dfhack_version']}  sha={m['exe_sha256'][:12]}")
    if snaps and EXE.is_file():
        same = sha256(EXE) == load_meta(snaps[-1])["exe_sha256"]
        print("目前安裝的 exe 與最新快照", "相同" if same else "不同：遊戲已更新，先跑 snapshot")
    return 0


def cmd_where(_args) -> int:
    for k, v in (("repo", ROOT), ("reference", REF), ("DF", DF), ("DFHack", DFHACK), ("workshop", WORKSHOP), ("ghidra", find_ghidra())):
        print(f"{k:10} {v}")
    return 0


FUNC_MARK = "// ============ "


def decomp_file(version: str | None) -> Path | None:
    snap = find_snapshot(version)
    c = snap / "ghidra/Dwarf Fortress.exe.c" if snap else None
    if c is None or not c.is_file():
        print("缺 L2 快照（先跑 decompile）", file=sys.stderr)
        return None
    return c


def decomp_lines(c: Path):
    """逐行讀 pseudo-C，產生 (行號, 所在函式, 是否已進函式本體, 行)。
    函式以匯出時的「// ============ NAME @ 0x… ============」分段；本體從第 0 欄的 { 開始。"""
    func, body = "-", False
    with c.open(encoding="utf-8", errors="replace") as f:
        for n, line in enumerate(f, 1):
            if line.startswith(FUNC_MARK):
                func, body = line.split()[2], False
            elif line.startswith("{"):
                body = True
            yield n, func, body, line


def cmd_find(args) -> int:
    c = decomp_file(args.version)
    if c is None:
        return 1
    rx = re.compile(args.pattern)
    total = 0
    for n, func, _, line in decomp_lines(c):
        if rx.search(line):
            total += 1
            if total <= args.max:
                print(f"{n}\t{func}\t{line.strip()[:200]}")
    print(f"顯示 {min(total, args.max)}／共 {total} 筆")
    return 0


def cmd_fn(args) -> int:
    c = decomp_file(args.version)
    if c is None:
        return 1
    found = started = False
    shown = 0
    for n, func, body, line in decomp_lines(c):
        if func != args.name:
            if found:
                break
            continue
        found = True
        if line.startswith("// DECOMPILE FAILED"):
            print(f"{n}\t{line.rstrip()}")
            print(f"{args.name} 反編譯失敗，沒有本體（可用 redecompile --only {args.name} 重做）")
            return 1
        # 函式標頭是標記後第一個第 0 欄、非註解的行（參數多時會折行，續行有縮排）
        started = started or line[:1] not in " \t\r\n/"
        if not started:
            continue
        if shown == args.max_lines:
            print(f"…超過 {args.max_lines} 行，已截斷（用 --max-lines 調高）")
            break
        print(f"{n}\t{line.rstrip()}")
        shown += 1
        if body and line.startswith("}"):
            break
    if not found:
        print(f"找不到函式 {args.name}", file=sys.stderr)
        return 1
    return 0


def cmd_callers(args) -> int:
    c = decomp_file(args.version)
    if c is None:
        return 1
    rx = re.compile(rf"(?<!\w){re.escape(args.name)}\(")
    sites: dict[str, list[int]] = {}
    # 只算函式本體內的；標頭（定義本身）在 { 之前，不會算進來
    for n, func, body, line in decomp_lines(c):
        if body and args.name in line and rx.search(line):
            sites.setdefault(func, []).append(n)
    for func, ns in sites.items():
        print(f"{func}: {', '.join(map(str, ns))}")
    print(f"共 {sum(map(len, sites.values()))} 處呼叫，分布在 {len(sites)} 個函式")
    return 0


def main() -> int:
    ap = argparse.ArgumentParser(description=__doc__, formatter_class=argparse.RawDescriptionHelpFormatter)
    sub = ap.add_subparsers(dest="cmd", required=True)
    p = sub.add_parser("snapshot")
    p.add_argument("--version", help="DFHack 尚未支援此版時手動指定，如 53.17")
    p.add_argument("--date", help="快照日期 YYYYMMDD，預設今天")
    p.set_defaults(fn=cmd_snapshot)
    p = sub.add_parser("verify")
    p.add_argument("version", nargs="?")
    p.set_defaults(fn=cmd_verify)
    p = sub.add_parser("diff")
    p.add_argument("old")
    p.add_argument("new")
    p.set_defaults(fn=cmd_diff)
    p = sub.add_parser("decompile")
    p.add_argument("version", nargs="?")
    p.add_argument("--timeout", type=int, default=14400, help="Ghidra 自動分析逾時秒數")
    p.add_argument("--export-only", action="store_true", help="沿用已分析的專案，只重跑匯出")
    p.set_defaults(fn=cmd_decompile)
    p = sub.add_parser("redecompile")
    p.add_argument("version", nargs="?")
    p.add_argument("--timeout", type=int, default=3600, help="每個函式的反編譯逾時秒數")
    p.add_argument("--max-instructions", type=int, default=1_000_000,
                   help="每個函式最多處理的指令數（Flow exceeded maximum allowable instructions 時調高）")
    p.add_argument("--only", nargs="+", metavar="FUN", help="只重做這些函式（FUN_14xxxxxxx 或 0x14xxxxxxx），先做重要的")
    p.set_defaults(fn=cmd_redecompile)
    sub.add_parser("status").set_defaults(fn=cmd_status)
    sub.add_parser("where").set_defaults(fn=cmd_where)
    p = sub.add_parser("find", help="在 pseudo-C 逐行搜尋 regex")
    p.add_argument("pattern")
    p.add_argument("--max", type=int, default=50, help="最多顯示幾筆")
    p.add_argument("--version")
    p.set_defaults(fn=cmd_find)
    p = sub.add_parser("fn", help="印出整個函式")
    p.add_argument("name")
    p.add_argument("--max-lines", type=int, default=3000)
    p.add_argument("--version")
    p.set_defaults(fn=cmd_fn)
    p = sub.add_parser("callers", help="列出呼叫 NAME( 的位置")
    p.add_argument("name")
    p.add_argument("--version")
    p.set_defaults(fn=cmd_callers)
    args = ap.parse_args()
    return args.fn(args)


if __name__ == "__main__":
    sys.exit(main())
