//! 自動換行段落（markup_text_boxst）：教學、說明、彈出視窗的長文。
//!
//! 遊戲先把一段英文拆成單字清單，排版函式（mtb_set_width）依寬度算出每個單字的座標，
//! 畫面上再一個字一個字畫（彈出視窗走 top_addst）。整句比對看到的只有單字，永遠對不到。
//! 這裡 hook 排版函式：原函式排完英文後，把單字接回整段去查字典；有譯文就依中文規則重新換行，
//! 把單字清單改寫成「每行一個字」——字串換成一個短代號（字典裡對應那一行中文），座標改成中文的位置。
//! 之後遊戲照常逐字畫，畫字 hook 看到代號就畫那一行中文。
//! 字串原地改寫：代號只有 8 個字元，一定放得進 std::string 既有的緩衝區（至少 15 bytes），不配置記憶體。
//!
//! 沒有譯文的段落（含有連結的傳說、編年史）照畫英文，但遊戲一樣逐字畫：單字若剛好是字典裡的介面標籤
//! （All、Fish、Iron…）會被換成中文，夾在英文句子裡。所以排版時記下這些單字的位址，畫字 hook 遇到就照畫英文。

use std::collections::HashMap;
use std::sync::{LazyLock, OnceLock};

use parking_lot::Mutex;

use crate::cp437;
use crate::dict::Dict;

/// markup_text_boxst／markup_text_wordst 的欄位位移，由 Lua 依 DFHack 的結構定義傳入。
#[derive(Clone, Copy)]
pub struct MtbLayout {
    pub words: usize,
    pub current_width: usize,
    pub max_y: usize,
    pub word_str: usize,
    pub word_red: usize,
    pub word_link: usize,
    pub word_px: usize,
    pub word_py: usize,
    pub word_flags: usize,
}

pub static MTB_LAYOUT: OnceLock<MtbLayout> = OnceLock::new();

/// markup_text_word_flag：[R] 換行、[B] 空一行、[P] 換行並縮排四格
const NEW_LINE: u32 = 1;
const BLANK_LINE: u32 = 2;
const INDENT: u32 = 4;
const MARKERS: u32 = NEW_LINE | BLANK_LINE | INDENT;
/// 每行代號的開頭；遊戲的字串不會以這兩個控制字元開頭
pub const KEY_PREFIX: &[u8] = b"\x01\x02";

/// 不能放在行首的標點（中文與半形），換行時連同前一個字一起移到下一行
const NO_START: &str = "，。、；：！？）」』》〉】〕…‥％・～,.;:!?)]}%";
/// 不能放在行尾的標點，換行時移到下一行
const NO_END: &str = "（「『《〈【〔([{";

/// 每個段落（以 markup_text_boxst 位址區分）的狀態：單字清單沒變、寬度沒變就不重做。
struct State {
    words: Vec<usize>,
    width: i32,
    translation: Option<String>,
}

static STATES: LazyLock<Mutex<HashMap<usize, State>>> = LazyLock::new(|| Mutex::new(HashMap::new()));

/// 照畫英文的段落單字：字串位址 → 排版後的座標。
static PLAIN_WORDS: LazyLock<Mutex<HashMap<usize, (i32, i32)>>> = LazyLock::new(|| Mutex::new(HashMap::new()));
/// ponytail: 段落釋放時沒有通知，舊單字只在同一段落重排時撤掉；超過上限整張清掉，
/// 當時畫面上的英文段落要等下次重排才再受保護。要精確就 hook 段落的解構函式。
const PLAIN_LIMIT: usize = 100_000;

unsafe fn read<T: Copy>(base: usize, offset: usize) -> T {
    unsafe { *((base + offset) as *const T) }
}

unsafe fn write<T>(base: usize, offset: usize, v: T) {
    unsafe { *((base + offset) as *mut T) = v }
}

/// MSVC std::string：16 bytes 內建緩衝區（或長字串指標）、長度、容量。
unsafe fn str_bytes<'a>(s: usize) -> &'a [u8] {
    unsafe {
        let (size, cap): (usize, usize) = (read(s, 16), read(s, 24));
        let data = if cap > 15 { read::<*const u8>(s, 0) } else { s as *const u8 };
        std::slice::from_raw_parts(data, size)
    }
}

/// 原地改寫字串內容；呼叫端保證 bytes 不超過容量（代號 8 bytes ≤ 最小容量 15）。
unsafe fn str_set(s: usize, bytes: &[u8]) {
    unsafe {
        let cap: usize = read(s, 24);
        let data = if cap > 15 { read::<*mut u8>(s, 0) } else { s as *mut u8 };
        std::ptr::copy_nonoverlapping(bytes.as_ptr(), data, bytes.len());
        *data.add(bytes.len()) = 0;
        write(s, 16, bytes.len());
    }
}

/// 單字接回整段：單字間一個空白，單獨的 . , ? ! 直接接在前一字後面（遊戲排版也是這樣貼著），
/// 換行標記寫成 [R]、[B]、[P]；換顏色的單字前面加 [C:前景:背景:亮度]（第一個單字的顏色當整段底色，不標）。
/// 都和遊戲的標記語法相同，譯文可以照用。有連結（傳說頁面）的段落不處理。
unsafe fn plain_text(words: &[usize], l: &MtbLayout) -> Option<Vec<u8>> {
    let mut out: Vec<u8> = Vec::new();
    let mut color: Option<[u8; 3]> = None;
    for &w in words {
        unsafe {
            if read::<i32>(w, l.word_link) != -1 {
                return None;
            }
            let flags: u32 = read(w, l.word_flags);
            let token: &[u8] = if flags & NEW_LINE != 0 {
                b"[R]"
            } else if flags & BLANK_LINE != 0 {
                b"[B]"
            } else if flags & INDENT != 0 {
                b"[P]"
            } else {
                str_bytes(w + l.word_str)
            };
            if token.is_empty() {
                continue;
            }
            if !out.is_empty() && !(token.len() == 1 && b".,?!".contains(&token[0])) {
                out.push(b' ');
            }
            if flags & MARKERS == 0 {
                let c: [u8; 3] = read(w, l.word_red);
                if color.is_some_and(|prev| prev != c) {
                    out.extend_from_slice(format!("[C:{}:{}:{}]", c[0], c[1], c[2]).as_bytes());
                }
                color = Some(c);
            }
            out.extend_from_slice(token);
        }
    }
    Some(out)
}

/// 換行的最小單位：中文字（兩格）、連在一起的英數（不拆開）、空白、換行標記、換色標記。
#[derive(Debug, PartialEq)]
enum Atom {
    Text(String, usize),
    Space,
    NewLine,
    Blank,
    Indent,
    Color([u8; 3]),
}

/// `[C:前景:背景:亮度]`：回傳顏色與標記的長度。
fn color_marker(s: &str) -> Option<([u8; 3], usize)> {
    let body = s.strip_prefix("[C:")?;
    let end = body.find(']')?;
    let mut nums = body[..end].split(':').map(|n| n.parse::<u8>().ok());
    let color = [nums.next()??, nums.next()??, nums.next()??];
    nums.next().is_none().then_some((color, end + 4))
}

fn atoms(text: &str) -> Vec<Atom> {
    let mut out = Vec::new();
    let mut run = String::new();
    let flush = |run: &mut String, out: &mut Vec<Atom>| {
        if !run.is_empty() {
            let n = run.chars().count();
            out.push(Atom::Text(std::mem::take(run), n));
        }
    };
    let mut rest = text;
    while let Some(ch) = rest.chars().next() {
        if let Some((color, n)) = color_marker(rest) {
            flush(&mut run, &mut out);
            out.push(Atom::Color(color));
            rest = &rest[n..];
            continue;
        }
        let marker = [("[R]", Atom::NewLine), ("[B]", Atom::Blank), ("[P]", Atom::Indent)]
            .into_iter()
            .find(|(m, _)| rest.starts_with(m));
        if let Some((m, atom)) = marker {
            flush(&mut run, &mut out);
            out.push(atom);
            rest = &rest[m.len()..];
            continue;
        }
        rest = &rest[ch.len_utf8()..];
        match cp437::byte_of(ch) {
            Some(b' ') => {
                flush(&mut run, &mut out);
                out.push(Atom::Space);
            }
            Some(_) if !NO_START.contains(ch) && !NO_END.contains(ch) => run.push(ch),
            _ => {
                flush(&mut run, &mut out);
                let w = if cp437::byte_of(ch).is_some() { 1 } else { 2 };
                out.push(Atom::Text(ch.to_string(), w));
            }
        }
    }
    flush(&mut run, &mut out);
    out
}

/// 換行後的一段：(px, py, 文字, 顏色)。同一行中途換顏色就拆成兩段。
pub type Segment = (i32, i32, String, [u8; 3]);

/// 依中文規則換行，回傳每段的位置、文字與顏色：base 是整段底色，[C:…] 之後改用標記的顏色（跨行延續）。
/// 和遊戲的排版一致：[R] 換行、[B] 再空一行、[P] 換行並縮排四格；
/// 中文字之間不加空白，英數之間的空白保留；行首不放 NO_START 標點、行尾不放 NO_END 標點。
pub fn wrap(text: &str, width: usize, base: [u8; 3]) -> Vec<Segment> {
    struct Line {
        px: usize,
        py: i32,
        parts: Vec<(String, usize, [u8; 3])>,
    }
    let width = width.max(4);
    let mut lines: Vec<Line> = Vec::new();
    let (mut x, mut y, mut remaining) = (0usize, 0i32, width);
    let mut space = false;
    let mut color = base;
    let new_line = |lines: &mut Vec<Line>, px: usize, py: i32| lines.push(Line { px, py, parts: Vec::new() });
    for atom in atoms(text) {
        match atom {
            Atom::Color(c) => color = c,
            Atom::NewLine => remaining = 0,
            Atom::Blank => {
                remaining = 0;
                x = 0;
                y += 1;
            }
            Atom::Indent => {
                remaining = width;
                x = 4;
                y += 1;
                new_line(&mut lines, x, y);
            }
            Atom::Space => space = lines.last().is_some_and(|l| l.py == y && !l.parts.is_empty()),
            Atom::Text(t, w) => {
                let gap = usize::from(space);
                space = false;
                if remaining < w + gap {
                    // 換行；標點不放行首：把上一行最後一個字一起帶下來
                    let carry = if NO_START.contains(t.as_str()) {
                        lines.last_mut().filter(|l| l.parts.len() > 1).and_then(|l| l.parts.pop())
                    } else {
                        // 上一行以開括號結尾：括號移到下一行
                        lines.last_mut().filter(|l| l.parts.len() > 1 && NO_END.contains(l.parts.last().unwrap().0.as_str()))
                            .and_then(|l| l.parts.pop())
                    };
                    y += 1;
                    x = 0;
                    remaining = width;
                    new_line(&mut lines, 0, y);
                    if let Some(c) = carry {
                        x += c.1;
                        remaining -= c.1.min(remaining);
                        lines.last_mut().unwrap().parts.push(c);
                    }
                    if w > remaining {
                        // 比整行還長的英數：硬切
                        let mut chunk = String::new();
                        for ch in t.chars() {
                            if remaining == 0 {
                                let n = chunk.chars().count();
                                lines.last_mut().unwrap().parts.push((std::mem::take(&mut chunk), n, color));
                                y += 1;
                                x = 0;
                                remaining = width;
                                new_line(&mut lines, 0, y);
                            }
                            chunk.push(ch);
                            remaining -= 1;
                        }
                        let n = chunk.chars().count();
                        x += n;
                        lines.last_mut().unwrap().parts.push((chunk, n, color));
                        continue;
                    }
                } else if lines.last().is_none_or(|l| l.py != y) {
                    new_line(&mut lines, x, y);
                } else if gap == 1 {
                    // 空白跟前一個字同色，才不會多拆出一段
                    let line = lines.last_mut().unwrap();
                    let prev = line.parts.last().map_or(color, |p| p.2);
                    line.parts.push((" ".to_owned(), 1, prev));
                }
                x += w;
                remaining -= w;
                lines.last_mut().unwrap().parts.push((t, w, color));
            }
        }
    }
    let mut out: Vec<Segment> = Vec::new();
    for line in lines {
        let mut px = line.px as i32;
        for (i, (text, w, c)) in line.parts.into_iter().enumerate() {
            match out.last_mut() {
                Some(seg) if i > 0 && seg.3 == c => seg.2.push_str(&text),
                _ => out.push((px, line.py, text, c)),
            }
            px += w as i32;
        }
    }
    out
}

/// 排版函式跑完後呼叫：整段有譯文就把單字清單改寫成中文的行。回傳 false 表示沒動（照畫英文）。
pub unsafe fn rewrite(mtb: usize, dict: &Dict, note_miss: impl FnOnce(&[u8])) -> bool {
    let Some(l) = MTB_LAYOUT.get() else { return false };
    unsafe {
        let begin: usize = read(mtb, l.words);
        let end: usize = read(mtb, l.words + 8);
        if begin == 0 || end <= begin {
            return false;
        }
        let words = std::slice::from_raw_parts(begin as *const usize, (end - begin) / 8);
        let width: i32 = read(mtb, l.current_width);
        let mut states = STATES.lock();
        let state = states.get(&mtb).filter(|s| s.words == words);
        let translation = match state {
            // 已排過、寬度沒變：不用重做
            Some(s) if s.width == width => return s.translation.is_some(),
            // 已改寫成中文、寬度變了：用記住的譯文重排
            Some(s) if str_bytes(words[0] + l.word_str).starts_with(KEY_PREFIX) => s.translation.clone(),
            _ => plain_text(words, l).and_then(|key| {
                let t = dict.translate_text(&key);
                if t.is_none() {
                    note_miss(&key);
                }
                t
            }),
        };
        let done = translation.as_deref().is_some_and(|t| write_lines(mtb, words, width, t, dict, l));
        let old = states.insert(mtb, State { words: words.to_vec(), width, translation: translation.filter(|_| done) });
        mark_plain(old.map(|s| s.words), (!done).then_some(words), l);
        done
    }
}

/// 依中文規則換行，把單字清單改寫成每段一個代號；段數比單字多（放不下）時不動，回傳 false。
unsafe fn write_lines(mtb: usize, words: &[usize], width: i32, translation: &str, dict: &Dict, l: &MtbLayout) -> bool {
    unsafe {
        // 底色取第一個一般單字的；譯文的 [C:…] 標記之後換成標記的顏色
        let base = words
            .iter()
            .find(|&&w| read::<u32>(w, l.word_flags) & MARKERS == 0)
            .map(|&w| read::<[u8; 3]>(w, l.word_red))
            .unwrap_or([7, 0, 1]);
        let segments = wrap(translation, width.max(1) as usize, base);
        let keys: Option<Vec<Vec<u8>>> = segments.iter().map(|s| dict.line_key(&s.2)).collect();
        let Some(keys) = keys.filter(|k| k.len() <= words.len()) else { return false };
        let mut max_y = 0;
        for (i, &w) in words.iter().enumerate() {
            let (px, py, key, color): (i32, i32, &[u8], [u8; 3]) = match (segments.get(i), keys.get(i)) {
                (Some(s), Some(k)) => (s.0, s.1, k, s.3),
                _ => (0, 0, b"", base),
            };
            str_set(w + l.word_str, key);
            write(w, l.word_red, color);
            write(w, l.word_px, px);
            write(w, l.word_py, py);
            write(w, l.word_flags, 0u32);
            max_y = max_y.max(py);
        }
        write(mtb, l.max_y, max_y);
        true
    }
}

/// 撤掉段落舊單字的登記，再登記照畫英文的單字與剛排好的座標。
unsafe fn mark_plain(old: Option<Vec<usize>>, english: Option<&[usize]>, l: &MtbLayout) {
    let mut plain = PLAIN_WORDS.lock();
    for w in old.into_iter().flatten() {
        plain.remove(&(w + l.word_str));
    }
    let Some(words) = english else { return };
    if plain.len() + words.len() > PLAIN_LIMIT {
        plain.clear();
    }
    for &w in words {
        plain.insert(w + l.word_str, unsafe { (read(w, l.word_px), read(w, l.word_py)) });
    }
}

/// 畫字 hook 用：這個字串是照畫英文的段落單字，不查字典、不記未翻譯。
/// 單字釋放後位址可能給別的字串用，所以座標也要和排版時相同才算；讀的位置在原本單字結構的範圍內。
pub unsafe fn is_plain_word(s: usize) -> bool {
    let Some(l) = MTB_LAYOUT.get() else { return false };
    let Some(&(px, py)) = PLAIN_WORDS.lock().get(&s) else { return false };
    let w = s - l.word_str;
    unsafe { read::<i32>(w, l.word_px) == px && read::<i32>(w, l.word_py) == py }
}

#[cfg(test)]
mod tests {
    use super::*;

    const GRAY: [u8; 3] = [7, 0, 0];

    fn lines(text: &str, width: usize) -> Vec<String> {
        wrap(text, width, GRAY).into_iter().map(|(px, py, t, _)| format!("{px},{py}:{t}")).collect()
    }

    #[test]
    fn chinese_wraps_at_width_without_spaces() {
        // 寬 8 格放 4 個中文字
        assert_eq!(lines("準備帶領你的矮人", 8), ["0,0:準備帶領", "0,1:你的矮人"]);
    }

    #[test]
    fn punctuation_never_starts_a_line() {
        // 寬 12 格：第一行剛好放滿六個字，「。」會落在第二行開頭，所以把「人」一起帶下去
        assert_eq!(lines("準備帶領矮人。出發", 12), ["0,0:準備帶領矮", "0,1:人。出發"]);
    }

    #[test]
    fn latin_words_stay_whole_and_keep_spaces() {
        assert_eq!(lines("按 Esc 鍵離開", 20), ["0,0:按 Esc 鍵離開"]);
        assert_eq!(lines("按住 Shift 鍵", 6), ["0,0:按住", "0,1:Shift", "0,2:鍵"]);
    }

    #[test]
    fn markers_follow_game_layout() {
        // [B] 空一行、[P] 換行縮排四格、[R] 換行
        assert_eq!(lines("第一段[B]第二段[P]縮排[R]下一行", 40),
                   ["0,0:第一段", "0,2:第二段", "4,3:縮排", "0,4:下一行"]);
    }

    #[test]
    fn color_markers_split_segments_and_carry_across_lines() {
        // 寬 12 格放 6 個中文字；綠色從「向」延續到下一行
        const GREEN: [u8; 3] = [2, 0, 1];
        assert_eq!(
            wrap("按[C:2:0:1]接受[C:7:0:0]鍵，[C:2:0:1]向大地進擊吧", 12, GRAY),
            [
                (0, 0, "按".to_owned(), GRAY),
                (2, 0, "接受".to_owned(), GREEN),
                (6, 0, "鍵，".to_owned(), GRAY),
                (10, 0, "向".to_owned(), GREEN),
                (0, 1, "大地進擊吧".to_owned(), GREEN),
            ]
        );
    }

    /// 假的段落：單字 56 bytes（MSVC std::string 32 bytes＋顏色、連結、座標、旗標），段落只用單字向量與寬度。
    struct Fake {
        mtb: usize,
        words: Vec<usize>,
    }

    impl Fake {
        fn new(texts: &[&str], width: i32) -> Fake {
            let mut x = 0;
            let words: Vec<usize> = texts
                .iter()
                .map(|t| {
                    let w = Box::into_raw(Box::new([0u64; 7])) as usize;
                    unsafe {
                        std::ptr::copy_nonoverlapping(t.as_ptr(), w as *mut u8, t.len());
                        write(w, 16, t.len());
                        write(w, 24, 15usize);
                        write(w, 32, [7u8, 0, 0]);
                        write(w, 36, -1i32);
                        write(w, 40, x);
                    }
                    x += t.len() as i32 + 1;
                    w
                })
                .collect();
            let mtb = Box::into_raw(Box::new([0u64; 8])) as usize;
            unsafe {
                write(mtb, 0, words.as_ptr() as usize);
                write(mtb, 8, words.as_ptr() as usize + words.len() * 8);
                write(mtb, 48, width);
            }
            Fake { mtb, words }
        }
    }

    impl Drop for Fake {
        fn drop(&mut self) {
            unsafe {
                drop(Box::from_raw(self.mtb as *mut [u64; 8]));
                for &w in &self.words {
                    drop(Box::from_raw(w as *mut [u64; 7]));
                }
            }
        }
    }

    #[test]
    fn untranslated_paragraph_words_are_drawn_as_english() {
        let _ = MTB_LAYOUT.set(MtbLayout {
            words: 0, current_width: 48, max_y: 52, word_str: 0, word_red: 32, word_link: 36, word_px: 40, word_py: 44, word_flags: 48,
        });
        let mut d = Dict::default();
        assert!(d.set_glyphs("全魚", vec![0; 4]));
        d.load_tsv("All\t全\nAll Fish\t全魚\n");
        unsafe {
            // 段落沒譯文：單字照畫英文，不拿 All 去查字典
            let plain = Fake::new(&["Fish", "All"], 40);
            assert!(!rewrite(plain.mtb, &d, |_| {}));
            assert!(plain.words.iter().all(|&w| is_plain_word(w)));
            // 單字釋放後同一位址給了別的字串（座標對不上）：不算
            write(plain.words[1], 40, 99i32);
            assert!(!is_plain_word(plain.words[1]));
            // 遊戲依新寬度重排後重新登記
            write(plain.mtb, 48, 30i32);
            assert!(!rewrite(plain.mtb, &d, |_| {}));
            assert!(is_plain_word(plain.words[1]));

            // 有譯文：單字改寫成代號，不登記
            let done = Fake::new(&["All", "Fish"], 40);
            assert!(rewrite(done.mtb, &d, |_| {}));
            assert!(str_bytes(done.words[0]).starts_with(KEY_PREFIX));
            assert!(!done.words.iter().any(|&w| is_plain_word(w)));
        }
    }
}
