//! 世界生成的英文名稱（世界、地區、據點、文明、團體、神器、書名、人物的英文姓氏與稱號）。
//!
//! 遊戲用 translate_name 把 language_name 轉成英文：原名（矮人語等）＋綽號＋ words[0]+words[1] 接成的一個字
//! （Scorchaxe）＋「the words[2] words[3] words[4]-words[5]」＋「of words[6]」，字取自 raws 的字表
//! （world.raws.language.words）。hook 讓原函式照跑、英文照用（版面、寬度、存檔都不動），另外依同樣的結構
//! 組出中文名，登記「英文 → 中文」；字典查不到一段英文時（整句、佔位抓到的片段、傳說頁的連結字）再查這張表。
//! 組中文時也照遊戲的規則拼一次英文，和原函式的輸出（不分大小寫）對得上才登記：遊戲改版或 MOD 改了字表時寧可照英文。
//!
//! 原名照原文；名字音譯開著時（見 translit.rs）英文名開頭的原名也音譯，另外攔組原名的函式（native_name），
//! 把整個原名（「Rigòth Nildum」）也登記起來。開頭的原名和整個原名都照原文存，查的時候才依名字音譯的開關換成音譯，
//! 遊戲中途切換選項不必等遊戲再組一次名字。

use std::collections::{HashMap, HashSet};
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, LazyLock, OnceLock};

use parking_lot::{Mutex, RwLock};

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
type Words = HashMap<Vec<u8>, [String; 4]>;
/// 英文詞形（小寫）→ 中文，名詞、形容詞、字首、動詞各一張；同一詞形有好幾個字時用先讀到的
type Forms = [HashMap<Vec<u8>, String>; 4];

/// 一種語言的名稱字表（names.tsv）
pub struct WordTable {
    words: Arc<Words>,
    /// 書名的隨機字（[NOUN]、[ADJ]，見 Dict 的 `{&n}`、`{=n}`）與拆英文名（見 parse）用
    forms: Arc<Forms>,
}

impl WordTable {
    /// 字數（status、載入紀錄用）
    pub fn word_count(&self) -> usize {
        self.words.len()
    }
}

/// 目前語言的字表與英文詞形（切換語言時換一份，見 lib.rs 的 load_names）
static WORDS: RwLock<Option<Arc<Words>>> = RwLock::new(None);
static FORMS: RwLock<Option<Arc<Forms>>> = RwLock::new(None);

/// 名字裡的一個字：(位置 0～6，詞性類別：名詞、形容詞、字首、動詞，字的代號)。中文查的時候才照目前語言的字表組（見 chinese）
type Word = (u8, u8, Box<[u8]>);

/// 遊戲組過的英文名：開頭照原文的原名、綽號（head，不含結尾的空白；沒有原名時是空的）、字表的字，
/// 以及遊戲是不是只輸出了第一段（複合字，見 compose）。存字的代號不存中文，切換語言後照新的字表組
#[derive(Clone)]
struct Name {
    head: Box<[u8]>,
    words: Box<[Word]>,
    first: bool,
}

/// 英文名 → 中文名。ponytail: 滿了整張清掉，畫面上的名字下次遊戲再組名字時補回來
static NAMES: LazyLock<Mutex<HashMap<Vec<u8>, Name>>> = LazyLock::new(|| Mutex::new(HashMap::new()));
const NAMES_LIMIT: usize = 200_000;
/// 遊戲組過的原名（整個原名，和只有名的寫法）：名字音譯開著時查得到音譯（見 get）。關著時也照記，打開選項就有
static NATIVES: LazyLock<Mutex<HashSet<Vec<u8>>>> = LazyLock::new(|| Mutex::new(HashSet::new()));
/// 每登記一個新名字加一：字典記成翻不出來的字串，在這之後可能翻得出來了（見 Dict::rule_entry）
static GENERATION: AtomicU64 = AtomicU64::new(0);

/// 讀 `代號<TAB>詞性<TAB>中文[<TAB>英文詞形]` 一行一條（詞性 NOUN／ADJ／PREFIX／VERB；英文詞形以 / 分隔，名詞是單數/複數）。
pub fn parse_table(content: &str) -> WordTable {
    let mut words: Words = HashMap::new();
    let mut forms: Forms = Default::default();
    for line in content.lines() {
        let mut cols = line.split('\t');
        let (Some(token), Some(pos), Some(zh)) = (cols.next(), cols.next(), cols.next()) else { continue };
        let Some(i) = ["NOUN", "ADJ", "PREFIX", "VERB"].iter().position(|p| *p == pos) else { continue };
        if zh.is_empty() {
            continue;
        }
        words.entry(token.as_bytes().to_vec()).or_default()[i] = zh.to_owned();
        if let (Some(en), Some(class)) = (cols.next(), forms.get_mut(i)) {
            for form in en.split('/').filter(|f| !f.is_empty()) {
                class.entry(form.to_ascii_lowercase().into_bytes()).or_insert_with(|| zh.to_owned());
            }
        }
    }
    WordTable { words: Arc::new(words), forms: Arc::new(forms) }
}

/// 換成這一份字表（目前語言的）
pub fn set_table(table: &WordTable) {
    *WORDS.write() = Some(table.words.clone());
    *FORMS.write() = Some(table.forms.clone());
}

/// 讀字表並換上（離線工具 examples/t.rs 用）；回傳字數。
pub fn load_words(content: &str) -> usize {
    let table = parse_table(content);
    set_table(&table);
    table.word_count()
}

/// 字表的英文詞形（名詞單數或複數、形容詞）的中文，字母不分大小寫。
fn form_zh(form: &[u8], adj: bool) -> Option<String> {
    if form.len() > 64 || form.contains(&b' ') {
        return None;
    }
    let mut buf = [0u8; 64];
    let lower = &mut buf[..form.len()];
    lower.copy_from_slice(form);
    lower.make_ascii_lowercase();
    FORMS.read().as_ref()?[usize::from(adj)].get(&*lower).cloned()
}

/// 書名的隨機字：名詞（單數或複數）或形容詞的英文詞形的中文。遊戲把書名的隨機字每個字改大寫開頭（FUN_14052d3c0），
/// 小寫開頭的不收（介面字串 `good food`、`new moon` 不會被書名樣板 `{=1} {$2}` 比走）。
pub fn word_form(form: &[u8], adj: bool) -> Option<String> {
    form.first().is_some_and(u8::is_ascii_uppercase).then(|| form_zh(form, adj)).flatten()
}

/// 程序生成的職位名裡的名詞（神殿職位「sacred torch」：FUN_140bd5c30 拿職位名字的 the 名詞，前面隨機接 holy、sacred）：
/// 字表名詞的英文詞形，小寫開頭才收。
pub fn lower_noun(form: &[u8]) -> Option<String> {
    form.first().is_some_and(u8::is_ascii_lowercase).then(|| form_zh(form, false)).flatten()
}

/// 測試用：只設書名的隨機字（字表 WORDS 給名稱的測試設）。先設的算數，各測試共用。
#[cfg(test)]
pub fn set_forms(nouns: &[(&str, &str)], adjs: &[(&str, &str)]) {
    let map = |v: &[(&str, &str)]| v.iter().map(|(en, zh)| (en.as_bytes().to_vec(), (*zh).to_owned())).collect();
    FORMS.write().get_or_insert_with(|| Arc::new([map(nouns), map(adjs), HashMap::new(), HashMap::new()]));
}

/// 字典查詞用：遊戲組過的英文名（名字音譯開著時也有原名）的中文。
pub fn get(english: &[u8]) -> Option<Arc<str>> {
    get_with(crate::translit::table().as_deref(), english)
}

/// 同 get，音譯表由呼叫端給（None 是名字音譯關著；測試用，免得動到全域的開關）
fn get_with(table: Option<&crate::translit::Table>, english: &[u8]) -> Option<Arc<str>> {
    #[cfg(test)]
    if let Some(zh) = FIXED.lock().get(english) {
        return Some(zh.clone());
    }
    if let Some(name) = NAMES.lock().get(english) {
        let zh = chinese(WORDS.read().as_deref()?, &name.words, name.first)?;
        return Some(if name.head.is_empty() { zh.into() } else { with_head_in(table, &name.head, &zh).into() });
    }
    let table = table?;
    if !NATIVES.lock().contains(english) {
        return None;
    }
    crate::translit::name_with(table, english).map(Into::into)
}

/// 字表組的中文名（照 table 這一種語言）：複合字＋「of 字之 the 片語」，first 時只有複合字。有字查不到中文時 None
fn chinese(table: &Words, words: &[Word], first: bool) -> Option<String> {
    let mut zh: [Option<&str>; 7] = [None; 7];
    for (slot, class, token) in words {
        let s = table.get(&**token)?[usize::from(*class)].as_str();
        if s.is_empty() {
            return None;
        }
        zh[usize::from(*slot)] = Some(s);
    }
    let compound: String = zh[0..2].iter().flatten().copied().collect();
    if first {
        return (!compound.is_empty()).then_some(compound);
    }
    // the 片語（words[2..6]）：遊戲只在 words[2]、words[3]、words[5] 有字時才拼
    let phrase: String = if zh[2].is_some() || zh[3].is_some() || zh[5].is_some() { zh[2..6].iter().flatten().copied().collect() } else { String::new() };
    let rest = match zh[6] {
        Some(of) if phrase.is_empty() => of.to_owned(),
        Some(of) => format!("{of}之{phrase}"),
        None => phrase,
    };
    let name = match (compound.is_empty(), rest.is_empty()) {
        (false, false) => format!("{compound}·{rest}"),
        (false, true) => compound,
        _ => rest,
    };
    (!name.is_empty()).then_some(name)
}

fn register(english: &[u8], name: Name) {
    let mut names = NAMES.lock();
    if names.len() >= NAMES_LIMIT {
        names.clear();
    }
    names.insert(english.to_vec(), name);
    GENERATION.fetch_add(1, Ordering::Release);
}

/// hook 用：組原名的函式（native_name）把名字轉成原文 out 之後呼叫：記下整個原名，和只有名的寫法
/// （傳說頁的事件句、相關人物清單的連結字只寫名 `Onsmost`，遊戲不經過組原名的函式）。名字音譯關著也記，打開選項就查得到。
pub fn note_native(out: &[u8]) {
    let first = out.split(|&c| c == b' ').next().filter(|f| f.len() < out.len() && f.first().is_some_and(|&c| c.is_ascii_alphabetic() || c >= 0x80));
    let mut natives = NATIVES.lock();
    let mut added = false;
    for name in std::iter::once(out).chain(first) {
        if name.is_empty() || natives.contains(name) {
            continue;
        }
        if natives.len() >= NAMES_LIMIT {
            natives.clear();
        }
        natives.insert(name.to_vec());
        added = true;
    }
    drop(natives);
    // 名字音譯開著時才有新的譯文：字典記成翻不出來的字串要重算
    if added && crate::translit::table().is_some() {
        GENERATION.fetch_add(1, Ordering::Release);
    }
}

/// 英文名開頭的原名、綽號（head，不含結尾的空白）接上字表組的中文名：名字音譯開著時原名也音譯、以「·」相接
/// （「翁斯莫斯特·年輕意志·鑲寶」；綽號在最後時空一格），否則照原文空一格（「Onsmost 年輕意志·鑲寶」）。
fn with_head(head: &[u8], zh: &str) -> String {
    with_head_in(crate::translit::table().as_deref(), head, zh)
}

fn with_head_in(table: Option<&crate::translit::Table>, head: &[u8], zh: &str) -> String {
    match table.and_then(|t| crate::translit::name_with(t, head)) {
        Some(h) if head.ends_with(b"'") || head.ends_with(b"\"") => format!("{h} {zh}"),
        Some(h) => format!("{h}·{zh}"),
        None => format!("{} {zh}", cp437::decode(head)),
    }
}

pub fn generation() -> u64 {
    GENERATION.load(Ordering::Acquire)
}

/// 換了語言（字表）：字典記成翻不出來的字串可能翻得出來了
pub fn changed() {
    GENERATION.fetch_add(1, Ordering::Release);
}

/// status 用：登記的英文名筆數
pub fn count() -> usize {
    NAMES.lock().len()
}

const NOUN: usize = 0;
const ADJ: usize = 1;
const PREFIX: usize = 2;
const VERB: usize = 3;
/// 拆英文名時各位置先試的詞性（見 parse）：同一個詞形在不同詞性中文不同時（veil 名詞「面紗」、動詞「遮掩」），照遊戲在這個位置
/// 最常用的詞性挑。實測一個世界的 2,434 個名字：複合字前後半都是名詞最多、動詞次之；the 片語前面是形容詞、最後一個是名詞；
/// of 後面名詞多於動詞（現在分詞，of Enchanting）
const COMPOUND: [usize; 4] = [NOUN, VERB, ADJ, PREFIX];
const DESCRIBER: [usize; 4] = [ADJ, NOUN, VERB, PREFIX];
const SUBJECT: [usize; 4] = [NOUN, ADJ, VERB, PREFIX];
const OF: [usize; 4] = [NOUN, VERB, ADJ, PREFIX];

/// 遊戲組好、這次執行卻還沒登記的英文名的中文：標題畫面的存檔清單只存組好的字串（`Thadar Tetha, "The Planet of Destiny"`），
/// 那時字表還沒載入，遊戲也不會再組一次。照 compose 的組法倒過來切：[原名、綽號] [複合字] [the 字 字 字-字] [of 字]，
/// 每個字查英文詞形；複合字從前半最短的切法試起。字表的字遊戲都改成大寫開頭，小寫開頭（介面片語）的不拆。
/// ponytail: 同一個詞形在不同詞性中文不同時只能照位置猜，實測約八成和遊戲組的一樣（世界名、地區名幾乎都對，複合字的堡壘名、
/// 人名差最多）；要完全一致得在讀檔時記下存檔清單上的名字
pub fn parse(english: &[u8]) -> Option<String> {
    let forms = FORMS.read().clone()?;
    parse_with(&forms, english)
}

fn parse_with(forms: &Forms, english: &[u8]) -> Option<String> {
    let zh = |form: &[u8], order: &[usize; 4]| order.iter().find_map(|&c| forms[c].get(form).map(String::as_str));
    let lower = english.to_ascii_lowercase();
    let originals: Vec<&[u8]> = english.split(|&c| c == b' ').collect();
    let mut tokens: Vec<&[u8]> = lower.split(|&c| c == b' ').collect();
    let capital = |k: usize| originals[k].first().is_some_and(u8::is_ascii_uppercase);
    let n = tokens.len();
    let of = if n >= 2 && tokens[n - 2] == b"of" {
        let w = capital(n - 1).then(|| zh(tokens[n - 1], &OF)).flatten()?;
        tokens.truncate(n - 2);
        Some(w)
    } else {
        None
    };
    // the 片語：字依序接，最後一個可能是「words[4]-words[5]」
    let mut phrase = String::new();
    if let Some(t) = tokens.iter().position(|w| *w == b"the") {
        let words = &tokens[t + 1..];
        if words.is_empty() {
            return None;
        }
        for (k, w) in words.iter().enumerate() {
            let last = k + 1 == words.len();
            if !capital(t + 1 + k) {
                return None;
            }
            match w.iter().position(|&c| c == b'-') {
                Some(h) if last => {
                    phrase.push_str(zh(&w[..h], &SUBJECT)?);
                    phrase.push_str(zh(&w[h + 1..], &SUBJECT)?);
                }
                Some(_) => return None,
                None => phrase.push_str(zh(w, if last { &SUBJECT } else { &DESCRIBER })?),
            }
        }
        tokens.truncate(t);
    }
    // 剩下的最後一個字是複合字（切不開就當原名），再前面是原名、綽號
    let mut head = tokens.len();
    let mut compound = String::new();
    if let Some(&w) = tokens.last().filter(|_| capital(head - 1)) {
        let split = (1..w.len()).find_map(|i| Some(format!("{}{}", zh(&w[..i], &COMPOUND)?, zh(&w[i..], &COMPOUND)?)));
        if let Some(c) = split.or_else(|| zh(w, &COMPOUND).map(str::to_owned)) {
            compound = c;
            head -= 1;
        }
    }
    let rest = match of {
        Some(of) if phrase.is_empty() => of.to_owned(),
        Some(of) => format!("{of}之{phrase}"),
        None => phrase,
    };
    let zh = match (compound.is_empty(), rest.is_empty()) {
        (false, false) => format!("{compound}·{rest}"),
        (false, true) => compound,
        (true, false) => rest,
        (true, true) => return None,
    };
    Some(if head == 0 { zh } else { with_head(&originals[..head].join(&b' '), &zh) })
}

/// 測試用：直接登記一個名字的中文（不經 translate_name，不看字表）。
#[cfg(test)]
static FIXED: LazyLock<Mutex<HashMap<Vec<u8>, Arc<str>>>> = LazyLock::new(|| Mutex::new(HashMap::new()));

#[cfg(test)]
pub fn insert(english: &[u8], zh: &str) {
    FIXED.lock().insert(english.to_vec(), zh.into());
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

/// 名字裡的一個字：(英文詞形, 詞性類別, 字的代號)。parts_of_speech：0 名詞、1 名詞複數、2 形容詞、3 字首、4–8 動詞各形。
unsafe fn word<'a>(l: &NameLayout, id: i32, pos: i16) -> Option<(&'a [u8], u8, &'a [u8])> {
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
        Some((str_bytes(w + l.word_forms + pos as usize * 32), class, str_bytes(w + l.word_token)))
    }
}

/// 照遊戲的組法拼出英文名的字表部分（不含原名與綽號，小寫），與各位置的字（中文見 chinese）。
/// 遊戲的組法（translate_name）：words[0]+words[1] 接成一個字；words[2]、words[3]、words[4] 各自前面空一格，
/// 第一個前面接 the，words[4] 後面接「-」再直接接 words[5]；有 words[6] 時接「of words[6]」。
/// 第三項是只有複合字的英文（後面還有 the／of 片語時）：遊戲「只要第一段」時只輸出這段（傳說事件句的 `Silverwarm`）
#[allow(clippy::type_complexity)]
unsafe fn compose(l: &NameLayout, name: usize) -> Option<(Vec<u8>, Vec<Word>, Option<Vec<u8>>)> {
    let (ids, pos): ([i32; 7], [i16; 7]) = unsafe { (read(name, l.words), read(name, l.parts)) };
    let mut forms: [Option<&[u8]>; 7] = [None; 7];
    let mut words = Vec::new();
    for i in 0..7 {
        if ids[i] != -1 {
            let (form, class, token) = unsafe { word(l, ids[i], pos[i]) }?;
            forms[i] = Some(form);
            words.push((i as u8, class, token.into()));
        }
    }
    let mut en: Vec<u8> = Vec::new();
    let space = |en: &mut Vec<u8>| {
        if !en.is_empty() && !en.ends_with(b"-") {
            en.push(b' ');
        }
    };
    // 複合字
    en.extend(forms[0..2].iter().flatten().flat_map(|f| f.iter()));
    let first_len = en.len();
    // the 片語（words[2..6]）：遊戲只在 words[2]、words[3]、words[5] 有字時才拼
    if forms[2].is_some() || forms[3].is_some() || forms[5].is_some() {
        let mut the = false;
        for (i, f) in forms.iter().enumerate().take(6).skip(2) {
            let Some(form) = f else { continue };
            space(&mut en);
            if !std::mem::replace(&mut the, true) {
                en.extend_from_slice(b"the ");
            }
            en.extend_from_slice(form);
            if i == 4 {
                en.push(b'-');
            }
        }
    }
    // of 片語
    if let Some(form) = forms[6] {
        space(&mut en);
        en.extend_from_slice(b"of ");
        en.extend_from_slice(form);
    }
    if en.is_empty() {
        return None;
    }
    en.make_ascii_lowercase();
    let first = (first_len > 0 && en.len() > first_len).then(|| en[..first_len].to_vec());
    Some((en, words, first))
}

/// hook 用：原函式把 name 轉成英文 out 之後呼叫，登記這個英文名（中文查的時候才照目前語言的字表組，見 get）。
/// 開頭的原名、綽號照原文存（英文輸出扣掉字表部分剩下的就是這段）。遊戲只要第一段時（傳說事件句的人名）輸出只有複合字，
/// 對上就登記複合字。目前語言的字表有字缺中文時不登記。
pub unsafe fn note(name: usize, out: &[u8]) {
    let Some(l) = NAME_LAYOUT.get() else { return };
    if out.is_empty() || NAMES.lock().contains_key(out) {
        return;
    }
    let Some((en, words, first)) = (unsafe { compose(l, name) }) else { return };
    let ends = |en: &[u8]| out.len() >= en.len() && out[out.len() - en.len()..].eq_ignore_ascii_case(en);
    let Some((en, first)) = [Some((en, false)), first.map(|f| (f, true))].into_iter().flatten().find(|(en, _)| ends(en)) else { return };
    let head = &out[..out.len() - en.len()];
    let head = match head.strip_suffix(b" ") {
        Some(head) => head,
        None if head.is_empty() => head,
        None => return,
    };
    if WORDS.read().as_deref().is_none_or(|table| chinese(table, &words, first).is_none()) {
        return;
    }
    let words: Box<[Word]> = words.into_iter().filter(|w| !first || w.0 < 2).collect();
    // 以 the 片語開頭的名字，有些畫面去掉開頭的 The 才畫（貴族畫面的小隊名「Spears of Obeying」）：去掉的寫法一起登記
    if head.is_empty() && !first && en.starts_with(b"the ") && out.len() > 4 && out[..4].eq_ignore_ascii_case(b"the ") {
        register(&out[4..], Name { head: head.into(), words: words.clone(), first });
    }
    register(out, Name { head: head.into(), words, first });
}

/// 讀檔時預先登記（Lua 的 note_name，世界裡的名字一次登記）：登記到上限的一半就停，留一半給遊戲之後組的名字
/// （滿了整張清掉，見 register）。回傳還能不能再登記
///
/// # Safety
/// `name` 要是遊戲裡一個 language_name 的位址（同 note，照 NAME_LAYOUT 讀字號）
pub unsafe fn preload(name: usize, out: &[u8]) -> bool {
    if NAMES.lock().len() >= NAMES_LIMIT / 2 {
        return false;
    }
    unsafe { note(name, out) };
    true
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
        WORDS.write().get_or_insert_with(|| {
            Arc::new(
                [("SCORCH", 3, "焦"), ("AXE", 0, "斧"), ("PLANET", 0, "星"), ("DESTINY", 0, "命運"), ("BRONZE", 1, "青銅"), ("GUILD", 0, "公會")]
                    .into_iter()
                    .fold(HashMap::new(), |mut m: Words, (t, i, zh)| {
                        m.entry(t.as_bytes().to_vec()).or_default()[i] = zh.to_owned();
                        m
                    }),
            )
        });
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
            assert_eq!(get(b"Planet of Destiny").as_deref(), Some("命運之星"), "去掉開頭 The 的寫法（貴族畫面的小隊名）也查得到");
            // 人物：原名照原文、空一格接中文
            let n = name([-1, -1, 4, -1, -1, 5, -1], [0, 0, 2, 0, 0, 0, 0]);
            note(&raw const n as usize, b"Urist the Bronze Guild");
            assert_eq!(get(b"Urist the Bronze Guild").as_deref(), Some("Urist 青銅公會"));
            // 遊戲中途打開名字音譯：不必等遊戲再組一次，原名就換成音譯
            let table: crate::translit::Table =
                [("u", "烏"), ("ri", "里"), ("s", "斯"), ("t", "特")].into_iter().map(|(k, v)| (k.as_bytes().to_vec(), v.to_owned())).collect();
            assert_eq!(get_with(Some(&table), b"Urist the Bronze Guild").as_deref(), Some("烏里斯特·青銅公會"));
            // 只要第一段：傳說事件句只寫複合字（龍「Silverwarm the Golden」寫成 `Silverwarm`）
            let n = name([0, 3, 4, -1, -1, 5, -1], [4, 0, 2, 0, 0, 0, 0]);
            note(&raw const n as usize, b"Scorchdestiny");
            assert_eq!(get(b"Scorchdestiny").as_deref(), Some("焦命運"));
            note(&raw const n as usize, b"Scorchdestiny the Bronze Guild");
            assert_eq!(get(b"Scorchdestiny the Bronze Guild").as_deref(), Some("焦命運·青銅公會"));
            // 字表缺中文（ROOT 沒有譯名）、或英文和遊戲輸出對不上：不登記
            let n = name([0, 6, -1, -1, -1, -1, -1], [4, 0, 0, 0, 0, 0, 0]);
            note(&raw const n as usize, b"Scorchroot");
            assert_eq!(get(b"Scorchroot"), None);
            let n = name([0, 1, -1, -1, -1, -1, -1], [4, 0, 0, 0, 0, 0, 0]);
            note(&raw const n as usize, b"Burnaxe");
            assert_eq!(get(b"Burnaxe"), None);
            // 換語言（換一份字表）：已登記的名字不必等遊戲再組一次，照新的字表組中文；字表缺字的照英文
            let hans: Words = [("SCORCH", 3, "焦"), ("DESTINY", 0, "命运"), ("BRONZE", 1, "青铜"), ("GUILD", 0, "公会")]
                .into_iter()
                .fold(HashMap::new(), |mut m: Words, (t, i, zh)| {
                    m.entry(t.as_bytes().to_vec()).or_default()[i] = zh.to_owned();
                    m
                });
            let hant = WORDS.write().replace(Arc::new(hans));
            assert_eq!(get(b"Scorchdestiny the Bronze Guild").as_deref(), Some("焦命运·青铜公会"));
            assert_eq!(get(b"Scorchaxe"), None);
            *WORDS.write() = hant;
            assert_eq!(get(b"Scorchdestiny the Bronze Guild").as_deref(), Some("焦命運·青銅公會"));
        }
        drop(f);
    }

    #[test]
    fn english_names_are_split_back_into_words() {
        let mut forms: Forms = Default::default();
        for (c, en, zh) in [(NOUN, "planet", "星"), (NOUN, "metal", "金屬"), (NOUN, "silver", "銀"), (ADJ, "infinite", "無盡"),
                            (VERB, "enchanting", "施咒"), (NOUN, "veil", "面紗"), (VERB, "veil", "遮掩")] {
            forms[c].insert(en.as_bytes().to_vec(), zh.to_owned());
        }
        let p = |s: &[u8]| parse_with(&forms, s);
        assert_eq!(p(b"The Infinite Planet of Enchanting").as_deref(), Some("施咒之無盡星"), "形容詞＋名詞、of 後面的現在分詞");
        assert_eq!(p(b"Metalsilver").as_deref(), Some("金屬銀"), "複合字拆成前後兩半");
        assert_eq!(p(b"Silverveil").as_deref(), Some("銀面紗"), "名詞、動詞同形時複合字先當名詞");
        assert_eq!(p(b"Urist Metalsilver the Infinite").as_deref(), Some("Urist 金屬銀·無盡"), "原名照原文");
        assert_eq!(p(b"list of planets"), None, "小寫開頭的片語不是名字");
        assert_eq!(p(b"The Shiny Planet"), None, "有字查不到就整個照英文");
    }

    #[test]
    fn transliterated_heads_join_the_chinese_name() {
        let table: crate::translit::Table =
            [("u", "烏"), ("ri", "里"), ("s", "斯"), ("t", "特")].into_iter().map(|(k, v)| (k.as_bytes().to_vec(), v.to_owned())).collect();
        assert_eq!(with_head_in(Some(&table), b"Urist", "青銅公會"), "烏里斯特·青銅公會");
        assert_eq!(with_head_in(Some(&table), b"Urist `Bob'", "青銅公會"), "烏里斯特 `Bob' 青銅公會", "綽號在最後時空一格");
        assert_eq!(with_head_in(Some(&table), b"Kib", "青銅公會"), "Kib 青銅公會", "音譯不出來時原名照原文");
        assert_eq!(with_head_in(None, b"Urist", "青銅公會"), "Urist 青銅公會", "選項關著");
    }

    #[test]
    fn native_names_seen_while_off_are_transliterated_once_turned_on() {
        let table: crate::translit::Table = [("on", "翁"), ("s", "斯"), ("mo", "莫"), ("t", "特"), ("e", "埃"), ("to", "托"), ("g", "格")]
            .into_iter().map(|(k, v)| (k.as_bytes().to_vec(), v.to_owned())).collect();
        note_native(b"Onsmost Etog");
        assert_eq!(get_with(None, b"Onsmost Etog"), None, "名字音譯關著時原名照原文");
        assert_eq!(get_with(Some(&table), b"Onsmost Etog").as_deref(), Some("翁斯莫斯特·埃托格"), "打開後不必等遊戲再組一次");
        assert_eq!(get_with(Some(&table), b"Onsmost").as_deref(), Some("翁斯莫斯特"), "傳說頁事件句的連結字只寫名");
        note_native(b"`Gem' Etog");
        assert_eq!(get_with(Some(&table), b"`Gem'"), None, "開頭是綽號時不另外登記");
    }
}
