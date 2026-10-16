//! 排段落用的字寬（一格 = UNIT 單位）。
//!
//! 一般：英數一格、中文兩格（字圖集一字兩格）。整句貼圖開著時（compact，見 overlay.rs）改用字型的實際字寬：
//! 段落照字寬換行、填滿框寬，替身字串也照字寬縮短（見 Dict::para_entry）。字寬在畫字圖集時依當下的格子大小量好（set_advances）。

use std::collections::HashMap;
use std::sync::OnceLock;
use std::sync::atomic::{AtomicBool, Ordering};

use crate::cp437;

/// 一格的單位數：字寬逐字無條件進位（見 atlas::advances），單位要夠細，一行累積的誤差才不會變成明顯的空隙
pub const UNIT: usize = 64;

static COMPACT: AtomicBool = AtomicBool::new(false);
static ADVANCES: OnceLock<HashMap<char, u16>> = OnceLock::new();

/// 量好的字寬（單位）；畫字圖集時呼叫一次
pub fn set_advances(advances: HashMap<char, u16>) {
    let _ = ADVANCES.set(advances);
}

/// 整句貼圖開著（遊戲啟動時決定；換行與替身字串都跟著改，中途切換要重開遊戲）
pub fn set_compact(on: bool) {
    COMPACT.store(on && ADVANCES.get().is_some(), Ordering::Release);
}
pub fn compact() -> bool {
    #[cfg(test)]
    if TEST_COMPACT.with(std::cell::Cell::get) {
        return true;
    }
    COMPACT.load(Ordering::Relaxed)
}

#[cfg(test)]
thread_local! {
    /// 測試用：只在這個執行緒開 compact（其他測試照一般字寬）
    static TEST_COMPACT: std::cell::Cell<bool> = const { std::cell::Cell::new(false) };
}

/// 測試用：這個執行緒改用 compact 字寬（字寬表全程只設一次）
#[cfg(test)]
pub fn test_compact(advances: impl FnOnce() -> HashMap<char, u16>) {
    let _ = ADVANCES.set(advances());
    TEST_COMPACT.with(|c| c.set(true));
}

/// 一般的字寬：CP437 有的字一格，其他（中文、全形標點）兩格
pub fn grid(c: char) -> usize {
    if cp437::byte_of(c).is_some() { UNIT } else { 2 * UNIT }
}

/// 目前排版用的字寬
pub fn units(c: char) -> usize {
    if compact() {
        if let Some(&u) = ADVANCES.get().and_then(|a| a.get(&c)) {
            return u as usize;
        }
    }
    grid(c)
}

pub fn width(s: &str) -> usize {
    s.chars().map(units).sum()
}

/// 單位換成格數（無條件進位）
pub fn cells(units: usize) -> usize {
    units.div_ceil(UNIT)
}


/// 一段字佔幾格：開頭的空白（縮排）一格一格算，其餘照目前的字寬（同 Dict::para_entry 的替身字串長度）
pub fn cells_of(s: &str) -> usize {
    let lead = s.len() - s.trim_start_matches(' ').len();
    lead + cells(width(&s[lead..]))
}