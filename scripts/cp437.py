"""DF 使用的 CP437：0x01–0x1F、0x7F 是圖形字元（☺、♀…），不是控制字元。

Python 端（build.py、verify_mod.py）從這裡取用；engine/src/cp437.rs 由本檔產生：
  uv run scripts/cp437.py   # 重寫 engine/src/cp437.rs
"""
from pathlib import Path

GRAPHICS = "☺☻♥♦♣♠•◘○◙♂♀♪♫☼►◄↕‼¶§▬↨↑↓→←∟↔▲▼"
TABLE = ["\0", *GRAPHICS, *map(chr, range(0x20, 0x7F)), "⌂", *bytes(range(0x80, 0x100)).decode("cp437")]
assert len(TABLE) == 256 and len(set(TABLE)) == 256
BYTE_OF = {c: i for i, c in enumerate(TABLE) if i}


def encodable(text: str) -> bool:
    return all(c in BYTE_OF for c in text)


def cells(text: str) -> int:
    """遊戲畫面上佔的格數：CP437 字一格，其他字（中文、全形標點）由引擎畫成兩格。"""
    return sum(1 if c in BYTE_OF else 2 for c in text)


def rust_source() -> str:
    def lit(c: str) -> str:
        return {"\0": r"'\0'", "'": r"'\''", "\\": r"'\\'", "\xa0": r"'\u{a0}'"}.get(c, f"'{c}'")

    rows = "\n".join("    " + ", ".join(lit(c) for c in TABLE[i:i + 16]) + "," for i in range(0, 256, 16))
    return f"""//! DF 使用的 CP437 字碼頁（0x01–0x1F、0x7F 是圖形字元，不是控制字元）。
//! 由 scripts/cp437.py 產生，改表請改那邊再重跑。

use std::collections::HashMap;
use std::sync::LazyLock;

pub const TABLE: [char; 256] = [
{rows}
];

static REVERSE: LazyLock<HashMap<char, u8>> =
    LazyLock::new(|| TABLE.iter().enumerate().skip(1).map(|(i, &c)| (c, i as u8)).collect());

/// Unicode 字元對應的 CP437 位元組；0x00 與表外的字回傳 None。
pub fn byte_of(c: char) -> Option<u8> {{
    if (' '..='~').contains(&c) {{
        return Some(c as u8);
    }}
    REVERSE.get(&c).copied()
}}

/// 整串轉成 CP437；有任何字不在表內就回傳 None。
pub fn encode(s: &str) -> Option<Vec<u8>> {{
    s.chars().map(byte_of).collect()
}}

pub fn decode(bytes: &[u8]) -> String {{
    bytes.iter().map(|&b| TABLE[b as usize]).collect()
}}
"""


if __name__ == "__main__":
    out = Path(__file__).resolve().parents[1] / "engine/src/cp437.rs"
    out.write_text(rust_source(), encoding="utf-8", newline="\n")
    print(f"-> {out}")
