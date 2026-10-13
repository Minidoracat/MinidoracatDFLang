//! 攔遊戲的四個寫字函式：字典有這句就改寫替身字串，原函式寫完後把佔位格換成字圖。
//!
//! 版面、裁切、顏色都交給遊戲原本的函式處理；這裡只多做一件事：在佔位格的
//! screentexpos 填字圖、在 screentexpos_flag 加 ADDCOLOR，讓渲染器用該格前景色為白色字圖上色。

use std::collections::HashSet;
use std::ffi::CStr;
use std::fs::File;
use std::io::Write;
use std::sync::atomic::{AtomicBool, AtomicPtr, AtomicU64, AtomicUsize, Ordering};
use std::sync::OnceLock;

use parking_lot::Mutex;
use retour::RawDetour;

use crate::cp437;
use crate::dict::Dict;

/// screentexpos_flag 位元（DFHack Screen.cpp）：ADDCOLOR 以該格前景色為貼圖上色；
/// TOP_OF_TEXT／BOTTOM_OF_TEXT 是「文字往下位移半格」的上下兩列，遊戲用半格字形分別畫上半與下半。
const FLAG_ADDCOLOR: u32 = 0x2;
const FLAG_BOTTOM_OF_TEXT: u32 = 0x10;

/// graphicst 欄位相對於 gps 的位移，由 Lua 依 DFHack 的結構定義算出後傳入。
#[derive(Clone, Copy)]
pub struct Layout {
    pub screenx: usize,
    pub screeny: usize,
    pub dimx: usize,
    pub dimy: usize,
    pub clipx: usize,
    pub clipy: usize,
    pub main: Layer,
    pub top: Layer,
}

#[derive(Clone, Copy)]
pub struct Layer {
    pub screen: usize,
    pub limit: usize,
    pub texpos: usize,
    pub flag: usize,
}

pub static LAYOUT: OnceLock<Layout> = OnceLock::new();
pub static DICT: AtomicPtr<Dict> = AtomicPtr::new(std::ptr::null_mut());
pub static ENABLED: AtomicBool = AtomicBool::new(false);
pub static HITS: AtomicU64 = AtomicU64::new(0);
pub static MISSES: AtomicU64 = AtomicU64::new(0);

#[repr(C)]
pub struct StdString {
    buf: [u8; 16],
    size: usize,
    cap: usize,
}

impl StdString {
    unsafe fn bytes(&self) -> &[u8] {
        let p = if self.cap > 15 { unsafe { *(self.buf.as_ptr() as *const *const u8) } } else { self.buf.as_ptr() };
        unsafe { std::slice::from_raw_parts(p, self.size) }
    }

    /// 指向既有緩衝區的長字串模式；被呼叫端只讀（const std::string&），不會釋放它。
    fn borrowed(bytes: &[u8]) -> StdString {
        let mut s = StdString { buf: [0; 16], size: bytes.len(), cap: bytes.len().max(16) };
        unsafe { *(s.buf.as_mut_ptr() as *mut *const u8) = bytes.as_ptr() };
        s
    }
}

type AddstFn = unsafe extern "C" fn(*mut u8, *const StdString, i32, i32);
type AddstFlagFn = unsafe extern "C" fn(*mut u8, *const StdString, i32, i32, u32);
type ColoredFn = unsafe extern "C" fn(*mut u8, *const u8, *const u8);

pub const HOOKS: [&str; 4] = ["addst", "addst_flag", "top_addst", "addcoloredst"];
static TRAMPOLINES: [AtomicUsize; 4] = [const { AtomicUsize::new(0) }; 4];

struct Detours(Vec<RawDetour>);
// RawDetour 只在持有鎖時由 Lua 指令啟停；遊戲執行緒只讀 TRAMPOLINES。
unsafe impl Send for Detours {}
static DETOURS: Mutex<Detours> = Mutex::new(Detours(Vec::new()));

pub fn attach(name: &str, target: usize) -> Result<(), String> {
    let index = HOOKS.iter().position(|h| *h == name).ok_or_else(|| format!("unknown hook {name}"))?;
    let hook: *const () = match index {
        0 => hook_addst as *const (),
        1 => hook_addst_flag as *const (),
        2 => hook_top_addst as *const (),
        _ => hook_addcoloredst as *const (),
    };
    let detour = unsafe { RawDetour::new(target as *const (), hook) }.map_err(|e| e.to_string())?;
    TRAMPOLINES[index].store(detour.trampoline() as *const () as usize, Ordering::Release);
    DETOURS.lock().0.push(detour);
    Ok(())
}

pub fn set_enabled(on: bool) -> Result<(), String> {
    let detours = DETOURS.lock();
    for d in &detours.0 {
        unsafe { if on { d.enable() } else { d.disable() } }.map_err(|e| e.to_string())?;
    }
    ENABLED.store(on && !detours.0.is_empty(), Ordering::Release);
    Ok(())
}

pub fn hook_count() -> usize {
    DETOURS.lock().0.len()
}

// ---- 未翻譯字串記錄（翻譯補漏用） ----

struct MissLog {
    seen: HashSet<Vec<u8>>,
    file: File,
}

static MISS_LOG: Mutex<Option<MissLog>> = Mutex::new(None);
static MISS_LOG_ON: AtomicBool = AtomicBool::new(false);

pub fn log_misses(path: Option<&str>) -> std::io::Result<()> {
    let mut log = MISS_LOG.lock();
    *log = match path {
        Some(p) => Some(MissLog { seen: HashSet::new(), file: File::options().create(true).append(true).open(p)? }),
        None => None,
    };
    MISS_LOG_ON.store(log.is_some(), Ordering::Release);
    Ok(())
}

fn note_miss(kind: &str, key: &[u8]) {
    MISSES.fetch_add(1, Ordering::Relaxed);
    if !MISS_LOG_ON.load(Ordering::Relaxed) || !key.iter().any(u8::is_ascii_alphabetic) {
        return;
    }
    if let Some(log) = MISS_LOG.lock().as_mut() {
        if log.seen.insert(key.to_vec()) {
            let _ = writeln!(log.file, "{kind}\t{}", cp437::decode(key));
        }
    }
}

// ---- 貼字圖 ----

unsafe fn read<T: Copy>(gps: *const u8, offset: usize) -> T {
    unsafe { *(gps.add(offset) as *const T) }
}

/// 把 (x0 + 格位, y) 上的佔位格換成字圖。條件與遊戲 addchar 相同：在裁切框內、不超出畫面緩衝區。
unsafe fn paint(gps: *mut u8, layer: &Layer, x0: i32, y: i32, glyphs: &[(usize, u32)], texpos: &[i32]) {
    let Some(layout) = LAYOUT.get() else { return };
    unsafe {
        let dimx: i32 = read(gps, layout.dimx);
        let dimy: i32 = read(gps, layout.dimy);
        let clip: [i32; 4] = [read(gps, layout.clipx), read(gps, layout.clipx + 4), read(gps, layout.clipy), read(gps, layout.clipy + 4)];
        if y < 0 || y >= dimy || y < clip[2] || y > clip[3] {
            return;
        }
        let screen: *const u8 = read(gps, layer.screen);
        let limit: *const u8 = read(gps, layer.limit);
        let tex: *mut i32 = read(gps, layer.texpos);
        let flag: *mut u32 = read(gps, layer.flag);
        if screen.is_null() || tex.is_null() || flag.is_null() {
            return;
        }
        for &(offset, glyph) in glyphs {
            for half in 0..2 {
                let x = x0 + offset as i32 + half;
                if x < 0 || x >= dimx || x < clip[0] || x > clip[1] {
                    continue;
                }
                let index = (x * dimy + y) as usize;
                if screen.add(index * 8) >= limit {
                    continue;
                }
                // 半格位移的文字（分頁標籤等）遊戲會在上下兩列各寫一次；字圖不能切半格，只畫在上列，下列留白
                if *flag.add(index) & FLAG_BOTTOM_OF_TEXT != 0 {
                    continue;
                }
                *tex.add(index) = texpos[glyph as usize * 2 + half as usize];
                *flag.add(index) |= FLAG_ADDCOLOR;
            }
        }
    }
}

/// 共用流程：查字典，命中就讓 `write` 寫替身字串，再貼字圖。回傳 false 表示要照原樣呼叫原函式。
unsafe fn translate_call(gps: *mut u8, kind: &str, key: &[u8], space: i32, layer: fn(&Layout) -> Layer,
                         write: impl FnOnce(&[u8])) -> bool {
    let dict = DICT.load(Ordering::Acquire);
    let Some(layout) = LAYOUT.get() else { return false };
    if dict.is_null() || !ENABLED.load(Ordering::Relaxed) {
        return false;
    }
    let dict = unsafe { &*dict };
    let Some(entry) = dict.entries.get(key) else {
        note_miss(kind, key);
        return false;
    };
    HITS.fetch_add(1, Ordering::Relaxed);
    let (cells, glyphs) = entry.fit(space);
    unsafe {
        let x0: i32 = read(gps, layout.screenx);
        let y: i32 = read(gps, layout.screeny);
        write(&entry.shadow()[..cells]);
        paint(gps, &layer(layout), x0, y, &entry.glyphs[..glyphs], &dict.texpos);
    }
    true
}

fn main_layer(l: &Layout) -> Layer {
    l.main
}

fn top_layer(l: &Layout) -> Layer {
    l.top
}

unsafe extern "C" fn hook_addst(gps: *mut u8, s: *const StdString, just: i32, space: i32) {
    let orig: AddstFn = unsafe { std::mem::transmute(TRAMPOLINES[0].load(Ordering::Acquire)) };
    let key = unsafe { (*s).bytes() };
    let done = unsafe { translate_call(gps, "addst", key, space, main_layer, |b| orig(gps, &StdString::borrowed(b), just, space)) };
    if !done {
        unsafe { orig(gps, s, just, space) }
    }
}

unsafe extern "C" fn hook_addst_flag(gps: *mut u8, s: *const StdString, just: i32, space: i32, sflag: u32) {
    let orig: AddstFlagFn = unsafe { std::mem::transmute(TRAMPOLINES[1].load(Ordering::Acquire)) };
    let key = unsafe { (*s).bytes() };
    let done = unsafe {
        translate_call(gps, "addst_flag", key, space, main_layer, |b| orig(gps, &StdString::borrowed(b), just, space, sflag))
    };
    if !done {
        unsafe { orig(gps, s, just, space, sflag) }
    }
}

unsafe extern "C" fn hook_top_addst(gps: *mut u8, s: *const StdString, just: i32, space: i32) {
    let orig: AddstFn = unsafe { std::mem::transmute(TRAMPOLINES[2].load(Ordering::Acquire)) };
    let key = unsafe { (*s).bytes() };
    let done = unsafe { translate_call(gps, "top_addst", key, 0, top_layer, |b| orig(gps, &StdString::borrowed(b), just, space)) };
    if !done {
        unsafe { orig(gps, s, just, space) }
    }
}

unsafe extern "C" fn hook_addcoloredst(gps: *mut u8, s: *const u8, colors: *const u8) {
    let orig: ColoredFn = unsafe { std::mem::transmute(TRAMPOLINES[3].load(Ordering::Acquire)) };
    let key = unsafe { CStr::from_ptr(s.cast()).to_bytes() };
    let done = unsafe {
        translate_call(gps, "addcoloredst", key, 0, main_layer, |b| {
            // 顏色字串與文字逐字對應；譯文長度不同，整句改用原文第一個字的顏色
            let first = if key.is_empty() { 0 } else { *colors };
            let mut text = b.to_vec();
            text.push(0);
            let mut col = vec![first; b.len()];
            col.push(0);
            orig(gps, text.as_ptr(), col.as_ptr());
        })
    };
    if !done {
        unsafe { orig(gps, s, colors) }
    }
}
