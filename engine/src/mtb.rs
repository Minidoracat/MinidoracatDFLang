//! 自動換行段落（markup_text_boxst）：教學、說明、彈出視窗的長文。
//!
//! 遊戲先把一段英文拆成單字清單，排版函式（mtb_set_width）依寬度算出每個單字的座標，
//! 畫面上再一個字一個字畫（彈出視窗走 top_addst）。整句比對看到的只有單字，永遠對不到。
//! 這裡 hook 排版函式：原函式排完英文後，把單字接回整段去查字典；有譯文就依中文規則重新換行，
//! 替每個單字訂好畫字計畫——每段中文交給同一列的一個英文單字（載體）畫，其他單字不畫。
//! 畫字 hook 看到載體單字，就在「載體座標＋位移」畫那段中文。
//!
//! 遊戲的單字清單一律不改：彈出視窗排好的單字會存進存檔，改過的字串讀檔後（或移除本 MOD 後）會變成亂碼。
//! 沒有譯文的段落（含有連結的傳說、編年史）也登記成「照畫英文」：單字若剛好是字典裡的介面標籤
//! （All、Fish、Iron…）才不會被換成中文，夾在英文句子裡。

use std::collections::HashMap;
use std::sync::{Arc, LazyLock, OnceLock};

use parking_lot::Mutex;

use crate::cp437;
use crate::dict::{Dict, Entry};

/// markup_text_boxst／markup_text_wordst 的欄位位移，由 Lua 依 DFHack 的結構定義傳入。
#[derive(Clone, Copy)]
pub struct MtbLayout {
    pub words: usize,
    pub current_width: usize,
    pub word_str: usize,
    pub word_red: usize,
    pub word_link: usize,
    pub word_px: usize,
    pub word_py: usize,
    pub word_flags: usize,
    /// gps.uccolor 的位址：遊戲調色盤 16 色 × RGB。單字只存 RGB，換回調色盤編號，段落原文才不受玩家配色影響
    pub palette: usize,
}

type Palette = [[u8; 3]; 16];

unsafe fn palette(l: &MtbLayout) -> Option<Palette> {
    (l.palette != 0).then(|| unsafe { *(l.palette as *const Palette) })
}

/// 標記 [C:前景:背景:亮度] 的顏色在調色盤上的 RGB（背景不影響字色）
fn marker_rgb(palette: &Palette, [fg, _, bright]: [u8; 3]) -> [u8; 3] {
    palette[(fg as usize % 8) + 8 * usize::from(bright != 0)]
}

pub static MTB_LAYOUT: OnceLock<MtbLayout> = OnceLock::new();

/// markup_text_word_flag：[R] 換行、[B] 空一行、[P] 換行並縮排四格
const NEW_LINE: u32 = 1;
const BLANK_LINE: u32 = 2;
const INDENT: u32 = 4;
const MARKERS: u32 = NEW_LINE | BLANK_LINE | INDENT;

/// 不能放在行首的標點（中文與半形），換行時連同前一個字一起移到下一行
const NO_START: &str = "，。、；：！？）」』》〉】〕…‥％・～,.;:!?)]}%";
/// 不能放在行尾的標點，換行時移到下一行
const NO_END: &str = "（「『《〈【〔([{";

/// 每個段落（以 markup_text_boxst 位址區分）上次排版的單字清單與寬度：都沒變就不重做。
struct State {
    words: Vec<usize>,
    width: i32,
    translated: bool,
}

static STATES: LazyLock<Mutex<HashMap<usize, State>>> = LazyLock::new(|| Mutex::new(HashMap::new()));

/// 一段中文：畫在載體字串座標＋(dx, dy)；color 有值時用它上色，沒有就沿用遊戲當下的顏色。
pub struct Piece {
    pub dx: i32,
    pub dy: i32,
    pub color: Option<[u8; 3]>,
    pub entry: Entry,
}

/// 段落單字或自動換行的一行被畫到時要做的事。
#[derive(Clone)]
pub enum Plan {
    /// 沒有譯文：照畫英文，不查字典
    Plain,
    /// 沒有譯文的提示行（見 lines.rs）：照畫英文，畫到時把整句記成未翻譯（不記片段）
    Missing(Arc<[u8]>),
    /// 有譯文，但這個字串不負責畫中文：不畫
    Skip,
    /// 載體：畫這幾段中文
    Draw(Arc<[Piece]>),
}

/// 段落單字的畫字計畫：字串位址 → (排版後的座標, 計畫)。
static WORDS: LazyLock<Mutex<HashMap<usize, (i32, i32, Plan)>>> = LazyLock::new(|| Mutex::new(HashMap::new()));
/// ponytail: 段落釋放時沒有通知，舊單字只在同一段落重排時撤掉；超過上限整張清掉，
/// 當時畫面上的段落要等下次重排才恢復中文。要精確就 hook 段落的解構函式。
const WORDS_LIMIT: usize = 200_000;

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

/// 單字接回整段：單字間一個空白，單獨的 . , ? ! 直接接在前一字後面（遊戲排版也是這樣貼著），
/// 換行標記寫成 [R]、[B]、[P]；換顏色的單字前面加 [C:前景:背景:亮度]（第一個單字的顏色當整段底色，不標）。
/// 都和遊戲的標記語法相同，譯文可以照用。有連結（傳說頁面）的段落、顏色不在調色盤上的段落不處理。
unsafe fn plain_text(words: &[usize], l: &MtbLayout, palette: Option<&Palette>) -> Option<Vec<u8>> {
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
                    // ponytail: 調色盤有兩格同色時取第一格，原文就和遊戲原始標記不同而對不上；遇到再改成比對原始字串
                    let i = palette?.iter().position(|p| *p == c)?;
                    out.extend_from_slice(format!("[C:{}:0:{}]", i % 8, i / 8).as_bytes());
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

/// `[C:前景:背景:亮度]`：回傳顏色與標記的長度。遊戲偶爾多拼一個冒號（`[C:7:0:0:]`），照樣收。
fn color_marker(s: &str) -> Option<([u8; 3], usize)> {
    let body = s.strip_prefix("[C:")?;
    let end = body.find(']')?;
    let mut nums = body[..end].strip_suffix(':').unwrap_or(&body[..end]).split(':').map(|n| n.parse::<u8>().ok());
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

/// 依中文規則換行，回傳每段的位置、文字與顏色：base 是整段底色（RGB），[C:…] 之後改用標記的顏色（跨行延續），
/// 標記的顏色由 rgb 換算。和遊戲的排版一致：[R] 換行、[B] 再空一行、[P] 換行並縮排四格；
/// 中文字之間不加空白，英數之間的空白保留；行首不放 NO_START 標點、行尾不放 NO_END 標點。
pub fn wrap(text: &str, width: usize, base: [u8; 3], rgb: impl Fn([u8; 3]) -> [u8; 3]) -> Vec<Segment> {
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
            Atom::Color(c) => color = rgb(c),
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
                    // 空白跟前一個字同色，才不會多拆出一段；空白也佔一格
                    let line = lines.last_mut().unwrap();
                    let prev = line.parts.last().map_or(color, |p| p.2);
                    line.parts.push((" ".to_owned(), 1, prev));
                    x += 1;
                    remaining -= 1;
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

/// 排版函式跑完後呼叫：依譯文訂好這個段落每個單字的畫字計畫。回傳 true 表示這段有譯文。
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
        if let Some(s) = states.get(&mtb).filter(|s| s.words == words && s.width == width) {
            return s.translated; // 已排過、寬度沒變：計畫還有效
        }
        let pal = palette(l);
        let plans = plain_text(words, l, pal.as_ref())
            .and_then(|key| {
                let t = dict.translate_paragraph(&key);
                if t.is_none() {
                    note_miss(&key);
                }
                t
            })
            .and_then(|t| plan_words(words, width, &t, dict, l, pal.as_ref()));
        let translated = plans.is_some();
        let mut table = WORDS.lock();
        if let Some(old) = states.remove(&mtb) {
            for w in old.words {
                table.remove(&(w + l.word_str));
            }
        }
        if table.len() + words.len() > WORDS_LIMIT {
            table.clear();
            states.clear();
        }
        for (i, &w) in words.iter().enumerate() {
            let plan = plans.as_ref().map_or(Plan::Plain, |p| p[i].clone());
            table.insert(w + l.word_str, (read(w, l.word_px), read(w, l.word_py), plan));
        }
        states.insert(mtb, State { words: words.to_vec(), width, translated });
        translated
    }
}

/// 依中文規則換行，把每段中文交給同一列第一個英文單字畫：回傳每個單字的計畫。
/// 載體和中文在同一列，捲動的清單裡遊戲依載體決定畫不畫，才會和中文那列一致。
/// 缺字圖或沒有能畫的單字時回傳 None（照畫英文）。
/// ponytail: 中文行數比英文多時，多出來的行掛在最後一個單字下面，會畫到段落框外；
/// 段落高度 max_y 會存進存檔所以不改，遇到再想辦法。
unsafe fn plan_words(words: &[usize], width: i32, translation: &str, dict: &Dict, l: &MtbLayout,
                     pal: Option<&Palette>) -> Option<Vec<Plan>> {
    unsafe {
        // 遊戲會畫的單字：(索引, px, py)
        let drawable: Vec<(usize, i32, i32)> = words
            .iter()
            .enumerate()
            .filter(|&(_, &w)| read::<u32>(w, l.word_flags) & MARKERS == 0 && !str_bytes(w + l.word_str).is_empty())
            .map(|(i, &w)| (i, read(w, l.word_px), read(w, l.word_py)))
            .collect();
        let &(first, ..) = drawable.first()?;
        // 底色取第一個單字的；譯文的 [C:…] 標記之後換成標記的顏色（沒有調色盤就不換色）
        let base: [u8; 3] = read(words[first], l.word_red);
        let segments = wrap(translation, width.max(1) as usize, base, |c| pal.map_or(base, |p| marker_rgb(p, c)));
        let mut pieces: Vec<Vec<Piece>> = (0..words.len()).map(|_| Vec::new()).collect();
        for (px, py, text, color) in segments {
            let entry = dict.text_entry(&text)?;
            // 那列英文是空行時往下找，再沒有就用最後一個單字
            let &(i, wx, wy) = drawable.iter().find(|d| d.2 == py).or_else(|| drawable.iter().find(|d| d.2 > py))
                .or(drawable.last())?;
            pieces[i].push(Piece { dx: px - wx, dy: py - wy, color: Some(color), entry });
        }
        Some(pieces.into_iter().map(|p| if p.is_empty() { Plan::Skip } else { Plan::Draw(p.into()) }).collect())
    }
}

/// 畫字 hook 用：這個字串是段落單字時，回傳它的畫字計畫。
/// 單字釋放後位址可能給別的字串用，所以座標也要和排版時相同才算；讀的位置在原本單字結構的範圍內。
pub unsafe fn word_plan(s: usize) -> Option<Plan> {
    let l = MTB_LAYOUT.get()?;
    let table = WORDS.lock();
    let (px, py, plan) = table.get(&s)?;
    let w = s - l.word_str;
    unsafe { (read::<i32>(w, l.word_px) == *px && read::<i32>(w, l.word_py) == *py).then(|| plan.clone()) }
}

#[cfg(test)]
mod tests {
    use super::*;

    unsafe fn write<T>(base: usize, offset: usize, v: T) {
        unsafe { *((base + offset) as *mut T) = v }
    }

    const GRAY: [u8; 3] = [7, 0, 0];

    fn lines(text: &str, width: usize) -> Vec<String> {
        wrap(text, width, GRAY, |c| c).into_iter().map(|(px, py, t, _)| format!("{px},{py}:{t}")).collect()
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
        // 名字裡的空白也佔一格：寬 14 格放不下「來自The Razor」（13 格）再加「的」
        assert_eq!(lines("來自The Razor的商隊", 14), ["0,0:來自The Razor", "0,1:的商隊"]);
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
            wrap("按[C:2:0:1]接受[C:7:0:0]鍵，[C:2:0:1]向大地進擊吧", 12, GRAY, |c| c),
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

    /// 測試用調色盤：第 i 格是 (i, i, i)
    static PALETTE: Palette = {
        let mut p = [[0u8; 3]; 16];
        let mut i = 0;
        while i < 16 {
            p[i] = [i as u8 * 10; 3];
            i += 1;
        }
        p
    };

    impl Fake {
        /// (單字, 調色盤第幾格的顏色)
        fn new(texts: &[(&str, usize)], width: i32) -> Fake {
            let _ = MTB_LAYOUT.set(MtbLayout {
                words: 0, current_width: 48, word_str: 0, word_red: 32, word_link: 36, word_px: 40, word_py: 44,
                word_flags: 48, palette: PALETTE.as_ptr() as usize,
            });
            let mut x = 0;
            let words: Vec<usize> = texts
                .iter()
                .map(|&(t, color)| {
                    let w = Box::into_raw(Box::new([0u64; 7])) as usize;
                    unsafe {
                        std::ptr::copy_nonoverlapping(t.as_ptr(), w as *mut u8, t.len());
                        write(w, 16, t.len());
                        write(w, 24, 15usize);
                        write(w, 32, PALETTE[color]);
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

    fn plan_of(w: usize) -> &'static str {
        match unsafe { word_plan(w) } {
            Some(Plan::Plain) => "plain",
            Some(Plan::Missing(_)) => "missing",
            Some(Plan::Skip) => "skip",
            Some(Plan::Draw(_)) => "draw",
            None => "none",
        }
    }

    #[test]
    fn untranslated_paragraph_words_are_drawn_as_english() {
        let mut d = Dict::default();
        assert!(d.set_glyphs("全魚", vec![0; 4]));
        d.load_tsv("All\t全\nAll Fish\t全魚\n");
        unsafe {
            // 段落沒譯文：單字照畫英文，不拿 All 去查字典
            let plain = Fake::new(&[("Fish", 7), ("All", 7)], 40);
            assert!(!rewrite(plain.mtb, &d, |_| {}));
            assert_eq!(plan_of(plain.words[1]), "plain");
            // 單字釋放後同一位址給了別的字串（座標對不上）：不算
            write(plain.words[1], 40, 99i32);
            assert_eq!(plan_of(plain.words[1]), "none");
            // 遊戲依新寬度重排後重新登記
            write(plain.mtb, 48, 30i32);
            assert!(!rewrite(plain.mtb, &d, |_| {}));
            assert_eq!(plan_of(plain.words[1]), "plain");
        }
    }

    #[test]
    fn translated_paragraph_is_drawn_by_a_carrier_word_and_game_data_stays_english() {
        let mut d = Dict::default();
        assert!(d.set_glyphs("全魚", vec![0; 4]));
        d.load_tsv("All Fish\t全魚\n");
        unsafe {
            let p = Fake::new(&[("All", 7), ("Fish", 7)], 40);
            assert!(rewrite(p.mtb, &d, |_| {}));
            // 第一個單字畫整行中文，第二個不畫；遊戲的單字字串不動（會存進存檔）
            assert_eq!((plan_of(p.words[0]), plan_of(p.words[1])), ("draw", "skip"));
            assert_eq!(str_bytes(p.words[0]), b"All");
            assert_eq!(str_bytes(p.words[1]), b"Fish");
        }
    }

    #[test]
    fn color_markers_use_palette_indices_not_rgb() {
        // 原文標記是遊戲的 [C:前景:背景:亮度]（亮綠是第 2 色加亮 = 調色盤第 10 格），玩家換配色也不變
        let mut d = Dict::default();
        assert!(d.set_glyphs("開鑿大地", vec![0; 8]));
        d.load_tsv("Okay. [C:2:0:1]Strike\t開鑿[C:2:0:1]大地\n");
        unsafe {
            let p = Fake::new(&[("Okay", 7), (".", 7), ("Strike", 10)], 40);
            assert!(rewrite(p.mtb, &d, |_| {}));
            // 「開鑿」維持第一個字的顏色，「大地」換成調色盤第 10 格；兩段都在第 0 列，由第一個單字畫
            let Some(Plan::Draw(pieces)) = word_plan(p.words[0]) else { panic!("第一個單字要畫中文") };
            let colors: Vec<Option<[u8; 3]>> = pieces.iter().map(|p| p.color).collect();
            assert_eq!(colors, [Some(PALETTE[7]), Some(PALETTE[10])]);
            assert_eq!(pieces[1].dx, 4, "「大地」接在「開鑿」（4 格）後面");
        }
    }
}
