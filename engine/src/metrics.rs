//! 排段落用的字寬（一格 = UNIT 單位）。
//!
//! 一般：英數一格、中文兩格（字圖集一字兩格）。整句貼圖開著時（compact，見 overlay.rs）改用字型的實際字寬：
//! 段落照字寬換行、填滿框寬，替身字串也照字寬縮短（見 Dict::para_entry）。字寬在換字型、字重、字的大小時依當下的格子大小量好（set_advances）。

use std::collections::HashMap;
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};

use parking_lot::RwLock;

use crate::cp437;

/// 一格的單位數：字寬逐字無條件進位（見 atlas::advances），單位要夠細，一行累積的誤差才不會變成明顯的空隙
pub const UNIT: usize = 64;

static COMPACT: AtomicBool = AtomicBool::new(false);
static ADVANCES: RwLock<Option<Arc<HashMap<char, u16>>>> = RwLock::new(None);

/// 量好的字寬（單位）：遊戲啟動時、玩家換字重、字的大小或語言時換一份（lib.rs 的 set_typeface）
pub fn set_advances(advances: HashMap<char, u16>) {
    *ADVANCES.write() = Some(Arc::new(advances));
}

/// 整句貼圖開著：換行與替身字串都照字寬（隨時可切；切完要重算段落，見 lib.rs 的 refresh）
pub fn set_compact(on: bool) {
    COMPACT.store(on && ADVANCES.read().is_some(), Ordering::Release);
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

/// 測試用：這個執行緒改用 compact 字寬。字寬表全程只設一次，各測試共用這一份：
/// 「他很有幽默感。不太在意」一字 1.375 格（88 單位）、空白約 0.3 格（20 單位）
#[cfg(test)]
pub fn test_compact() {
    let (cjk, space) = ((UNIT * 11 / 8) as u16, (UNIT * 5 / 16) as u16);
    let mut advances = ADVANCES.write();
    if advances.is_none() {
        *advances = Some(Arc::new("他很有幽默感。不太在意".chars().map(|c| (c, cjk)).chain([(' ', space)]).collect()));
    }
    TEST_COMPACT.with(|c| c.set(true));
}

/// 一般的字寬：CP437 有的字一格，其他（中文、全形標點）兩格
pub fn grid(c: char) -> usize {
    if cp437::byte_of(c).is_some() { UNIT } else { 2 * UNIT }
}

/// 目前排版用的字寬
pub fn units(c: char) -> usize {
    if compact() {
        if let Some(&u) = ADVANCES.read().as_ref().and_then(|a| a.get(&c)) {
            return u as usize;
        }
    }
    grid(c)
}

pub fn width(s: &str) -> usize {
    if !compact() {
        return s.chars().map(grid).sum();
    }
    let advances = ADVANCES.read();
    s.chars().map(|c| advances.as_ref().and_then(|a| a.get(&c)).map_or_else(|| grid(c), |&u| u as usize)).sum()
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