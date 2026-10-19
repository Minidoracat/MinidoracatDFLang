//! 遊戲自己的輸入框（傳說清單的篩選、庫存搜尋、命名、數量）裡玩家打的字照原樣畫：打出字典裡有的字（Gallows、Gems）時，
//! 輸入框不能變成中文。整段和輸入框的字相同才照原文，所以清單裡剛好和打的字同字的那一列也照英文。

use parking_lot::Mutex;
use std::sync::atomic::{AtomicBool, Ordering};

/// 遊戲全域物件 main_interface 裡輸入框的 std::string 位址：Lua 啟動時從 DFHack 的結構定義找好交來（set_fields），整局不變
static FIELDS: Mutex<Vec<usize>> = Mutex::new(Vec::new());
/// Lua 每幀送來的字：畫面物件裡的輸入框（傳說清單的篩選）畫面關掉後位址就無效，所以送內容不送位址
static EXTRA: Mutex<Vec<u8>> = Mutex::new(Vec::new());
/// 這一幀輸入框裡的字（去掉頭尾空白，不含空的）
static CURRENT: Mutex<Vec<Vec<u8>>> = Mutex::new(Vec::new());
/// CURRENT 有沒有東西：平常輸入框都是空的，畫字時不必上鎖
static ANY: AtomicBool = AtomicBool::new(false);

pub fn set_fields(addrs: Vec<usize>) {
    *FIELDS.lock() = addrs;
}

pub fn set_extra(text: &[u8]) {
    let mut extra = EXTRA.lock();
    extra.clear();
    extra.extend_from_slice(text);
}

/// 每幀畫字前（render_things 開頭）讀一次輸入框現在的字
pub fn refresh() {
    let fields = FIELDS.lock();
    let extra = EXTRA.lock();
    let mut current = CURRENT.lock();
    current.clear();
    let texts = fields.iter().map(|&a| unsafe { crate::lines::str_bytes(a) }).chain(std::iter::once(extra.as_slice()));
    for text in texts.map(<[u8]>::trim_ascii).filter(|t| !t.is_empty() && t.len() < 256) {
        current.push(text.to_vec());
    }
    ANY.store(!current.is_empty(), Ordering::Relaxed);
}

/// 整段就是輸入框裡的字（遊戲畫輸入中的字時後面接著游標「_」也算）
pub fn is_typed(text: &[u8]) -> bool {
    if !ANY.load(Ordering::Relaxed) {
        return false;
    }
    let text = text.trim_ascii();
    let bare = text.strip_suffix(b"_").unwrap_or(text).trim_ascii_end();
    CURRENT.lock().iter().any(|t| t.as_slice() == text || t.as_slice() == bare)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn text_typed_into_a_game_input_box_is_matched_whole() {
        set_extra(b"Gallows");
        refresh();
        assert!(is_typed(b"Gallows"));
        assert!(is_typed(b" Gallows_"), "輸入中的游標、頭尾空白不論");
        assert!(!is_typed(b"Gallows of Doom"), "整段相同才算");
        set_extra(b"");
        refresh();
        assert!(!is_typed(b"Gallows"), "清空後照常翻");
    }
}
