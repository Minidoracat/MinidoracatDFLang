//! 自動換行的提示文字（工具列按鈕的提示框、訪客說明、人物頁的近期想法等）。
//!
//! 遊戲用 wrap_text（內部是 wrap_lines，也有直接呼叫它的）把一句英文依寬度切成好幾行（vector<string*>），再一行一行畫，整句比對只看得到片段。
//! 這裡在切好之後整段查字典（查不到就逐句查，見 Dict::translate_sentences）：有譯文就依中文規則重新換行，
//! 第 k 行中文交給第 k 行英文（載體）畫，多出來的英文行不畫；中文行比英文多時掛在最後一行下面。
//! 沒有譯文的行登記成「照畫英文」，不拿片段查字典。和段落一樣只在畫字時換（見 mtb.rs），遊戲資料不改。
//! 大部分提示在遊戲啟動時（靜態初始化，早於本引擎載入）就切好了：enable 時從呼叫 wrap_text 的程式碼找出來補做。

use std::collections::HashMap;
use std::sync::{Arc, LazyLock};

use parking_lot::Mutex;

use crate::dict::Dict;
use crate::mtb::{Piece, Plan};

struct Line {
    /// 登記時的英文：行字串釋放後位址可能給別的字串用，內容相同才照計畫畫
    text: Box<[u8]>,
    plan: Plan,
    /// 啟動時就切好的提示（常駐），清表時保留
    keep: bool,
}

static LINES: LazyLock<Mutex<HashMap<usize, Line>>> = LazyLock::new(|| Mutex::new(HashMap::new()));
/// ponytail: 行字串釋放時沒有通知，執行時切的行只在超過上限時整批清掉（常駐的保留）。
const LINES_LIMIT: usize = 100_000;

/// 同一句、同寬、同行數算出來的計畫（None 是沒有譯文）：每幀重切的提示不必每次重算
type Key = (Vec<u8>, i32, usize);
static PLANS: LazyLock<Mutex<HashMap<Key, Option<Arc<[Plan]>>>>> = LazyLock::new(|| Mutex::new(HashMap::new()));
const PLANS_LIMIT: usize = 20_000;

unsafe fn read<T: Copy>(addr: usize) -> T {
    unsafe { *(addr as *const T) }
}

/// MSVC std::string 的內容（16 bytes 內建緩衝區或長字串指標、長度、容量）
pub unsafe fn str_bytes<'a>(s: usize) -> &'a [u8] {
    unsafe {
        let (size, cap): (usize, usize) = (read(s + 16), read(s + 24));
        let data = if cap > 15 { read::<*const u8>(s) } else { s as *const u8 };
        std::slice::from_raw_parts(data, size)
    }
}

/// vector<string*> 的元素（字串位址）
pub unsafe fn vec_items<'a>(v: usize) -> &'a [usize] {
    unsafe {
        let (begin, end): (usize, usize) = (read(v), read(v + 8));
        if begin == 0 || end < begin {
            return &[];
        }
        std::slice::from_raw_parts(begin as *const usize, (end - begin) / 8)
    }
}

pub unsafe fn vec_len(v: usize) -> usize {
    unsafe { vec_items(v).len() }
}

/// 遊戲自己切好的行接回一句（開局畫面的外觀、性格段落，見 hooks.rs）：換行時遊戲把行首空白跳過，
/// 句間的兩個空白在換行處可能只剩一個；句末標點後只剩一個空白、下一行大寫開頭時補回一個（逐句查要靠它切句）。
pub unsafe fn join(lines: &[usize]) -> Vec<u8> {
    let mut text: Vec<u8> = Vec::new();
    for &s in lines {
        let line = unsafe { str_bytes(s) };
        if let [.., end, b' '] = text[..] {
            if b".!?".contains(&end) && line.first().is_some_and(u8::is_ascii_uppercase) {
                text.push(b' ');
            }
        }
        text.extend_from_slice(line);
    }
    text
}

/// 一句切成 n 行時每行的計畫；沒有譯文或缺字圖時回傳 None。
fn compute(text: &[u8], width: i32, n: usize, dict: &Dict) -> Option<Arc<[Plan]>> {
    let translation: String = dict.translate_sentences(text).ok()?.into_iter().map(|(t, _)| t).collect();
    // 提示文字沒有換色；顏色沿用遊戲畫這一行時的顏色
    let segments = crate::mtb::wrap(&translation, width.max(1) as usize, [0; 3], |c| c);
    let mut pieces: Vec<Vec<Piece>> = (0..n).map(|_| Vec::new()).collect();
    for (px, py, t, _) in segments {
        let carrier = (py as usize).min(n - 1);
        pieces[carrier].push(Piece { dx: px, dy: py - carrier as i32, color: None, entry: dict.text_entry(&t)? });
    }
    Some(pieces.into_iter().map(|p| if p.is_empty() { Plan::Skip } else { Plan::Draw(p.into()) }).collect())
}

/// 原文 text 切成的這幾行（字串位址）登記畫字計畫；回傳這句有沒有譯文。
pub unsafe fn plan(lines: &[usize], text: &[u8], width: i32, dict: &Dict, keep: bool) -> bool {
    if lines.is_empty() {
        return false;
    }
    let plans = {
        let mut cache = PLANS.lock();
        if cache.len() > PLANS_LIMIT {
            cache.clear();
        }
        cache.entry((text.to_vec(), width, lines.len())).or_insert_with(|| compute(text, width, lines.len(), dict)).clone()
    };
    let translated = plans.is_some();
    // 沒有譯文：每一行照畫英文，畫到時才把整句記成未翻譯（啟動時補登記的提示不是每句都會看到）
    let plans = plans.unwrap_or_else(|| vec![Plan::Missing(text.into()); lines.len()].into());
    let mut table = LINES.lock();
    if table.len() + lines.len() > LINES_LIMIT {
        table.retain(|_, l| l.keep);
    }
    for (&s, plan) in lines.iter().zip(plans.iter()) {
        table.insert(s, Line { text: unsafe { str_bytes(s) }.into(), plan: plan.clone(), keep });
    }
    translated
}

/// 畫字 hook 用：這個字串是自動換行的一行時，回傳它的畫字計畫。
pub unsafe fn line_plan(s: usize) -> Option<Plan> {
    let table = LINES.lock();
    let line = table.get(&s)?;
    (*line.text == *unsafe { str_bytes(s) }).then(|| line.plan.clone())
}

/// 遊戲啟動時就切好的提示：從呼叫 wrap_text 的程式碼找出 (切好的行, 原文, 寬度)，補登記。
/// 只在切好的行和原文的單字完全相同時才算（找錯字串常數、或同一個 vector 接了好幾句時就略過）。
/// 回傳有譯文的句數。
pub unsafe fn plan_static(wrap_text: usize, dict: &Dict) -> usize {
    let words = |b: &[u8]| b.split(|&c| c == b' ').filter(|w| !w.is_empty()).map(<[u8]>::to_vec).collect::<Vec<_>>();
    let mut translated = 0;
    for (vector, string, width) in crate::scan::constant_calls(wrap_text) {
        let lines = unsafe { vec_items(vector) };
        let Some(text) = crate::scan::rdata_cstr(string).filter(|t| !lines.is_empty() && t.len() < 4096) else { continue };
        let joined: Vec<u8> = lines.iter().flat_map(|&s| unsafe { str_bytes(s) }.iter().copied()).collect();
        if words(&joined) == words(text) && unsafe { plan(lines, text, width, dict, true) } {
            translated += 1;
        }
    }
    translated
}

#[cfg(test)]
mod tests {
    use super::*;

    /// 測試用的 MSVC std::string（內容放在長字串模式的獨立緩衝區）
    struct Strings(Vec<(Box<[u64; 4]>, Box<[u8]>)>);

    impl Strings {
        fn new(lines: &[&str]) -> Strings {
            Strings(
                lines
                    .iter()
                    .map(|t| {
                        let mut buf: Box<[u8]> = format!("{t}\0").into_bytes().into();
                        let s = Box::new([buf.as_mut_ptr() as u64, 0, t.len() as u64, 31]);
                        (s, buf)
                    })
                    .collect(),
            )
        }

        fn addrs(&self) -> Vec<usize> {
            self.0.iter().map(|(s, _)| s.as_ptr() as usize).collect()
        }
    }

    fn kinds(addrs: &[usize]) -> Vec<String> {
        addrs
            .iter()
            .map(|&s| match unsafe { line_plan(s) } {
                Some(Plan::Plain) => "plain".to_owned(),
                Some(Plan::Missing(t)) => format!("missing {}", String::from_utf8_lossy(&t)),
                Some(Plan::Skip) => "skip".to_owned(),
                Some(Plan::Draw(p)) => p.iter().map(|p| format!("{},{}", p.dx, p.dy)).collect::<Vec<_>>().join(" "),
                None => "none".to_owned(),
            })
            .collect()
    }

    #[test]
    fn translated_lines_are_drawn_by_matching_english_lines() {
        let mut d = Dict::default();
        assert!(d.set_glyphs("建立活動區以便規劃工作", vec![0; 22]));
        d.load_tsv("Establish burrows to organize work.\t建立活動區以便規劃工作\n");
        // 英文切成 3 行；中文 22 格在寬 12 格切成 2 行：第 1、2 行英文各畫一行中文，第 3 行不畫
        let en = Strings::new(&["Establish ", "burrows to ", "organize work."]);
        let addrs = en.addrs();
        assert!(unsafe { plan(&addrs, b"Establish burrows to organize work.", 12, &d, false) });
        assert_eq!(kinds(&addrs), ["0,0", "0,0", "skip"]);
    }

    #[test]
    fn extra_chinese_lines_hang_below_the_last_english_line() {
        let mut d = Dict::default();
        assert!(d.set_glyphs("一二三四五六", vec![0; 12]));
        d.load_tsv("one two\t一二三四五六\n");
        // 英文 1 行、中文在寬 4 格切成 3 行：都由唯一的英文行畫，往下 0、1、2 列
        let en = Strings::new(&["one two"]);
        let addrs = en.addrs();
        assert!(unsafe { plan(&addrs, b"one two", 4, &d, false) });
        assert_eq!(kinds(&addrs), ["0,0 0,1 0,2"]);
    }

    #[test]
    fn untranslated_lines_stay_english_and_reused_addresses_do_not_match() {
        let d = Dict::default();
        let mut en = Strings::new(&["Hold left ", "button."]);
        let addrs = en.addrs();
        assert!(!unsafe { plan(&addrs, b"Hold left button.", 10, &d, false) });
        // 畫到時記整句，不記片段
        assert_eq!(kinds(&addrs), ["missing Hold left button.", "missing Hold left button."]);
        // 位址還在、內容變了（行字串釋放後給別的字串用）：計畫不算數
        en.0[0].0[2] = 4;
        assert_eq!(kinds(&addrs[..1]), ["none"]);
    }

    #[test]
    fn joined_lines_restore_sentence_spacing_lost_at_breaks() {
        let en = Strings::new(&["His beard is combed. ", "His hair is ", "auburn.  ", "He is old."]);
        assert_eq!(unsafe { join(&en.addrs()) }, b"His beard is combed.  His hair is auburn.  He is old.", "句末只剩一個空白才補，句中照接");
    }
}
