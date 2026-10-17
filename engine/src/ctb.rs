//! 彩色文字框（color_text_boxst）：人物頁的想法、記憶、性格（特質、價值觀、偏好、需求）、屬性等。
//!
//! 遊戲把一段含 `[C:前景:背景:亮度]` 換色標記的原文交給 add_string，依框寬切成好幾行
//! （color_text_linest：文字與每字顏色各 769 bytes），再一行一行用 addcoloredst 畫，畫字 hook 只看得到一行。
//! 這裡在 add_string 之後整段處理：去掉換色標記並記下每個字的顏色，`[P]`、`[R]`、`[B]` 處分段，
//! 每段逐句查字典（Dict::translate_sentences）；都有譯文就依中文規則以同樣框寬重新換行，
//! 第 k 行中文交給第 k 行英文（載體）畫，多出來的英文行不畫。譯文依原文位置的顏色上色：
//! 佔位的譯文用佔位原文的顏色，句型的固定字用比對範圍開頭的顏色。
//! 沒有譯文的段落整段照畫英文，不拿行片段查字典（免得一段裡有的行中文、有的行英文）；段落缺句時其餘照翻開著時，
//! 缺的句子照原文接在譯文裡一起重新換行（見 translate）。和段落一樣只在畫字時換，遊戲資料不改。

use std::collections::HashMap;
use std::sync::{Arc, LazyLock, OnceLock};

use parking_lot::Mutex;

use crate::dict::{Dict, Entry};

/// color_text_boxst.width、color_text_linest.color 的位移（行的文字在位移 0），由 Lua 依 DFHack 的結構定義傳入。
#[derive(Clone, Copy)]
pub struct CtbLayout {
    pub width: usize,
    pub color: usize,
}

pub static CTB_LAYOUT: OnceLock<CtbLayout> = OnceLock::new();

/// 一段中文：畫在載體行的座標＋(dx, dy)，顏色是遊戲的顏色位元組（前景＋背景×8＋亮度×64）。
pub struct Piece {
    pub dx: i32,
    pub dy: i32,
    pub color: u8,
    pub entry: Entry,
}

/// 一行被畫到時要做的事。
#[derive(Clone)]
pub enum Plan {
    /// 整段沒有譯文：照畫英文
    English,
    /// 中文行數比英文少：這行不畫
    Skip,
    Draw(Arc<[Piece]>),
}

struct Line {
    /// 登記時的英文：行釋放後位址可能給別的行用，內容相同才照計畫畫
    text: Box<[u8]>,
    plan: Plan,
}

static LINES: LazyLock<Mutex<HashMap<usize, Line>>> = LazyLock::new(|| Mutex::new(HashMap::new()));
/// ponytail: 行釋放時沒有通知，超過上限整批清掉；當時畫面上的框要等下次重建（切分頁）才恢復中文。
const LINES_LIMIT: usize = 50_000;

/// 同一段原文、同框寬、同行數、同底色算出來的計畫（None 是沒有譯文）
type Key = (Vec<u8>, i32, usize, u8);
static PLANS: LazyLock<Mutex<HashMap<Key, Option<Arc<[Plan]>>>>> = LazyLock::new(|| Mutex::new(HashMap::new()));
const PLANS_LIMIT: usize = 5_000;

fn color_byte([fg, bg, bright]: [u8; 3]) -> u8 {
    (fg & 7) | (bg & 7) << 3 | u8::from(bright != 0) << 6
}

fn color_parts(c: u8) -> [u8; 3] {
    [c & 7, (c >> 3) & 7, (c >> 6) & 1]
}

/// 一段（`[P]`、`[R]`、`[B]` 之間）去掉換色標記的文字、每個字的顏色，和這段前面的分段標記。
#[derive(Debug, PartialEq)]
pub struct Block {
    pub text: Vec<u8>,
    pub colors: Vec<u8>,
    /// 前面是 `[B]`（空一行）；否則是換行。第一段不看
    pub blank_before: bool,
}

/// `[C:前景:背景:亮度]` 開頭時回傳 (顏色位元組, 標記長度)。原文是 CP437 位元組，不一定是 UTF-8，所以逐位元組讀。
/// 遊戲偶爾多拼一個冒號（病歷的 `[C:7:0:0:]`），遊戲照樣當成換色，這裡也收。
fn color_marker(rest: &[u8]) -> Option<(u8, usize)> {
    let body = rest.strip_prefix(b"[C:")?;
    let end = body.iter().take(12).position(|&c| c == b']')?;
    let fields = body[..end].strip_suffix(b":").unwrap_or(&body[..end]);
    let mut nums = fields.split(|&c| c == b':').map(|n| std::str::from_utf8(n).ok()?.parse::<u8>().ok());
    let color = [nums.next()??, nums.next()??, nums.next()??];
    nums.next().is_none().then_some((color_byte(color), end + 4))
}

/// 拆段並去掉換色標記；base 是第一個標記之前的顏色。空段（開頭的 `[P]`）略過。
pub fn blocks(raw: &[u8], base: u8) -> Vec<Block> {
    let mut out = Vec::new();
    let mut cur = Block { text: Vec::new(), colors: Vec::new(), blank_before: false };
    let mut color = base;
    let mut i = 0;
    while i < raw.len() {
        let rest = &raw[i..];
        if let Some(n) = [b"[P]", b"[R]", b"[B]"].iter().find(|m| rest.starts_with(&m[..])).map(|m| m.len()) {
            let blank = rest.starts_with(b"[B]");
            if cur.text.iter().any(|&c| c != b' ') {
                out.push(std::mem::replace(&mut cur, Block { text: Vec::new(), colors: Vec::new(), blank_before: blank }));
            } else {
                cur = Block { text: Vec::new(), colors: Vec::new(), blank_before: cur.blank_before || blank };
            }
            i += n;
            continue;
        }
        if let Some((c, n)) = color_marker(rest) {
            color = c;
            i += n;
            continue;
        }
        cur.text.push(raw[i]);
        cur.colors.push(color);
        i += 1;
    }
    if cur.text.iter().any(|&c| c != b' ') {
        out.push(cur);
    }
    out
}

/// 整段的中文（含換色、分段標記，交給 mtb::wrap 換行）與查不到的句子。每句都翻得出來才有中文；段落缺句時其餘照翻
/// （options.partial）開著時至少一句有譯文就有中文，翻不出來的句子、整段都翻不出來的段照原文（每個字照原文的顏色）。
pub fn translate(raw: &[u8], base: u8, dict: &Dict) -> (Option<String>, Vec<Vec<u8>>) {
    let mut out = String::new();
    let mut missing = Vec::new();
    let mut current: Option<u8> = None;
    let mut translated = false;
    for (k, block) in blocks(raw, base).iter().enumerate() {
        let (pieces, m) = dict.translate_sentences(&block.text);
        missing.extend(m);
        let pieces = match pieces {
            Some(p) => {
                translated = true;
                p
            }
            None if dict.options.partial => crate::dict::english_pieces(&block.text, 0),
            None => continue,
        };
        if k > 0 {
            out.push_str(if block.blank_before { "[B]" } else { "[R]" });
        }
        for (text, at) in pieces {
            let c = block.colors.get(at).copied().unwrap_or(base);
            if current != Some(c) {
                let [fg, bg, bright] = color_parts(c);
                out.push_str(&format!("[C:{fg}:{bg}:{bright}]"));
                current = Some(c);
            }
            out.push_str(&text);
        }
    }
    let ok = missing.is_empty() || (dict.options.partial && translated);
    (ok.then_some(out), missing)
}

/// 去掉換色標記（原文沒有標記時，查詢工具與 dflang t 的輸出不顯示引擎加上的標記）
pub fn strip_colors(s: &str) -> String {
    let mut out = String::new();
    let mut rest = s;
    while let Some(i) = rest.find("[C:") {
        out.push_str(&rest[..i]);
        rest = rest[i..].find(']').map_or("", |j| &rest[i + j + 1..]);
    }
    out.push_str(rest);
    out
}

/// 一段排成 n 行時每行的計畫；缺字圖時回傳 None（照畫英文）。
fn compute(translation: &str, width: i32, n: usize, base: u8, dict: &Dict) -> Option<Arc<[Plan]>> {
    let segments = crate::mtb::wrap(translation, width.max(1) as usize, color_parts(base), |c| c);
    let mut pieces: Vec<Vec<Piece>> = (0..n).map(|_| Vec::new()).collect();
    for (px, py, t, c) in segments {
        let carrier = (py as usize).min(n - 1);
        pieces[carrier].push(Piece { dx: px, dy: py - carrier as i32, color: color_byte(c), entry: dict.para_entry(&t)? });
    }
    Some(pieces.into_iter().map(|p| if p.is_empty() { Plan::Skip } else { Plan::Draw(p.into()) }).collect())
}

unsafe fn cstr<'a>(p: usize) -> &'a [u8] {
    unsafe { std::ffi::CStr::from_ptr(p as *const std::ffi::c_char) }.to_bytes()
}

/// add_string 剛切好的這幾行（行的位址）登記畫字計畫。回傳查不到的句子（給未翻譯紀錄）。
pub unsafe fn plan(lines: &[usize], raw: &[u8], width: i32, dict: &Dict) -> Vec<Vec<u8>> {
    let Some(layout) = CTB_LAYOUT.get() else { return Vec::new() };
    if lines.is_empty() {
        return Vec::new();
    }
    let base = unsafe { *((lines[0] + layout.color) as *const u8) };
    let mut missing = Vec::new();
    let plans = {
        let mut cache = PLANS.lock();
        if cache.len() > PLANS_LIMIT {
            cache.clear();
        }
        cache
            .entry((raw.to_vec(), width, lines.len(), base))
            .or_insert_with(|| {
                let (t, m) = translate(raw, base, dict);
                missing = m;
                t.and_then(|t| compute(&t, width, lines.len(), base, dict))
            })
            .clone()
    };
    let mut table = LINES.lock();
    if table.len() + lines.len() > LINES_LIMIT {
        table.clear();
    }
    for (k, &s) in lines.iter().enumerate() {
        let plan = plans.as_ref().map_or(Plan::English, |p| p[k].clone());
        table.insert(s, Line { text: unsafe { cstr(s) }.into(), plan });
    }
    missing
}

/// 畫字 hook 用：這個字串是彩色文字框的一行時，回傳它的畫字計畫。
pub unsafe fn line_plan(s: usize) -> Option<Plan> {
    let table = LINES.lock();
    let line = table.get(&s)?;
    (*line.text == *unsafe { cstr(s) }).then(|| line.plan.clone())
}

/// status 用：(登記的行數, 計畫快取筆數)
pub fn counts() -> (usize, usize) {
    let lines = LINES.lock().len();
    (lines, PLANS.lock().len())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn dict() -> Dict {
        let mut d = Dict::default();
        let chars = "他她在衝突中感到喜悅。很慢才生氣";
        assert!(d.set_glyphs(chars, vec![0; chars.chars().count() * 2]));
        d.load_tsv("he\t他\njoy\t喜悅\nwhile in conflict\t在衝突中\nHe is slow to anger.\t他很慢才生氣。\n");
        d.load_rules_tsv("{@1} feels {@2} {%3}.\t{1}{3}感到{2}。\n");
        d
    }

    #[test]
    fn markers_become_per_character_colors_and_blocks() {
        let b = blocks(b"[P][C:7:0:1]He feels [C:2:0:1]joy[R]x[B][B]y", 7);
        assert_eq!(b.len(), 3, "開頭的 [P] 不算一段");
        assert_eq!(b[0].text, b"He feels joy");
        assert_eq!(b[0].colors[0], 7 + 64);
        assert_eq!(b[0].colors[9], 2 + 64, "joy 是 [C:2:0:1]");
        assert!(!b[1].blank_before && b[2].blank_before);
        let odd = blocks(b"[C:7:0:0:]Diagnosed with fever", 7);
        assert_eq!(odd[0].text, b"Diagnosed with fever", "遊戲多拼冒號的標記也要去掉");
    }

    #[test]
    fn captures_keep_their_source_color_and_literals_take_the_sentence_color() {
        let d = dict();
        let zh = translate(b"[C:7:0:1]He feels [C:2:0:1]joy [C:7:0:1]while in conflict.", 7, &d).0.unwrap();
        assert_eq!(zh, "[C:7:0:1]他在衝突中感到[C:2:0:1]喜悅[C:7:0:1]。");
        let two = translate(b"[C:7:0:1]He feels [C:2:0:1]joy [C:7:0:1]while in conflict.  He is slow to anger.", 7, &d).0.unwrap();
        assert!(two.ends_with("。他很慢才生氣。"), "{two}");
    }

    #[test]
    fn a_literal_between_neighbouring_captures_takes_the_color_between_them() {
        let mut d = Dict::default();
        d.load_tsv("he\t他\nclumsy\t笨拙\nweak\t虛弱\n");
        d.load_rules_tsv("{@1} is {@2} and {@3}.\t{1}{2}、{3}。\n");
        let zh = translate(b"[C:7:0:1]He is [C:4:0:1]clumsy and weak.", 7, &d).0.unwrap();
        assert_eq!(zh, "[C:7:0:1]他[C:4:0:1]笨拙、虛弱[C:7:0:1]。", "「、」對應原文紅色的 and，句末標點用句首的顏色");
    }

    #[test]
    fn untranslated_sentence_keeps_the_paragraph_english() {
        let d = dict();
        let r = translate(b"[C:7:0:1]He is slow to anger.  He is quick to forgive.", 7, &d);
        assert_eq!(r, (None, vec![b"He is quick to forgive.".to_vec()]));
    }

    #[test]
    fn partial_option_draws_missing_sentences_in_their_source_colors() {
        let mut d = dict();
        d.options.partial = true;
        let (zh, missing) = translate(b"[C:7:0:1]He is slow to anger.  He is [C:4:0:1]clumsy.[R][C:7:0:1]Nobody knows.", 7, &d);
        assert_eq!(missing, vec![b"He is clumsy.".to_vec(), b"Nobody knows.".to_vec()]);
        assert_eq!(zh.as_deref(), Some("[C:7:0:1]他很慢才生氣。 He is[C:4:0:1] clumsy.[R][C:7:0:1]Nobody knows."),
                   "照原文的字保留原文的顏色；整段翻不出來的段照原文");
    }

    #[test]
    fn chinese_is_drawn_by_the_english_lines_in_order() {
        let d = dict();
        let zh = translate(b"[C:7:0:1]He feels [C:2:0:1]joy [C:7:0:1]while in conflict.", 7, &d).0.unwrap();
        let plans = compute(&zh, 20, 3, 7 + 64, &d).unwrap();
        let Plan::Draw(first) = &plans[0] else { panic!("第一行要畫") };
        assert_eq!((first[0].dx, first[0].dy, first[0].color), (0, 0, 7 + 64));
        assert_eq!((first[1].dx, first[1].color), (14, 2 + 64), "喜悅接在「他在衝突中感到」後面，用 joy 的顏色");
        assert!(matches!(plans[1], Plan::Skip) && matches!(plans[2], Plan::Skip), "中文一行，其餘英文行不畫");
    }
}
