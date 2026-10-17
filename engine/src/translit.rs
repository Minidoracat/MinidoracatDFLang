//! 原生語言名字的音譯（設定視窗的「原生語言的名字音譯」，隨時可開關，見 set_on）。
//!
//! 原名（矮人語、精靈語、哥布林語、人類語的字：Urist、Rigòth Nildum）沒有中文字義可查，照拼法切成音節再查音譯表
//! （data/translit/translit.tsv）：子音（th、sh 這類雙字母算一個）接母音是一節；母音後面的 n（或 ng）不接母音時
//! 併進同一節（kogan → ko＋gan）；接不到母音的子音自成一節。y 後面接母音是子音，否則當 i；帶符號的母音（ò、â、ÿ）照基本母音，
//! 連續兩個相同的子音只算一個。名字的每個字各自音譯、以「·」相接；綽號（`Bob'）是玩家取的，照原文。
//! 名字裡有一節查不到（MOD 加的語言用了表上沒有的拼法）就整個名字照原文。

use std::collections::HashMap;
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};

use parking_lot::RwLock;

use crate::cp437;

/// 音節（小寫 ASCII，如 ba、ban、th）→ 中文
pub type Table = HashMap<Vec<u8>, String>;

/// 目前語言的音譯表（切換語言時換一份，見 lib.rs 的 load_translit）
static TABLE: RwLock<Option<Arc<Table>>> = RwLock::new(None);
/// 名字音譯開著（設定視窗的選項）
static ON: AtomicBool = AtomicBool::new(false);

/// 讀 `音節<TAB>中文` 一行一條。
pub fn parse(content: &str) -> Table {
    content
        .lines()
        .filter_map(|line| {
            let (key, zh) = line.split_once('\t')?;
            (!key.is_empty() && !zh.is_empty()).then(|| (key.as_bytes().to_vec(), zh.to_owned()))
        })
        .collect()
}

/// 換成這一份音譯表（目前語言的）
pub fn set_table(table: Arc<Table>) {
    *TABLE.write() = Some(table);
}

/// 名字音譯開關
pub fn set_on(on: bool) {
    ON.store(on, Ordering::Release);
}

/// 音譯表（選項關著、或還沒載入時是 None）
pub fn table() -> Option<Arc<Table>> {
    if !ON.load(Ordering::Acquire) {
        return None;
    }
    TABLE.read().clone()
}

/// 原文名字（CP437，以空白隔開的幾個字）的音譯；選項關著、或有字譯不出來時回傳 None。
pub fn name(native: &[u8]) -> Option<String> {
    let table = table()?;
    name_with(&table, native)
}

/// 同 name，音譯表由呼叫端給（測試用，免得動到全域的表）。
pub fn name_with(table: &Table, native: &[u8]) -> Option<String> {
    let mut out = String::new();
    // 上一段是音譯的字：字與字之間接「·」，和綽號之間空一格
    let mut after_word = false;
    let mut tokens = native.split(|&c| c == b' ').filter(|t| !t.is_empty());
    while let Some(token) = tokens.next() {
        if let Some(&open) = token.first().filter(|c| b"`'\"".contains(c)) {
            let close = if open == b'`' { b'\'' } else { open };
            let mut nick = token.to_vec();
            while nick.len() < 2 || nick.last() != Some(&close) {
                let Some(next) = tokens.next() else { break };
                nick.push(b' ');
                nick.extend_from_slice(next);
            }
            if !out.is_empty() {
                out.push(' ');
            }
            out.push_str(&cp437::decode(&nick));
            after_word = false;
            continue;
        }
        let zh = word(table, token)?;
        if !out.is_empty() {
            out.push(if after_word { '·' } else { ' ' });
        }
        out.push_str(&zh);
        after_word = true;
    }
    (!out.is_empty()).then_some(out)
}

/// 一個字的音譯；有字母以外的字、或有音節不在表上時回傳 None。
fn word(table: &Table, token: &[u8]) -> Option<String> {
    let letters = fold(token)?;
    let mut out = String::new();
    for unit in syllables(&letters) {
        out.push_str(table.get(&unit)?);
    }
    (!out.is_empty()).then_some(out)
}

/// 小寫 ASCII 字母：帶符號的母音照基本母音，連續兩個相同的子音只算一個（tth 也就成了 th）。
fn fold(token: &[u8]) -> Option<Vec<u8>> {
    let mut out: Vec<u8> = Vec::with_capacity(token.len());
    for c in cp437::decode(token).chars() {
        let base = match c {
            'a'..='z' => c,
            'A'..='Z' => c.to_ascii_lowercase(),
            'â' | 'ä' | 'à' | 'å' | 'á' | 'Ä' | 'Å' => 'a',
            'é' | 'ê' | 'ë' | 'è' | 'É' => 'e',
            'ï' | 'î' | 'ì' | 'í' => 'i',
            'ô' | 'ö' | 'ò' | 'ó' | 'Ö' => 'o',
            'ü' | 'û' | 'ù' | 'ú' | 'Ü' => 'u',
            'ÿ' => 'y',
            'ñ' | 'Ñ' => 'n',
            'ç' | 'Ç' => 'c',
            'æ' | 'Æ' => {
                out.extend_from_slice(b"ae");
                continue;
            }
            _ => return None,
        } as u8;
        if out.last() == Some(&base) && !b"aeiou".contains(&base) {
            continue;
        }
        out.push(base);
    }
    Some(out)
}

/// w[i] 是母音：a e i o u；y 後面接母音時是子音，否則當母音
fn vowel(w: &[u8], i: usize) -> bool {
    match w.get(i) {
        Some(b'a' | b'e' | b'i' | b'o' | b'u') => true,
        Some(b'y') => !w.get(i + 1).is_some_and(|c| b"aeiou".contains(c)),
        _ => false,
    }
}

const DIGRAPHS: [&[u8]; 7] = [b"th", b"sh", b"ch", b"kh", b"gh", b"zh", b"ph"];

/// 切成音節（查表的鍵）：子音＋母音（＋不接母音的 n、ng）、單獨的母音、接不到母音的子音。
fn syllables(w: &[u8]) -> Vec<Vec<u8>> {
    let mut out = Vec::new();
    let mut i = 0;
    while i < w.len() {
        let mut unit = Vec::new();
        if !vowel(w, i) {
            // qu 接母音是一個子音（qua、que），其餘照雙字母表
            let len = if w[i..].starts_with(b"qu") && vowel(w, i + 2) {
                2
            } else {
                DIGRAPHS.iter().find(|d| w[i..].starts_with(d)).map_or(1, |d| d.len())
            };
            unit.extend_from_slice(&w[i..i + len]);
            i += len;
            if !vowel(w, i) {
                out.push(unit);
                continue;
            }
        }
        unit.push(if w[i] == b'y' { b'i' } else { w[i] });
        i += 1;
        if w.get(i) == Some(&b'n') {
            let mut j = i + 1;
            if w.get(j) == Some(&b'g') && !vowel(w, j + 1) {
                j += 1;
            }
            if !vowel(w, j) {
                unit.push(b'n');
                i = j;
            }
        }
        out.push(unit);
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    fn keys(w: &str) -> Vec<String> {
        syllables(&fold(&cp437::encode(w).unwrap()).unwrap()).into_iter().map(|u| String::from_utf8(u).unwrap()).collect()
    }

    #[test]
    fn syllables_follow_spelling() {
        assert_eq!(keys("Rigòth"), ["ri", "go", "th"], "帶符號的母音照基本母音，th 是一個子音");
        assert_eq!(keys("Kogan"), ["ko", "gan"], "n 不接母音時併進前一節");
        assert_eq!(keys("Ngozong"), ["n", "go", "zon"], "字尾的 ng 也併進前一節；字首的 n 接子音自成一節");
        assert_eq!(keys("Onsmost"), ["on", "s", "mo", "s", "t"]);
        assert_eq!(keys("Oddet"), ["o", "de", "t"], "連續兩個相同的子音只算一個");
        assert_eq!(keys("Eyo"), ["e", "yo"], "y 接母音是子音");
        assert_eq!(keys("Zÿn"), ["zin"], "y 不接母音時當 i");
        assert_eq!(keys("Quel"), ["que", "l"], "qu 接母音是一個子音");
        assert_eq!(keys("Anu"), ["a", "nu"], "n 接母音時是下一節的開頭");
    }

    #[test]
    fn names_join_words_and_keep_nicknames() {
        let table: Table = [("ri", "里"), ("go", "戈"), ("th", "斯"), ("ni", "尼"), ("l", "爾"), ("du", "杜"), ("m", "姆")]
            .into_iter()
            .map(|(k, v)| (k.as_bytes().to_vec(), v.to_owned()))
            .collect();
        let name = |s: &str| name_with(&table, &cp437::encode(s).unwrap());
        assert_eq!(name("Rigòth Nildum").as_deref(), Some("里戈斯·尼爾杜姆"));
        assert_eq!(name("Rigòth `Big Bob' Nildum").as_deref(), Some("里戈斯 `Big Bob' 尼爾杜姆"), "綽號照原文，前後空一格");
        assert_eq!(name("Rigòth Urist"), None, "有音節查不到就整個名字照原文");
        assert_eq!(name("Rigòth-Nildum"), None, "字母以外的字不音譯");
    }
}
