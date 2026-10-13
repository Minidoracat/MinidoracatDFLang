//! 在記憶體裡的 `Dwarf Fortress.exe` 程式碼段找特徵碼（engine/patterns.tsv，"?? " 是萬用位元組）。

use std::ffi::c_void;

#[link(name = "kernel32")]
unsafe extern "system" {
    fn GetModuleHandleW(name: *const u16) -> *mut c_void;
}

/// 主程式 .text 段的起點與長度。
fn text_section() -> Option<&'static [u8]> {
    unsafe {
        let base = GetModuleHandleW(std::ptr::null()) as *const u8;
        if base.is_null() {
            return None;
        }
        let pe = base.add(*(base.add(0x3c) as *const u32) as usize);
        if *(pe as *const u32) != 0x0000_4550 {
            return None;
        }
        let sections = *(pe.add(6) as *const u16) as usize;
        let optional_size = *(pe.add(20) as *const u16) as usize;
        let mut header = pe.add(24 + optional_size);
        for _ in 0..sections {
            if std::slice::from_raw_parts(header, 8).starts_with(b".text") {
                let size = *(header.add(8) as *const u32) as usize;
                let rva = *(header.add(12) as *const u32) as usize;
                return Some(std::slice::from_raw_parts(base.add(rva), size));
            }
            header = header.add(40);
        }
        None
    }
}

pub fn parse(pattern: &str) -> Option<Vec<Option<u8>>> {
    pattern
        .split_whitespace()
        .map(|t| if t == "??" { Some(None) } else { u8::from_str_radix(t, 16).ok().map(Some) })
        .collect()
}

/// 回傳第一個命中的位址與命中數（最多數到 2，夠判斷是否唯一）。
pub fn find(pattern: &[Option<u8>]) -> (Option<usize>, usize) {
    let (Some(text), Some(Some(first))) = (text_section(), pattern.first()) else {
        return (None, 0);
    };
    let mut hits = 0;
    let mut found = None;
    let last = text.len().saturating_sub(pattern.len());
    for i in 0..=last {
        if text[i] == *first
            && pattern.iter().zip(&text[i..]).all(|(p, b)| p.is_none_or(|p| p == *b))
        {
            hits += 1;
            found.get_or_insert(text.as_ptr() as usize + i);
            if hits == 2 {
                break;
            }
        }
    }
    (found, hits)
}
