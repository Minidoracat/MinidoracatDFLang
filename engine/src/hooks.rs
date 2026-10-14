//! 攔遊戲的寫字函式：字典有這句就改寫替身字串，原函式寫完後把佔位格換成字圖。
//!
//! 版面、裁切、顏色都交給遊戲原本的函式處理；這裡只多做一件事：在佔位格的
//! screentexpos 填字圖、在 screentexpos_flag 加 ADDCOLOR，讓渲染器用該格前景色為白色字圖上色。
//! 另外攔段落排版函式（mtb_set_width），把自動換行的長文改寫成中文的行（見 mtb.rs）；
//! 也攔錨定貼圖函式（add_anchored_tile），翻譯逐字畫成貼圖的分頁標籤（見 hook_add_anchored_tile）。

use std::collections::HashSet;
use std::ffi::CStr;
use std::fs::File;
use std::io::Write;
use std::sync::atomic::{AtomicBool, AtomicPtr, AtomicU64, AtomicUsize, Ordering};
use std::sync::{Arc, OnceLock};

use parking_lot::Mutex;
use retour::RawDetour;

use crate::cp437;
use crate::dict::{Dict, Entry};
use crate::mtb::Plan;

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
    /// screentexpos_anchored：跨好幾格的貼圖（錨定貼圖）每格的貼圖編號
    pub anchored: usize,
    /// screen_color_r／g／b：目前的 RGB 前景色（段落換色時改）
    pub color: [usize; 3],
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
/// 目前的 DFHack 焦點字串（多個以換行分隔），Lua 每幀更新；依畫面套用的譯文看它（data/screens）
pub static FOCUS: Mutex<String> = Mutex::new(String::new());
/// 遊戲字型表 init.font.small_font_texpos、large_font_texpos 的位址（各 256 個 texpos，依 CP437 字元排）
pub static FONTS: OnceLock<[usize; 2]> = OnceLock::new();

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
type SetWidthFn = unsafe extern "C" fn(*mut u8, i32);
/// (gps, 貼圖, 錨點 x, 錨點 y, 寬幾格, 高幾格, 用畫面顏色上色)
type AnchoredFn = unsafe extern "C" fn(*mut u8, i32, i32, i32, i32, i32, u8);
/// (切好的行要接在後面的 vector<string*>, 原文, 寬幾格)
type WrapFn = unsafe extern "C" fn(*mut u8, *const StdString, i32);
/// (color_text_boxst：開頭是 vector<color_text_linest*>, 含換色標記的原文)
type CtbAddFn = unsafe extern "C" fn(*mut u8, *const StdString);

pub const HOOKS: [&str; 8] =
    ["addst", "addst_flag", "top_addst", "addcoloredst", "mtb_set_width", "add_anchored_tile", "wrap_text", "ctb_add_string"];
static TRAMPOLINES: [AtomicUsize; 8] = [const { AtomicUsize::new(0) }; 8];
/// wrap_text 的位址：enable 時從呼叫它的程式碼找出遊戲啟動時就切好的提示（見 lines.rs）
pub static WRAP_TEXT: OnceLock<usize> = OnceLock::new();

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
        3 => hook_addcoloredst as *const (),
        4 => hook_mtb_set_width as *const (),
        5 => hook_add_anchored_tile as *const (),
        6 => hook_wrap_text as *const (),
        _ => hook_ctb_add_string as *const (),
    };
    let detour = unsafe { RawDetour::new(target as *const (), hook) }.map_err(|e| e.to_string())?;
    TRAMPOLINES[index].store(detour.trampoline() as *const () as usize, Ordering::Release);
    DETOURS.lock().0.push(detour);
    if name == "wrap_text" {
        let _ = WRAP_TEXT.set(target);
    }
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

unsafe fn write<T>(gps: *mut u8, offset: usize, v: T) {
    unsafe { *(gps.add(offset) as *mut T) = v }
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

/// 字典裡的譯文：先找目前畫面專用的（data/screens），再找整句字典，最後套句型規則。
enum Found<'a> {
    Fixed(&'a Entry),
    Rule(Arc<Entry>),
}

impl std::ops::Deref for Found<'_> {
    type Target = Entry;
    fn deref(&self) -> &Entry {
        match self {
            Found::Fixed(e) => e,
            Found::Rule(e) => e,
        }
    }
}

fn lookup<'a>(dict: &'a Dict, key: &[u8], y: i32) -> Option<Found<'a>> {
    let screen = if dict.screens.is_empty() { None } else { dict.screen_entry(key, &FOCUS.lock(), y) };
    if let Some(entry) = screen.or_else(|| dict.entries.get(key)) {
        return Some(Found::Fixed(entry));
    }
    dict.rule_entry(key).map(Found::Rule)
}

/// 共用流程：查字典（整句、再句型規則），命中就讓 `write` 寫替身字串，再貼字圖。回傳 false 表示要照原樣呼叫原函式。
unsafe fn translate_call(gps: *mut u8, kind: &str, key: &[u8], space: i32, layer: fn(&Layout) -> Layer,
                         write: impl FnOnce(&[u8])) -> bool {
    flush_label();
    let dict = DICT.load(Ordering::Acquire);
    let Some(layout) = LAYOUT.get() else { return false };
    if dict.is_null() || !ENABLED.load(Ordering::Relaxed) {
        return false;
    }
    let dict = unsafe { &*dict };
    let (x0, y): (i32, i32) = unsafe { (read(gps, layout.screenx), read(gps, layout.screeny)) };
    let Some(entry) = lookup(dict, key, y) else {
        note_miss(kind, key);
        return false;
    };
    HITS.fetch_add(1, Ordering::Relaxed);
    let (cells, glyphs) = entry.fit(space);
    unsafe {
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

/// 段落單字與自動換行的行（見 mtb.rs、lines.rs）：照畫英文、不畫，或在載體的座標上畫幾段中文。回傳 false 表示都不是。
/// `draw` 用遊戲原本的寫字函式畫一個字串（載體本身，或中文段的替身字串）。
unsafe fn paragraph_word(gps: *mut u8, s: *const StdString, layer: fn(&Layout) -> Layer, draw: impl Fn(&StdString)) -> bool {
    let plan = unsafe { crate::mtb::word_plan(s as usize).or_else(|| crate::lines::line_plan(s as usize)) };
    let Some(plan) = plan else { return false };
    flush_label();
    let dict = DICT.load(Ordering::Acquire);
    let pieces = match plan {
        Plan::Plain => {
            unsafe { draw(&*s) };
            return true;
        }
        Plan::Missing(text) => {
            note_miss("wrap", &text);
            unsafe { draw(&*s) };
            return true;
        }
        Plan::Skip => return true,
        Plan::Draw(pieces) => pieces,
    };
    let Some(layout) = LAYOUT.get().filter(|_| !dict.is_null()) else { return true };
    let dict = unsafe { &*dict };
    HITS.fetch_add(1, Ordering::Relaxed);
    unsafe {
        let (x, y): (i32, i32) = (read(gps, layout.screenx), read(gps, layout.screeny));
        let color: [u8; 3] = layout.color.map(|off| read(gps, off));
        for p in pieces.iter() {
            write(gps, layout.screenx, x + p.dx);
            write(gps, layout.screeny, y + p.dy);
            if let Some(c) = p.color {
                for (off, v) in layout.color.iter().zip(c) {
                    write(gps, *off, v);
                }
            }
            draw(&StdString::borrowed(p.entry.shadow()));
            paint(gps, &layer(layout), x + p.dx, y + p.dy, &p.entry.glyphs, &dict.texpos);
        }
        for (off, c) in layout.color.iter().zip(color) {
            write(gps, *off, c);
        }
        write(gps, layout.screenx, x);
        write(gps, layout.screeny, y);
    }
    true
}

unsafe extern "C" fn hook_addst(gps: *mut u8, s: *const StdString, just: i32, space: i32) {
    let orig: AddstFn = unsafe { std::mem::transmute(TRAMPOLINES[0].load(Ordering::Acquire)) };
    let key = unsafe { (*s).bytes() };
    let done = unsafe {
        paragraph_word(gps, s, main_layer, |st| orig(gps, st, just, space))
            || translate_call(gps, "addst", key, space, main_layer, |b| orig(gps, &StdString::borrowed(b), just, space))
    };
    if !done {
        unsafe { orig(gps, s, just, space) }
    }
}

unsafe extern "C" fn hook_addst_flag(gps: *mut u8, s: *const StdString, just: i32, space: i32, sflag: u32) {
    let orig: AddstFlagFn = unsafe { std::mem::transmute(TRAMPOLINES[1].load(Ordering::Acquire)) };
    let key = unsafe { (*s).bytes() };
    let done = unsafe {
        paragraph_word(gps, s, main_layer, |st| orig(gps, st, just, space, sflag))
            || translate_call(gps, "addst_flag", key, space, main_layer, |b| orig(gps, &StdString::borrowed(b), just, space, sflag))
    };
    if !done {
        unsafe { orig(gps, s, just, space, sflag) }
    }
}

unsafe extern "C" fn hook_top_addst(gps: *mut u8, s: *const StdString, just: i32, space: i32) {
    let orig: AddstFn = unsafe { std::mem::transmute(TRAMPOLINES[2].load(Ordering::Acquire)) };
    let key = unsafe { (*s).bytes() };
    let done = unsafe {
        paragraph_word(gps, s, top_layer, |st| orig(gps, st, just, space))
            || translate_call(gps, "top_addst", key, 0, top_layer, |b| orig(gps, &StdString::borrowed(b), just, space))
    };
    if !done {
        unsafe { orig(gps, s, just, space) }
    }
}

unsafe extern "C" fn hook_addcoloredst(gps: *mut u8, s: *const u8, colors: *const u8) {
    let orig: ColoredFn = unsafe { std::mem::transmute(TRAMPOLINES[3].load(Ordering::Acquire)) };
    if unsafe { colored_line(gps, s, |t, c| orig(gps, t, c)) } {
        return;
    }
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

/// 彩色文字框的一行（見 ctb.rs）：照畫英文、不畫，或在這行的座標上畫幾段中文。回傳 false 表示不是框裡的行。
/// `draw` 用遊戲原本的 addcoloredst 畫（文字, 每字顏色）。
unsafe fn colored_line(gps: *mut u8, s: *const u8, draw: impl Fn(*const u8, *const u8)) -> bool {
    let Some(plan) = (unsafe { crate::ctb::line_plan(s as usize) }) else { return false };
    flush_label();
    let pieces = match plan {
        crate::ctb::Plan::English => {
            let color = crate::ctb::CTB_LAYOUT.get().map_or(0, |l| l.color);
            draw(s, unsafe { s.add(color) });
            return true;
        }
        crate::ctb::Plan::Skip => return true,
        crate::ctb::Plan::Draw(pieces) => pieces,
    };
    let dict = DICT.load(Ordering::Acquire);
    let Some(layout) = LAYOUT.get().filter(|_| !dict.is_null()) else { return true };
    let dict = unsafe { &*dict };
    HITS.fetch_add(1, Ordering::Relaxed);
    unsafe {
        let (x, y): (i32, i32) = (read(gps, layout.screenx), read(gps, layout.screeny));
        for p in pieces.iter() {
            write(gps, layout.screenx, x + p.dx);
            write(gps, layout.screeny, y + p.dy);
            let shadow = p.entry.shadow();
            let text: Vec<u8> = shadow.iter().copied().chain([0]).collect();
            let cols: Vec<u8> = std::iter::repeat_n(p.color, shadow.len()).chain([0]).collect();
            draw(text.as_ptr(), cols.as_ptr());
            paint(gps, &layout.main, x + p.dx, y + p.dy, &p.entry.glyphs, &dict.texpos);
        }
        write(gps, layout.screenx, x);
        write(gps, layout.screeny, y);
    }
    true
}

/// 段落排版：原函式依寬度排完英文單字後，整段有譯文就訂好每個單字的畫字計畫（見 mtb.rs）。
unsafe extern "C" fn hook_mtb_set_width(mtb: *mut u8, width: i32) {
    let orig: SetWidthFn = unsafe { std::mem::transmute(TRAMPOLINES[4].load(Ordering::Acquire)) };
    unsafe { orig(mtb, width) };
    let dict = DICT.load(Ordering::Acquire);
    if dict.is_null() || !ENABLED.load(Ordering::Relaxed) {
        return;
    }
    unsafe { crate::mtb::rewrite(mtb as usize, &*dict, |key| note_miss("mtb", key)) };
}

/// 自動換行：原函式把一句切成好幾行、接在 out 後面；替這次切出來的行訂畫字計畫（見 lines.rs）。
unsafe extern "C" fn hook_wrap_text(out: *mut u8, text: *const StdString, width: i32) {
    let orig: WrapFn = unsafe { std::mem::transmute(TRAMPOLINES[6].load(Ordering::Acquire)) };
    let before = unsafe { crate::lines::vec_len(out as usize) };
    unsafe { orig(out, text, width) };
    let dict = DICT.load(Ordering::Acquire);
    if dict.is_null() || !ENABLED.load(Ordering::Relaxed) {
        return;
    }
    unsafe {
        let lines = crate::lines::vec_items(out as usize);
        crate::lines::plan(&lines[before.min(lines.len())..], (*text).bytes(), width, &*dict, false);
    }
}

/// 彩色文字框：原函式把含換色標記的一段切成好幾行、接在框的行清單後面；整段有譯文就替這幾行訂畫字計畫（見 ctb.rs）。
unsafe extern "C" fn hook_ctb_add_string(boxp: *mut u8, raw: *const StdString) {
    let orig: CtbAddFn = unsafe { std::mem::transmute(TRAMPOLINES[7].load(Ordering::Acquire)) };
    let before = unsafe { crate::lines::vec_len(boxp as usize) };
    unsafe { orig(boxp, raw) };
    let dict = DICT.load(Ordering::Acquire);
    let Some(layout) = crate::ctb::CTB_LAYOUT.get() else { return };
    if dict.is_null() || !ENABLED.load(Ordering::Relaxed) {
        return;
    }
    unsafe {
        let lines = crate::lines::vec_items(boxp as usize);
        let width: i32 = read(boxp, layout.width);
        for sentence in crate::ctb::plan(&lines[before.min(lines.len())..], (*raw).bytes(), width, &*dict) {
            note_miss("ctb", &sentence);
        }
    }
}

// ---- 逐字畫成貼圖的分頁標籤 ----
//
// 出發準備畫面的分頁標籤（Dwarves、Items、Crucial、Labor…）在開圖形模式時不經寫字函式：遊戲把每個字
// 當成跨兩列的錨定貼圖（字型表裡那個字的貼圖）逐字畫。這裡把同一列、緊接著畫的字收成一個標籤，
// 等下一個不接續的畫圖呼叫、或開始寫別的字串時才查字典；有譯文就把那幾格的貼圖換成中文字圖（置中），
// 多出來的格換成空白。都在遊戲畫面的同一幀裡改，渲染器看到的就是中文。

struct Label {
    gps: usize,
    x0: i32,
    y: i32,
    /// 這個標籤用的字型表位址
    font: usize,
    text: Vec<u8>,
}

static LABEL: Mutex<Option<Label>> = Mutex::new(None);
static LABEL_PENDING: AtomicBool = AtomicBool::new(false);

unsafe fn font_tex(font: usize, ch: u8) -> i32 {
    unsafe { *(font as *const i32).add(ch as usize) }
}

/// 這個貼圖是不是字型表裡的字：(字型表位址, 字元)。字型表是連續的貼圖編號，先用頭一個算出位置再核對。
unsafe fn font_char(texpos: i32) -> Option<(usize, u8)> {
    FONTS.get()?.iter().find_map(|&font| unsafe {
        let d = texpos.wrapping_sub(font_tex(font, 0));
        ((0..256).contains(&d) && font_tex(font, d as u8) == texpos).then_some((font, d as u8))
    })
}

unsafe extern "C" fn hook_add_anchored_tile(gps: *mut u8, texpos: i32, ax: i32, ay: i32, w: i32, h: i32, color: u8) {
    let orig: AnchoredFn = unsafe { std::mem::transmute(TRAMPOLINES[5].load(Ordering::Acquire)) };
    unsafe { orig(gps, texpos, ax, ay, w, h, color) };
    let Some(layout) = LAYOUT.get() else { return };
    // 標籤的字一格寬、兩列高；其他錨定貼圖（人像、建築）不是字
    let ch = if w == 1 && h == 2 && ENABLED.load(Ordering::Relaxed) { unsafe { font_char(texpos) } } else { None };
    let Some((font, ch)) = ch else {
        flush_label();
        return;
    };
    let (x, y): (i32, i32) = unsafe { (read(gps, layout.screenx), read(gps, layout.screeny)) };
    let mut label = LABEL.lock();
    match label.as_mut() {
        Some(l) if l.y == y && l.font == font && l.x0 + l.text.len() as i32 == x => l.text.push(ch),
        _ => {
            let done = label.replace(Label { gps: gps as usize, x0: x, y, font, text: vec![ch] });
            LABEL_PENDING.store(true, Ordering::Release);
            drop(label);
            if let Some(done) = done {
                unsafe { finish_label(done) };
            }
        }
    }
}

fn flush_label() {
    if !LABEL_PENDING.swap(false, Ordering::AcqRel) {
        return;
    }
    if let Some(label) = LABEL.lock().take() {
        unsafe { finish_label(label) };
    }
}

unsafe fn finish_label(label: Label) {
    let dict = DICT.load(Ordering::Acquire);
    let Some(layout) = LAYOUT.get() else { return };
    if dict.is_null() {
        return;
    }
    let dict = unsafe { &*dict };
    let Some(entry) = lookup(dict, &label.text, label.y) else {
        note_miss("add_anchored_tile", &label.text);
        return;
    };
    let Some(tiles) = label_tiles(label.text.len(), &entry, &dict.texpos, |c| unsafe { font_tex(label.font, c) }) else {
        return; // 放不下，照畫英文
    };
    HITS.fetch_add(1, Ordering::Relaxed);
    unsafe {
        let gps = label.gps as *const u8;
        let (dimx, dimy): (i32, i32) = (read(gps, layout.dimx), read(gps, layout.dimy));
        let anchored: *mut i32 = read(gps, layout.anchored);
        for (k, (&english, &tex)) in label.text.iter().zip(&tiles).enumerate() {
            let x = label.x0 + k as i32;
            // 只改原函式真的畫了這個字的格（裁切掉的格不動）；錨點那格和下面那格都存同一個貼圖
            for y in [label.y, label.y + 1] {
                if (0..dimx).contains(&x) && (0..dimy).contains(&y) {
                    let cell = anchored.add((x * dimy + y) as usize);
                    if *cell == font_tex(label.font, english) {
                        *cell = tex;
                    }
                }
            }
        }
    }
}

/// 標籤每一格換成的貼圖：譯文在原文寬度裡置中，中文字放字圖的左右半格，其他字用字型表的字，
/// 多出來的格放空白。譯文比原文寬就回傳 None（照畫英文）。
fn label_tiles(n: usize, entry: &Entry, glyph_tex: &[i32], font: impl Fn(u8) -> i32) -> Option<Vec<i32>> {
    let shadow = entry.shadow();
    let pad = n.checked_sub(shadow.len())? / 2;
    let tile = |k: usize| match k.checked_sub(pad).filter(|&r| r < shadow.len()) {
        Some(r) => match entry.glyphs.iter().find(|&&(off, _)| r == off || r == off + 1) {
            Some(&(off, g)) => glyph_tex[g as usize * 2 + (r - off)],
            None => font(shadow[r]),
        },
        None => font(b' '),
    };
    Some((0..n).map(tile).collect())
}

#[cfg(test)]
mod tests {
    use super::label_tiles;
    use crate::dict::Dict;

    #[test]
    fn label_is_centered_in_the_english_width() {
        let mut d = Dict::default();
        assert!(d.set_glyphs("關鍵", vec![10, 11, 20, 21]));
        d.load_tsv("Crucial\t關鍵\nC1\tA關\n");
        let font = |c: u8| 500 + c as i32;
        let sp = font(b' ');
        // Crucial 7 格、關鍵 4 格：左邊空 1 格，右邊空 2 格
        assert_eq!(label_tiles(7, &d.entries[&b"Crucial"[..]], &d.texpos, font), Some(vec![sp, 10, 11, 20, 21, sp, sp]));
        // 英數照字型表；放不下就不換
        assert_eq!(label_tiles(3, &d.entries[&b"C1"[..]], &d.texpos, font), Some(vec![font(b'A'), 10, 11]));
        assert_eq!(label_tiles(2, &d.entries[&b"C1"[..]], &d.texpos, font), None);
    }
}
