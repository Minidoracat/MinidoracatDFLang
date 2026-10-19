//! DF 使用的 CP437 字碼頁（0x01–0x1F、0x7F 是圖形字元，不是控制字元）。
//! 由 scripts/cp437.py 產生，改表請改那邊再重跑。

use std::collections::HashMap;
use std::sync::LazyLock;

pub const TABLE: [char; 256] = [
    '\0', '☺', '☻', '♥', '♦', '♣', '♠', '•', '◘', '○', '◙', '♂', '♀', '♪', '♫', '☼',
    '►', '◄', '↕', '‼', '¶', '§', '▬', '↨', '↑', '↓', '→', '←', '∟', '↔', '▲', '▼',
    ' ', '!', '"', '#', '$', '%', '&', '\'', '(', ')', '*', '+', ',', '-', '.', '/',
    '0', '1', '2', '3', '4', '5', '6', '7', '8', '9', ':', ';', '<', '=', '>', '?',
    '@', 'A', 'B', 'C', 'D', 'E', 'F', 'G', 'H', 'I', 'J', 'K', 'L', 'M', 'N', 'O',
    'P', 'Q', 'R', 'S', 'T', 'U', 'V', 'W', 'X', 'Y', 'Z', '[', '\\', ']', '^', '_',
    '`', 'a', 'b', 'c', 'd', 'e', 'f', 'g', 'h', 'i', 'j', 'k', 'l', 'm', 'n', 'o',
    'p', 'q', 'r', 's', 't', 'u', 'v', 'w', 'x', 'y', 'z', '{', '|', '}', '~', '⌂',
    'Ç', 'ü', 'é', 'â', 'ä', 'à', 'å', 'ç', 'ê', 'ë', 'è', 'ï', 'î', 'ì', 'Ä', 'Å',
    'É', 'æ', 'Æ', 'ô', 'ö', 'ò', 'û', 'ù', 'ÿ', 'Ö', 'Ü', '¢', '£', '¥', '₧', 'ƒ',
    'á', 'í', 'ó', 'ú', 'ñ', 'Ñ', 'ª', 'º', '¿', '⌐', '¬', '½', '¼', '¡', '«', '»',
    '░', '▒', '▓', '│', '┤', '╡', '╢', '╖', '╕', '╣', '║', '╗', '╝', '╜', '╛', '┐',
    '└', '┴', '┬', '├', '─', '┼', '╞', '╟', '╚', '╔', '╩', '╦', '╠', '═', '╬', '╧',
    '╨', '╤', '╥', '╙', '╘', '╒', '╓', '╫', '╪', '┘', '┌', '█', '▄', '▌', '▐', '▀',
    'α', 'ß', 'Γ', 'π', 'Σ', 'σ', 'µ', 'τ', 'Φ', 'Θ', 'Ω', 'δ', '∞', 'φ', 'ε', '∩',
    '≡', '±', '≥', '≤', '⌠', '⌡', '÷', '≈', '°', '∙', '·', '√', 'ⁿ', '²', '■', '\u{a0}',
];

static REVERSE: LazyLock<HashMap<char, u8>> =
    LazyLock::new(|| TABLE.iter().enumerate().skip(1).map(|(i, &c)| (c, i as u8)).collect());

/// Unicode 字元對應的 CP437 位元組；0x00 與表外的字回傳 None。
pub fn byte_of(c: char) -> Option<u8> {
    if (' '..='~').contains(&c) {
        return Some(c as u8);
    }
    REVERSE.get(&c).copied()
}

/// 整串轉成 CP437；有任何字不在表內就回傳 None。
pub fn encode(s: &str) -> Option<Vec<u8>> {
    s.chars().map(byte_of).collect()
}

/// Lua 傳來的原文（`engine.translate`）換成查字典用的 CP437：UTF-8、每個字都在表內的（`dflang t`、`dfhack.df2utf` 轉過的）照字換；
/// 其他的照遊戲原本的 CP437 位元組收（名字有 ö 的 0x94 不成 UTF-8，♂ 的 0x0B 是 UTF-8 的控制字元、不在表內）
pub fn from_lua(bytes: &[u8]) -> Vec<u8> {
    std::str::from_utf8(bytes).ok().and_then(encode).unwrap_or_else(|| bytes.to_vec())
}

pub fn decode(bytes: &[u8]) -> String {
    bytes.iter().map(|&b| TABLE[b as usize]).collect()
}

#[cfg(test)]
mod tests {
    #[test]
    fn lua_text_is_looked_up_the_same_whether_utf8_or_the_games_cp437() {
        // 遊戲的字串直接傳進來，和 dfhack.df2utf 轉過的查同一串：「Mörul」的 ö 是 0x94，「Cats, ♂」的 ♂ 是 0x0B
        for (utf8, cp437) in [("Mörul", &b"M\x94rul"[..]), ("Cats, ♂", &b"Cats, \x0b"[..])] {
            assert_eq!(super::from_lua(utf8.as_bytes()), cp437);
            assert_eq!(super::from_lua(cp437), cp437);
        }
    }
}
