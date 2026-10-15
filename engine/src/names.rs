//! 世界生成的英文名稱（世界、地區、據點、文明、團體、神器、書名、人物的英文姓氏與稱號）。
//!
//! 遊戲用 translate_name 把 language_name 轉成英文：原名（矮人語等）＋綽號＋ words[0]+words[1] 接成的一個字
//! （Scorchaxe）＋「the words[2] words[3] words[4]-words[5]」＋「of words[6]」，字取自 raws 的字表
//! （world.raws.language.words）。hook 讓原函式照跑、英文照用（版面、寬度、存檔都不動），另外依同樣的結構
//! 組出中文名，登記「英文 → 中文」；字典查不到一段英文時（整句、佔位抓到的片段、傳說頁的連結字）再查這張表。
//! 組中文時也照遊戲的規則拼一次英文，和原函式的輸出（不分大小寫）對得上才登記：遊戲改版或 MOD 改了字表時寧可照英文。

use std::collections::HashMap;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, LazyLock, OnceLock};

use parking_lot::Mutex;

use crate::cp437;

/// language_name／language_word 的欄位位移與字表位址，由 Lua 依 DFHack 的結構定義傳入。
#[derive(Clone, Copy)]
pub struct NameLayout {
    /// language_name.words（int32[7]）
    pub words: usize,
    /// language_name.parts_of_speech（int16[7]）
    pub parts: usize,
    /// world.raws.language.words（std::vector<language_word*>）的位址
    pub word_vec: usize,
    /// language_word.word（字的代號，std::string）
    pub word_token: usize,
    /// language_word.forms（std::string[9]，依詞性排）
    pub word_forms: usize,
}

pub static NAME_LAYOUT: OnceLock<NameLayout> = OnceLock::new();

/// 字表：字的代號 → 名詞、形容詞、字首、動詞的中文（空字串表示沒有這個詞性）
static WORDS: OnceLock<HashMap<Vec<u8>, [String; 4]>> = OnceLock::new();
/// 書名的隨機字（[NOUN]、[ADJ]，見 Dict 的 `{&n}`、`{=n}`）：英文詞形（小寫）→ 名詞、形容詞的中文；同一詞形有好幾個字時用先讀到的
static FORMS: OnceLock<[HashMap<Vec<u8>, String>; 2]> = OnceLock::new();

/// 英文名 → 中文名。ponytail: 滿了整張清掉，畫面上的名字下次遊戲再組名字時補回來
static NAMES: LazyLock<Mutex<HashMap<Vec<u8>, Arc<str>>>> = LazyLock::new(|| Mutex::new(HashMap::new()));
const NAMES_LIMIT: usize = 200_000;
/// 每登記一個新名字加一：字典記成翻不出來的字串，在這之後可能翻得出來了（見 Dict::rule_entry）
static GENERATION: AtomicU64 = AtomicU64::new(0);

/// 讀 `代號<TAB>詞性<TAB>中文[<TAB>英文詞形]` 一行一條（詞性 NOUN／ADJ／PREFIX／VERB；英文詞形以 / 分隔，名詞是單數/複數）；
/// 回傳載入條數。只能載入一次。
pub fn load_words(content: &str) -> usize {
    let mut table: HashMap<Vec<u8>, [String; 4]> = HashMap::new();
    let mut forms: [HashMap<Vec<u8>, String>; 2] = Default::default();
    let mut n = 0;
    for line in content.lines() {
        let mut cols = line.split('\t');
        let (Some(token), Some(pos), Some(zh)) = (cols.next(), cols.next(), cols.next()) else { continue };
        let Some(i) = ["NOUN", "ADJ", "PREFIX", "VERB"].iter().position(|p| *p == pos) else { continue };
        if zh.is_empty() {
            continue;
        }
        table.entry(token.as_bytes().to_vec()).or_default()[i] = zh.to_owned();
        if let (Some(en), Some(class)) = (cols.next(), forms.get_mut(i)) {
            for form in en.split('/').filter(|f| !f.is_empty()) {
                class.entry(form.to_ascii_lowercase().into_bytes()).or_insert_with(|| zh.to_owned());
            }
        }
        n += 1;
    }
    let _ = WORDS.set(table);
    let _ = FORMS.set(forms);
    n
}

/// 書名的隨機字：名詞（單數或複數）或形容詞的英文詞形的中文。遊戲把書名的隨機字每個字改大寫開頭（FUN_14052d3c0），
/// 小寫開頭的不收（介面字串 `good food`、`new moon` 不會被書名樣板 `{=1} {$2}` 比走），其餘字母不分大小寫。
pub fn word_form(form: &[u8], adj: bool) -> Option<&'static str> {
    if form.len() > 64 || form.contains(&b' ') || !form.first().is_some_and(u8::is_ascii_uppercase) {
        return None;
    }
    let mut buf = [0u8; 64];
    let lower = &mut buf[..form.len()];
    lower.copy_from_slice(form);
    lower.make_ascii_lowercase();
    FORMS.get()?[usize::from(adj)].get(&*lower).map(String::as_str)
}

/// 測試用：只設書名的隨機字（字表 WORDS 給名稱的測試設）。
#[cfg(test)]
pub fn set_forms(nouns: &[(&str, &str)], adjs: &[(&str, &str)]) {
    let map = |v: &[(&str, &str)]| v.iter().map(|(en, zh)| (en.as_bytes().to_vec(), (*zh).to_owned())).collect();
    let _ = FORMS.set([map(nouns), map(adjs)]);
}

/// 字典查詞用：遊戲組過的英文名的中文。
pub fn get(english: &[u8]) -> Option<Arc<str>> {
    NAMES.lock().get(english).cloned()
}

pub fn generation() -> u64 {
    GENERATION.load(Ordering::Acquire)
}

/// 測試用：直接登記一個名字（不經 translate_name）。
#[cfg(test)]
pub fn insert(english: &[u8], zh: &str) {
    NAMES.lock().insert(english.to_vec(), zh.into());
    GENERATION.fetch_add(1, Ordering::Release);
}

unsafe fn read<T: Copy>(base: usize, offset: usize) -> T {
    unsafe { *((base + offset) as *const T) }
}

/// MSVC std::string：16 bytes 內建緩衝區（或長字串指標）、長度、容量。
unsafe fn str_bytes<'a>(s: usize) -> &'a [u8] {
    unsafe {
        let (size, cap): (usize, usize) = (read(s, 16), read(s, 24));
        let data = if cap > 15 { read::<*const u8>(s, 0) } else { s as *const u8 };
        std::slice::from_raw_parts(data, size)
    }
}

/// 名字裡的一個字：(英文詞形, 中文)。parts_of_speech：0 名詞、1 名詞複數、2 形容詞、3 字首、4–8 動詞各形。
unsafe fn word<'a>(l: &NameLayout, id: i32, pos: i16) -> Option<(&'a [u8], &'static str)> {
    let class = match pos {
        0 | 1 => 0,
        2 => 1,
        3 => 2,
        4..=8 => 3,
        _ => return None,
    };
    unsafe {
        let (begin, end): (usize, usize) = (read(l.word_vec, 0), read(l.word_vec, 8));
        let id = usize::try_from(id).ok().filter(|&i| i < (end - begin) / 8)?;
        let w: usize = read(begin, id * 8);
        let form = str_bytes(w + l.word_forms + pos as usize * 32);
        let zh = WORDS.get()?.get(str_bytes(w + l.word_token))?[class].as_str();
        (!zh.is_empty()).then_some((form, zh))
    }
}

/// 照遊戲的組法拼出英文名的字表部分（不含原名與綽號，小寫）與中文；有字缺中文時回傳 None。
/// 遊戲的組法（translate_name）：words[0]+words[1] 接成一個字；words[2]、words[3]、words[4] 各自前面空一格，
/// 第一個前面接 the，words[4] 後面接「-」再直接接 words[5]；有 words[6] 時接「of words[6]」。
unsafe fn compose(l: &NameLayout, name: usize) -> Option<(Vec<u8>, String)> {
    let (ids, pos): ([i32; 7], [i16; 7]) = unsafe { (read(name, l.words), read(name, l.parts)) };
    let mut words: [Option<(&[u8], &str)>; 7] = [None; 7];
    for i in 0..7 {
        if ids[i] != -1 {
            words[i] = Some(unsafe { word(l, ids[i], pos[i]) }?);
        }
    }
    let mut en: Vec<u8> = Vec::new();
    let space = |en: &mut Vec<u8>| {
        if !en.is_empty() && !en.ends_with(b"-") {
            en.push(b' ');
        }
    };
    // 複合字
    let compound: String = words[0..2].iter().flatten().map(|w| w.1).collect();
    if words[0..2].iter().any(Option::is_some) {
        en.extend(words[0..2].iter().flatten().flat_map(|w| w.0));
    }
    // the 片語（words[2..6]）：遊戲只在 words[2]、words[3]、words[5] 有字時才拼
    let mut phrase = String::new();
    if words[2].is_some() || words[3].is_some() || words[5].is_some() {
        let mut the = false;
        for (i, w) in words.iter().enumerate().take(6).skip(2) {
            let Some((form, zh)) = w else { continue };
            space(&mut en);
            if !std::mem::replace(&mut the, true) {
                en.extend_from_slice(b"the ");
            }
            en.extend_from_slice(form);
            if i == 4 {
                en.push(b'-');
            }
            phrase.push_str(zh);
        }
    }
    // of 片語
    let of = words[6].map(|(form, zh)| {
        space(&mut en);
        en.extend_from_slice(b"of ");
        en.extend_from_slice(form);
        zh
    });
    if en.is_empty() {
        return None;
    }
    en.make_ascii_lowercase();
    let rest = match of {
        Some(of) if phrase.is_empty() => of.to_owned(),
        Some(of) => format!("{of}之{phrase}"),
        None => phrase,
    };
    let zh = match (compound.is_empty(), rest.is_empty()) {
        (false, false) => format!("{compound}·{rest}"),
        (false, true) => compound,
        _ => rest,
    };
    Some((en, zh))
}

/// hook 用：原函式把 name 轉成英文 out 之後呼叫，登記這個英文名的中文。
/// 開頭的原名、綽號照原文：英文輸出扣掉字表部分剩下的就是這段，中文名接在它後面、空一格。
pub unsafe fn note(name: usize, out: &[u8]) {
    let Some(l) = NAME_LAYOUT.get() else { return };
    if out.is_empty() || NAMES.lock().contains_key(out) {
        return;
    }
    let Some((en, zh)) = (unsafe { compose(l, name) }) else { return };
    if out.len() < en.len() || !out[out.len() - en.len()..].eq_ignore_ascii_case(&en) {
        return;
    }
    let head = &out[..out.len() - en.len()];
    let zh = match head.strip_suffix(b" ") {
        Some(head) => format!("{} {zh}", cp437::decode(head)),
        None if head.is_empty() => zh,
        None => return,
    };
    let mut names = NAMES.lock();
    if names.len() >= NAMES_LIMIT {
        names.clear();
    }
    names.insert(out.to_vec(), zh.into());
    GENERATION.fetch_add(1, Ordering::Release);
}

#[cfg(test)]
mod tests {
    use super::*;

    /// 假的字表：每個字是 (代號, 九個詞形)；欄位只是讓記憶體活到測試結束
    struct Fake {
        _vec: Box<[usize; 3]>,
        _words: Vec<Box<[u64; 4 + 9 * 4]>>,
        _ptrs: Vec<usize>,
    }

    unsafe fn put_str(at: usize, s: &str) {
        assert!(s.len() <= 15, "測試只用內建緩衝區");
        unsafe {
            std::ptr::copy_nonoverlapping(s.as_ptr(), at as *mut u8, s.len());
            *((at + 16) as *mut usize) = s.len();
            *((at + 24) as *mut usize) = 15;
        }
    }

    impl Fake {
        fn new(entries: &[(&str, [&str; 9])]) -> Fake {
            let words: Vec<Box<[u64; 40]>> = entries
                .iter()
                .map(|(token, forms)| {
                    let w = Box::new([0u64; 40]);
                    let base = w.as_ptr() as usize;
                    unsafe {
                        put_str(base, token);
                        for (i, f) in forms.iter().enumerate() {
                            put_str(base + 32 + i * 32, f);
                        }
                    }
                    w
                })
                .collect();
            let ptrs: Vec<usize> = words.iter().map(|w| w.as_ptr() as usize).collect();
            let vec = Box::new([ptrs.as_ptr() as usize, ptrs.as_ptr() as usize + ptrs.len() * 8, 0]);
            let _ = NAME_LAYOUT.set(NameLayout { words: 0, parts: 28, word_vec: vec.as_ptr() as usize, word_token: 0, word_forms: 32 });
            Fake { _vec: vec, _words: words, _ptrs: ptrs }
        }
    }

    /// language_name 的 words 與 parts_of_speech（測試的位移：words 0、parts 28）
    #[repr(C)]
    struct Name {
        words: [i32; 7],
        parts: [i16; 7],
    }

    fn name(words: [i32; 7], parts: [i16; 7]) -> Name {
        Name { words, parts }
    }

    const N: [&str; 9] = ["", "", "", "", "", "", "", "", ""];

    #[test]
    fn names_follow_the_game_structure_and_keep_native_parts() {
        let _ = WORDS.set(
            [("SCORCH", 3, "焦"), ("AXE", 0, "斧"), ("PLANET", 0, "星"), ("DESTINY", 0, "命運"), ("BRONZE", 1, "青銅"), ("GUILD", 0, "公會")]
                .into_iter()
                .fold(HashMap::new(), |mut m: HashMap<Vec<u8>, [String; 4]>, (t, i, zh)| {
                    m.entry(t.as_bytes().to_vec()).or_default()[i] = zh.to_owned();
                    m
                }),
        );
        let mut scorch = N;
        scorch[4] = "scorch";
        let f = Fake::new(&[
            ("SCORCH", scorch),
            ("AXE", ["axe", "axes", "", "", "", "", "", "", ""]),
            ("PLANET", ["planet", "planets", "", "", "", "", "", "", ""]),
            ("DESTINY", ["destiny", "destinies", "", "", "", "", "", "", ""]),
            ("BRONZE", ["", "", "bronze", "", "", "", "", "", ""]),
            ("GUILD", ["guild", "guilds", "", "", "", "", "", "", ""]),
            ("ROOT", ["root", "roots", "", "", "", "", "", "", ""]),
        ]);
        unsafe {
            // 複合字：SCORCH（動詞）＋AXE；據點名前面沒有原名，堡壘名「原名, "英文"」由句型處理
            let n = name([0, 1, -1, -1, -1, -1, -1], [4, 0, 0, 0, 0, 0, 0]);
            note(&raw const n as usize, b"Scorchaxe");
            assert_eq!(get(b"Scorchaxe").as_deref(), Some("焦斧"));
            // The 片語＋of：「命運之星」
            let n = name([-1, -1, -1, -1, -1, 2, 3], [0, 0, 0, 0, 0, 0, 0]);
            note(&raw const n as usize, b"The Planet of Destiny");
            assert_eq!(get(b"The Planet of Destiny").as_deref(), Some("命運之星"));
            // 人物：原名照原文、空一格接中文
            let n = name([-1, -1, 4, -1, -1, 5, -1], [0, 0, 2, 0, 0, 0, 0]);
            note(&raw const n as usize, b"Urist the Bronze Guild");
            assert_eq!(get(b"Urist the Bronze Guild").as_deref(), Some("Urist 青銅公會"));
            // 字表缺中文（ROOT 沒有譯名）、或英文和遊戲輸出對不上：不登記
            let n = name([0, 6, -1, -1, -1, -1, -1], [4, 0, 0, 0, 0, 0, 0]);
            note(&raw const n as usize, b"Scorchroot");
            assert_eq!(get(b"Scorchroot"), None);
            let n = name([0, 1, -1, -1, -1, -1, -1], [4, 0, 0, 0, 0, 0, 0]);
            note(&raw const n as usize, b"Burnaxe");
            assert_eq!(get(b"Burnaxe"), None);
        }
        drop(f);
    }
}
