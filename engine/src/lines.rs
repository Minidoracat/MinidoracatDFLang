//! 自動換行的提示文字（工具列按鈕的提示框、訪客說明、人物頁的近期想法等）。
//!
//! 遊戲用 wrap_text（內部是 wrap_lines，也有直接呼叫它的）把一句英文依寬度切成好幾行（vector<string*>），再一行一行畫，整句比對只看得到片段。
//! 這裡在切好之後整段查字典（查不到就逐句查，見 Dict::translate_sentences）：有譯文就依中文規則重新換行，
//! 第 k 行中文交給第 k 行英文（載體）畫，多出來的英文行不畫；中文行比英文多時掛在最後一行下面。
//! 沒有譯文的行登記成「照畫英文」，不拿片段查字典（段落缺句時其餘照翻開著時，有幾句翻得出來就照翻、其餘照原文接在譯文裡）。
//! 和段落一樣只在畫字時換（見 mtb.rs），遊戲資料不改。
//! 大部分提示在遊戲啟動時（靜態初始化，早於本引擎載入）就切好了：enable 時從呼叫 wrap_text 的程式碼找出來補做。
//! 玩家在設定視窗改了選項或語言以後，畫到一行時照新的設定重算整句（見 line_plan），不必等遊戲重切。

use std::collections::HashMap;
use std::sync::{Arc, LazyLock};

use parking_lot::Mutex;

use crate::dict::Dict;
use crate::hooks::Stamp;
use crate::mtb::{Piece, Plan};

/// 一句切成的幾行：登記時的原文、寬度與每一行的位址（設定換了以後照它重算，見 line_plan）
struct Group {
    text: Box<[u8]>,
    width: i32,
    lines: Box<[usize]>,
}

struct Line {
    /// 登記時的英文：行字串釋放後位址可能給別的字串用，內容相同才照計畫畫
    text: Box<[u8]>,
    plan: Plan,
    /// 啟動時就切好的提示（常駐），清表時保留
    keep: bool,
    group: Arc<Group>,
    /// 計畫照哪一版設定、哪一份字典算的
    stamp: Stamp,
}

static LINES: LazyLock<Mutex<HashMap<usize, Line>>> = LazyLock::new(|| Mutex::new(HashMap::new()));
/// ponytail: 行字串釋放時沒有通知，執行時切的行只在超過上限時整批清掉（常駐的保留）。
const LINES_LIMIT: usize = 100_000;

/// 同一句、同寬、同行數、同一版設定算出來的計畫（None 是沒有譯文）：每幀重切的提示不必每次重算
type Key = (Vec<u8>, i32, usize, Stamp);
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

/// 一句切成 n 行時每行的計畫（沒有譯文或缺字圖時是 None），以及查不到的句子（段落缺句時其餘照翻開著、照原文接在譯文裡的句子）。
fn compute(text: &[u8], width: i32, n: usize, dict: &Dict) -> (Option<Arc<[Plan]>>, Vec<Vec<u8>>) {
    let (pieces, missing) = dict.translate_sentences(text);
    let Some(pieces) = pieces else { return (None, missing) };
    let translation: String = pieces.into_iter().map(|(t, _)| t).collect();
    // 提示文字沒有換色；顏色沿用遊戲畫這一行時的顏色
    let segments = crate::mtb::wrap(&translation, width.max(1) as usize, [0; 3], |c| c);
    let mut pieces: Vec<Vec<Piece>> = (0..n).map(|_| Vec::new()).collect();
    for (px, py, t, _) in segments {
        let carrier = (py as usize).min(n - 1);
        let Some(entry) = dict.para_entry(&t) else { return (None, missing) };
        pieces[carrier].push(Piece { dx: px, dy: py - carrier as i32, color: None, entry });
    }
    (Some(pieces.into_iter().map(|p| if p.is_empty() { Plan::Skip } else { Plan::Draw(p.into()) }).collect()), missing)
}

/// 一句切成 n 行的計畫（快取）。第一次算時，段落缺句時其餘照翻開著、照原文接在譯文裡的句子交給 note_miss
/// （整句沒有譯文的照舊畫到時才記）。
fn cached(text: &[u8], width: i32, n: usize, dict: &Dict, stamp: Stamp, note_miss: &mut impl FnMut(&[u8])) -> Option<Arc<[Plan]>> {
    let mut cache = PLANS.lock();
    if cache.len() > PLANS_LIMIT {
        cache.clear();
    }
    cache
        .entry((text.to_vec(), width, n, stamp))
        .or_insert_with(|| {
            let (plans, missing) = compute(text, width, n, dict);
            if plans.is_some() {
                missing.iter().for_each(|m| note_miss(m));
            }
            plans
        })
        .clone()
}

/// 沒有譯文：每一行照畫英文，畫到時才把整句記成未翻譯（啟動時補登記的提示不是每句都會看到）
fn english(text: &[u8], n: usize) -> Arc<[Plan]> {
    vec![Plan::Missing(text.into()); n].into()
}

/// 原文 text 切成的這幾行（字串位址）登記畫字計畫；回傳這句有沒有譯文。
pub unsafe fn plan(lines: &[usize], text: &[u8], width: i32, dict: &Dict, keep: bool, mut note_miss: impl FnMut(&[u8])) -> bool {
    if lines.is_empty() {
        return false;
    }
    // 先記版本再算：算到一半玩家改了設定，這份計畫就算舊的，畫到時重算
    let stamp = Stamp::of(dict);
    let plans = cached(text, width, lines.len(), dict, stamp, &mut note_miss);
    let translated = plans.is_some();
    let plans = plans.unwrap_or_else(|| english(text, lines.len()));
    let group = Arc::new(Group { text: text.into(), width, lines: lines.into() });
    let mut table = LINES.lock();
    if table.len() + lines.len() > LINES_LIMIT {
        table.retain(|_, l| l.keep);
    }
    for (&s, plan) in lines.iter().zip(plans.iter()) {
        table.insert(s, Line { text: unsafe { str_bytes(s) }.into(), plan: plan.clone(), keep, group: group.clone(), stamp });
    }
    translated
}

/// 畫字 hook 用：這個字串是自動換行的一行時，回傳它的畫字計畫。玩家改過設定或語言（dict 是現在的字典）時，
/// 照現在的設定重算這一行所屬的整句，同一句的其他行一起換。
pub unsafe fn line_plan(s: usize, dict: &Dict) -> Option<Plan> {
    let mut table = LINES.lock();
    let line = table.get(&s).filter(|l| *l.text == *unsafe { str_bytes(s) })?;
    let stamp = Stamp::of(dict);
    if line.stamp == stamp {
        return Some(line.plan.clone());
    }
    let group = line.group.clone();
    let plans = cached(&group.text, group.width, group.lines.len(), dict, stamp, &mut |_| {})
        .unwrap_or_else(|| english(&group.text, group.lines.len()));
    for (addr, plan) in group.lines.iter().zip(plans.iter()) {
        if let Some(l) = table.get_mut(addr).filter(|l| Arc::ptr_eq(&l.group, &group)) {
            l.plan = plan.clone();
            l.stamp = stamp;
        }
    }
    table.get(&s).map(|l| l.plan.clone())
}

/// status 用：(登記的行數, 計畫快取筆數)
pub fn counts() -> (usize, usize) {
    let lines = LINES.lock().len();
    (lines, PLANS.lock().len())
}

/// 這幾行都已經照現在的內容登記過畫字計畫（每幀重畫的框不必每幀重算、重登記）。
pub unsafe fn planned(lines: &[usize]) -> bool {
    let table = LINES.lock();
    lines.iter().all(|&s| table.get(&s).is_some_and(|l| *l.text == *unsafe { str_bytes(s) }))
}

/// 遊戲啟動時就切好的提示：從呼叫 wrap_text 的程式碼找出 (切好的行, 原文, 寬度)，補登記。
/// 只在切好的行和原文的單字完全相同時才算（找錯字串常數、或同一個 vector 接了好幾句時就略過）。
/// 回傳有譯文的句數。這時還沒畫到，段落缺句時其餘照翻接上的原文句子不記進未翻譯紀錄。
pub unsafe fn plan_static(wrap_text: usize, dict: &Dict) -> usize {
    let words = |b: &[u8]| b.split(|&c| c == b' ').filter(|w| !w.is_empty()).map(<[u8]>::to_vec).collect::<Vec<_>>();
    let mut translated = 0;
    for (vector, string, width) in crate::scan::constant_calls(wrap_text) {
        let lines = unsafe { vec_items(vector) };
        let Some(text) = crate::scan::rdata_cstr(string).filter(|t| !lines.is_empty() && t.len() < 4096) else { continue };
        let joined: Vec<u8> = lines.iter().flat_map(|&s| unsafe { str_bytes(s) }.iter().copied()).collect();
        if words(&joined) == words(text) && unsafe { plan(lines, text, width, dict, true, |_| {}) } {
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

    fn kinds(addrs: &[usize], d: &Dict) -> Vec<String> {
        addrs
            .iter()
            .map(|&s| match unsafe { line_plan(s, d) } {
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
        assert!(unsafe { plan(&addrs, b"Establish burrows to organize work.", 12, &d, false, |_| {}) });
        assert_eq!(kinds(&addrs, &d), ["0,0", "0,0", "skip"]);
    }

    #[test]
    fn extra_chinese_lines_hang_below_the_last_english_line() {
        let mut d = Dict::default();
        assert!(d.set_glyphs("一二三四五六", vec![0; 12]));
        d.load_tsv("one two\t一二三四五六\n");
        // 英文 1 行、中文在寬 4 格切成 3 行：都由唯一的英文行畫，往下 0、1、2 列
        let en = Strings::new(&["one two"]);
        let addrs = en.addrs();
        assert!(unsafe { plan(&addrs, b"one two", 4, &d, false, |_| {}) });
        assert_eq!(kinds(&addrs, &d), ["0,0 0,1 0,2"]);
    }

    #[test]
    fn untranslated_lines_stay_english_and_reused_addresses_do_not_match() {
        let d = Dict::default();
        let mut en = Strings::new(&["Hold left ", "button."]);
        let addrs = en.addrs();
        assert!(!unsafe { plan(&addrs, b"Hold left button.", 10, &d, false, |_| {}) });
        // 畫到時記整句，不記片段
        assert_eq!(kinds(&addrs, &d), ["missing Hold left button.", "missing Hold left button."]);
        // 位址還在、內容變了（行字串釋放後給別的字串用）：計畫不算數
        en.0[0].0[2] = 4;
        assert_eq!(kinds(&addrs[..1], &d), ["none"]);
    }

    #[test]
    fn planned_lines_follow_a_settings_change_without_rewrapping() {
        let mut d = Dict::default();
        assert!(d.set_glyphs("勞動結束。", vec![0; 10]));
        d.load_tsv("Labor ends.\t勞動結束。\n");
        let en = Strings::new(&["Labor ends.  ", "Greed went home."]);
        let addrs = en.addrs();
        assert!(!unsafe { plan(&addrs, b"Labor ends.  Greed went home.", 30, &d, false, |_| {}) }, "缺一句就整句照英文");
        // 遊戲中途打開「段落缺句時其餘照翻」（lib.rs 的 refresh）：遊戲不會重切這段，畫到時照新設定重算
        d.set_options(crate::dict::Options { partial: true, ..Default::default() });
        crate::hooks::bump_epoch();
        assert_eq!(kinds(&addrs, &d), ["0,0", "skip"]);
    }

    #[test]
    fn joined_lines_restore_sentence_spacing_lost_at_breaks() {
        let en = Strings::new(&["His beard is combed. ", "His hair is ", "auburn.  ", "He is old."]);
        assert_eq!(unsafe { join(&en.addrs()) }, b"His beard is combed.  His hair is auburn.  He is old.", "句末只剩一個空白才補，句中照接");
    }
}
