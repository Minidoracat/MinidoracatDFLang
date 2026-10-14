//! 在記憶體裡的 `Dwarf Fortress.exe` 程式碼段找特徵碼（engine/patterns.tsv，"?? " 是萬用位元組）。

use std::ffi::c_void;

#[link(name = "kernel32")]
unsafe extern "system" {
    fn GetModuleHandleW(name: *const u16) -> *mut c_void;
}

/// 主程式某個區段（.text、.rdata…）在記憶體裡的內容。
fn section(name: &[u8]) -> Option<&'static [u8]> {
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
            let raw = std::slice::from_raw_parts(header, 8);
            if raw.starts_with(name) && raw[name.len()..].iter().all(|&b| b == 0) {
                let size = *(header.add(8) as *const u32) as usize;
                let rva = *(header.add(12) as *const u32) as usize;
                return Some(std::slice::from_raw_parts(base.add(rva), size));
            }
            header = header.add(40);
        }
        None
    }
}

fn text_section() -> Option<&'static [u8]> {
    section(b".text")
}

/// .rdata 裡從 addr 開始、以 NUL 結尾的字串；不在 .rdata 或找不到結尾時回傳 None。
pub fn rdata_cstr(addr: usize) -> Option<&'static [u8]> {
    let rdata = section(b".rdata")?;
    let start = addr.checked_sub(rdata.as_ptr() as usize).filter(|&o| o < rdata.len())?;
    let rest = &rdata[start..];
    Some(&rest[..rest.iter().position(|&b| b == 0)?])
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

/// 呼叫 target 時三個參數都是常數的地方：`lea rcx,[rip+全域變數]`（緊接在 call 前）、`mov r8d,寬度`、
/// 前面剛組好的字串常數 `lea rdx,[rip+字串]`。回傳 (全域變數位址, 字串位址, 寬度)。
/// 遊戲啟動時切好提示文字的初始化程式就是這個樣子；看起來不一樣的呼叫略過（交給執行時的 hook）。
pub fn constant_calls(target: usize) -> Vec<(usize, usize, i32)> {
    let Some(text) = text_section() else { return Vec::new() };
    let base = text.as_ptr() as usize;
    let rel = |at: usize| i32::from_le_bytes(text[at..at + 4].try_into().unwrap()) as isize;
    // RIP 相對位移：從下一條指令的位址算起
    let target_of = |disp_at: usize, next: usize| (base + next).wrapping_add_signed(rel(disp_at));
    let mut out = Vec::new();
    for i in 64..text.len().saturating_sub(5) {
        if text[i] != 0xe8 || target_of(i + 1, i + 5) != target || text[i - 7..i - 4] != [0x48, 0x8d, 0x0d] {
            continue;
        }
        let vector = target_of(i - 4, i);
        let width = (i - 7 - 16..i - 7).rev().find(|&k| text[k..k + 2] == [0x41, 0xb8]).map(|k| rel(k + 2) as i32);
        let string = (i - 7 - 48..i - 7).rev().find(|&k| text[k..k + 3] == [0x48, 0x8d, 0x15]).map(|k| target_of(k + 3, k + 7));
        if let (Some(width), Some(string)) = (width, string) {
            out.push((vector, string, width));
        }
    }
    out
}
