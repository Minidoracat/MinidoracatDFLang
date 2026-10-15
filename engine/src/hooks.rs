//! 攔遊戲的寫字函式：字典有這句就改寫替身字串，原函式寫完後把佔位格換成字圖。
//!
//! 版面、裁切、顏色都交給遊戲原本的函式處理；這裡只多做一件事：在佔位格的
//! screentexpos 填字圖、在 screentexpos_flag 加 ADDCOLOR，讓渲染器用該格前景色為白色字圖上色。
//! 另外攔段落排版函式（mtb_set_width），把自動換行的長文改寫成中文的行（見 mtb.rs）；自動換行（wrap_lines）同理（見 lines.rs）；
//! 也攔錨定貼圖函式（add_anchored_tile），翻譯逐字畫成貼圖的分頁標籤（見 hook_add_anchored_tile）；
//! 以及 std::string::resize，記下遊戲截成「開頭...」之前的原文（見 lookup）。

use std::cell::RefCell;
use std::collections::{HashMap, HashSet, VecDeque};
use std::ffi::CStr;
use std::fs::File;
use std::io::Write;
use std::sync::atomic::{AtomicBool, AtomicPtr, AtomicU64, AtomicUsize, Ordering};
use std::sync::{Arc, LazyLock, OnceLock};

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
/// (切好的行要接在後面的 vector<string*>, 要切的幾段字串（接成一句）vector<string*>, 寬幾格)
type WrapFn = unsafe extern "C" fn(*mut u8, *const u8, i32);
/// (color_text_boxst：開頭是 vector<color_text_linest*>, 含換色標記的原文)
type CtbAddFn = unsafe extern "C" fn(*mut u8, *const StdString);
/// (language_name, 輸出的英文名, 每段字首大寫, 只要第一段)
type NameFn = unsafe extern "C" fn(*const u8, *mut StdString, u8, u8);
/// (std::string, 新長度, 變長時的填充字元)
type ResizeFn = unsafe extern "C" fn(*mut StdString, usize, u8);
/// (viewscreen)
type ScreenFn = unsafe extern "C" fn(*mut u8);
/// (冒險模式介面, 框左緣, 框右緣)
type LogFn = unsafe extern "C" fn(*mut u8, i32, i32);
/// (交談畫面, ?, 框左緣, 框右緣)
type TalkLogFn = unsafe extern "C" fn(*mut u8, u32, i32, i32);

pub const HOOKS: [&str; 14] = [
    "addst", "addst_flag", "top_addst", "addcoloredst", "mtb_set_width", "add_anchored_tile", "wrap_lines", "ctb_add_string",
    "translate_name", "string_resize", "adv_appearance", "adv_personality", "adv_log", "adv_talk_log",
];
static TRAMPOLINES: [AtomicUsize; 14] = [const { AtomicUsize::new(0) }; 14];
/// wrap_text 的位址（只登記、不攔；它切行時呼叫 wrap_lines）：enable 時從呼叫它的程式碼找出遊戲啟動時就切好的提示（見 lines.rs）
pub static WRAP_TEXT: OnceLock<usize> = OnceLock::new();

struct Detours(Vec<RawDetour>);
// RawDetour 只在持有鎖時由 Lua 指令啟停；遊戲執行緒只讀 TRAMPOLINES。
unsafe impl Send for Detours {}
static DETOURS: Mutex<Detours> = Mutex::new(Detours(Vec::new()));

pub fn attach(name: &str, target: usize) -> Result<(), String> {
    if name == "wrap_text" {
        let _ = WRAP_TEXT.set(target);
        return Ok(());
    }
    let index = HOOKS.iter().position(|h| *h == name).ok_or_else(|| format!("unknown hook {name}"))?;
    let hook: *const () = match index {
        0 => hook_addst as *const (),
        1 => hook_addst_flag as *const (),
        2 => hook_top_addst as *const (),
        3 => hook_addcoloredst as *const (),
        4 => hook_mtb_set_width as *const (),
        5 => hook_add_anchored_tile as *const (),
        6 => hook_wrap_lines as *const (),
        7 => hook_ctb_add_string as *const (),
        8 => hook_translate_name as *const (),
        9 => hook_string_resize as *const (),
        10 => hook_adv_appearance as *const (),
        11 => hook_adv_personality as *const (),
        12 => hook_adv_log as *const (),
        _ => hook_adv_talk_log as *const (),
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

unsafe fn write<T>(gps: *mut u8, offset: usize, v: T) {
    unsafe { *(gps.add(offset) as *mut T) = v }
}

/// 把 (x0 + 格位, y) 上的佔位格換成字圖。條件與遊戲 addchar 相同：在裁切框內（`clip`）、不超出畫面緩衝區。
/// DFHack 的 paintString 不看遊戲的裁切框（裁切由它的 Lua Painter 先做），貼它的字時 `clip` 給 false。
unsafe fn paint(gps: *mut u8, layer: &Layer, x0: i32, y: i32, glyphs: &[(usize, u32)], texpos: &[i32], clip: bool) {
    let Some(layout) = LAYOUT.get() else { return };
    unsafe {
        let dimx: i32 = read(gps, layout.dimx);
        let dimy: i32 = read(gps, layout.dimy);
        let clip: [i32; 4] = if clip {
            [read(gps, layout.clipx), read(gps, layout.clipx + 4), read(gps, layout.clipy), read(gps, layout.clipy + 4)]
        } else {
            [0, dimx - 1, 0, dimy - 1]
        };
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
    // 遊戲截短的字串（`Start new conversation with the Human baro...`）：翻截短前的原文再截短
    if let Some(e) = cut_original(key).and_then(|full| lookup(dict, &full, y)).and_then(|f| dict.cut_entry(key, &f.text)) {
        return Some(Found::Rule(e));
    }
    dict.rule_entry(key).map(Found::Rule)
}

// ---- 截短前的原文 ----

thread_local! {
    /// 這個執行緒最近被縮短的字串原文（新的在後）。遊戲在同一個執行緒縮短後馬上接「...」畫出來，留幾筆就夠；
    /// 每個執行緒各記各的，世界生成等其他執行緒的縮短不會擠掉或混進畫面這邊的紀錄。
    static CUTS: RefCell<VecDeque<Vec<u8>>> = const { RefCell::new(VecDeque::new()) };
}
const CUTS_KEEP: usize = 16;

/// 截短的字串（`開頭...`）的原文：這個執行緒最近縮短過、比它長、開頭相同的那筆（同一清單好幾筆開頭相同時，剛縮短的就是正要畫的）。
fn cut_original(key: &[u8]) -> Option<Vec<u8>> {
    let head = key.strip_suffix(b"...")?;
    CUTS.with_borrow(|cuts| cuts.iter().rev().find(|f| f.len() > key.len() && f.starts_with(head)).cloned())
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
    if !dict.verbatim_screens.is_empty() && dict.keeps_word(key, &FOCUS.lock(), false) {
        return false;
    }
    let Some(entry) = lookup(dict, key, y) else {
        note_miss(kind, key);
        return false;
    };
    HITS.fetch_add(1, Ordering::Relaxed);
    let (cells, glyphs) = entry.fit(space);
    unsafe {
        write(&entry.shadow()[..cells]);
        paint(gps, &layer(layout), x0, y, &entry.glyphs[..glyphs], &dict.texpos, true);
    }
    true
}

// ---- DFHack 視窗的字（Lua 包住 dfhack.screen.paintString，見 lua/loader.lua） ----
//
// DFHack 的介面（overlay、gui/ 腳本）不經遊戲的畫字函式，直接寫畫面緩衝區。Lua 依序呼叫：dfhack_translate 查字典、
// 原本的 paintString 畫影子字串（格數、顏色、裁切、點擊範圍都照英文），dfhack_stamp 貼字圖。都在主執行緒。

/// dfhack_translate 查到、等 dfhack_stamp 貼上的字圖 (格位, 字圖編號)
static DH_PENDING: Mutex<Vec<(usize, u32)>> = Mutex::new(Vec::new());

/// DFHack 自動換行的段落（string:wrap 切的，見 dfhack_wrap）：英文行 → 同一列要畫的中文（None：中文行比英文少，這一行畫空白）。
/// ponytail: 以英文行的文字當鍵、全部畫面共用：不同段落（或同一段的兩行）剛好有同一行英文時用最後登記的，別處單獨畫出同樣英文的字串
/// 也會換成這裡的中文或空白；表滿了整批清掉。真的撞到再改成依段落（WrappedLabel）登記。
static DH_LINES: LazyLock<Mutex<HashMap<Vec<u8>, Option<Arc<Entry>>>>> = LazyLock::new(|| Mutex::new(HashMap::new()));
const DH_LINES_LIMIT: usize = 20_000;

/// DFHack 要畫的一段字：有譯文就回傳影子字串（補到原長，頭尾空白照原文，視窗標題前後各有兩個空白），字圖留給 dfhack_stamp；
/// 沒有譯文、或譯文比原文寬時回傳 None，照畫英文。段落的一行（dfhack_wrap 登記過的）畫同一列的中文。
pub fn dfhack_translate(text: &[u8], y: i32) -> Option<Vec<u8>> {
    let dict = DICT.load(Ordering::Acquire);
    if dict.is_null() || !ENABLED.load(Ordering::Relaxed) {
        return None;
    }
    if let Some(row) = DH_LINES.lock().get(text).cloned() {
        let shadow = row.as_ref().map_or(&[][..], |e| e.shadow());
        let mut out = vec![b' '; text.len().max(shadow.len())];
        out[..shadow.len()].copy_from_slice(shadow);
        *DH_PENDING.lock() = row.map_or_else(Vec::new, |e| e.glyphs.clone());
        return Some(out);
    }
    let dict = unsafe { &*dict };
    let lead = text.iter().take_while(|&&c| c == b' ').count();
    let key = text[lead..].trim_ascii_end();
    if !key.iter().any(u8::is_ascii_alphabetic) || (!dict.verbatim_screens.is_empty() && dict.keeps_word(key, &FOCUS.lock(), true)) {
        return None;
    }
    let Some(entry) = lookup(dict, key, y) else {
        note_miss("dfhack", key);
        return None;
    };
    let shadow = entry.shadow();
    if lead + shadow.len() > text.len() {
        return None;
    }
    HITS.fetch_add(1, Ordering::Relaxed);
    let mut out = vec![b' '; text.len()];
    out[lead..lead + shadow.len()].copy_from_slice(shadow);
    *DH_PENDING.lock() = entry.glyphs.iter().map(|&(offset, glyph)| (offset + lead, glyph)).collect();
    Some(out)
}

/// DFHack 的 string:wrap（WrappedLabel、確認對話框都用它）把整段英文 text（`\n` 分段）依 width 格切成 lines，之後一行行畫：整段有譯文時依中文規則以同樣寬度重新換行，
/// 登記每一行英文要畫的中文（見 wrap_plan），回傳 true。缺一句就整段照英文，翻不出來的那一段記進未翻譯紀錄。
pub fn dfhack_wrap(text: &[u8], width: usize, lines: &[Vec<u8>]) -> bool {
    let dict = DICT.load(Ordering::Acquire);
    if dict.is_null() || !ENABLED.load(Ordering::Relaxed) || lines.is_empty() || width == 0 {
        return false;
    }
    let dict = unsafe { &*dict };
    let Some(plan) = wrap_plan(dict, text, width, lines) else { return false };
    let mut entries = Vec::with_capacity(plan.len());
    for (k, row) in plan {
        let entry = match row {
            Some(row) => {
                let Some(entry) = dict.text_entry(&row) else { return false };
                Some(Arc::new(entry))
            }
            None => None,
        };
        entries.push((k, entry));
    }
    let mut table = DH_LINES.lock();
    if table.len() + entries.len() > DH_LINES_LIMIT {
        table.clear();
    }
    for (k, entry) in entries {
        table.insert(lines[k].clone(), entry);
    }
    true
}

/// dfhack_wrap 的對照：(英文第幾行, 那一行要畫的中文；None 是畫空白)。string:wrap 一段一段切（原文的 `\n` 分段），
/// 這裡也每段各自翻、各自依 width 格換行，中文列依序放進同一段的英文行：前一段中文比較短時，後一段照樣從自己的第一行畫起，
/// 段落間的空行對得上。英文的空行（Painter 不畫空字串）不列。翻不出來的段落記進未翻譯紀錄；有段落翻不出來、某段中文比英文多列、
/// 或英文行對不回原文時回傳 None，整段照英文。
fn wrap_plan(dict: &Dict, text: &[u8], width: usize, lines: &[Vec<u8>]) -> Option<Vec<(usize, Option<String>)>> {
    let cells = |s: &str| s.chars().map(|c| if cp437::byte_of(c).is_some() { 1 } else { 2 }).sum::<usize>();
    let solid = |s: &[u8]| s.iter().filter(|c| !c.is_ascii_whitespace()).count();
    let mut plan = Vec::new();
    let mut next = 0;
    for para in text.split(|&c| c == b'\n') {
        // 這一段的英文行：string:wrap 只在空白處切行（太長的字切成幾截），各行扣掉空白的字數加起來就是這一段扣掉空白的字數；空段佔一行
        let (first, need) = (next, solid(para));
        let mut got = 0;
        while next == first || got < need {
            got += solid(lines.get(next)?);
            next += 1;
        }
        if got != need {
            return None;
        }
        let zh = if para.iter().any(u8::is_ascii_alphabetic) {
            match dict.translate_sentences(para) {
                Ok(pieces) => pieces.into_iter().map(|(t, _)| t).collect::<String>(),
                Err(_) => {
                    note_miss("dfhack", para);
                    return None;
                }
            }
        } else {
            cp437::decode(para)
        };
        // 換行後的每一段 (列內位移, 第幾列, 字)：同一列的段依位移接成一行。英文段開頭的縮排（string:wrap 只留在第一行）照樣加在第一列，
        // 所以換行寬度扣掉縮排
        let lead = para.iter().take_while(|&&c| c == b' ').count();
        let mut rows: Vec<String> = Vec::new();
        for (px, py, t, _) in crate::mtb::wrap(&zh, width.saturating_sub(lead).max(1), [0; 3], |c| c) {
            let (px, py) = (px.max(0) as usize, py.max(0) as usize);
            let px = if py == 0 { px + lead } else { px };
            if rows.len() <= py {
                rows.resize(py + 1, String::new());
            }
            let pad = px.saturating_sub(cells(&rows[py]));
            rows[py].extend(std::iter::repeat_n(' ', pad));
            rows[py].push_str(&t);
        }
        if rows.len() > next - first {
            return None;
        }
        for (k, line) in lines[first..next].iter().enumerate() {
            let row = rows.get(k).filter(|row| !row.trim().is_empty()).cloned();
            match (solid(line) == 0, row) {
                (true, Some(_)) => return None, // 有字的中文列落在不會畫的英文空行
                (true, None) => {}
                (false, row) => plan.push((first + k, row)),
            }
        }
    }
    lines[next..].iter().all(|line| solid(line) == 0).then_some(plan)
}

/// 把 dfhack_translate 查到的字圖貼在 (x, y)：DFHack 在頂層開著時（`top_in_use`）只寫頂層，字圖跟著貼頂層。
pub fn dfhack_stamp(gps: usize, x: i32, y: i32, top: bool) {
    let glyphs = std::mem::take(&mut *DH_PENDING.lock());
    let dict = DICT.load(Ordering::Acquire);
    let Some(layout) = LAYOUT.get() else { return };
    if dict.is_null() || gps == 0 || glyphs.is_empty() {
        return;
    }
    let layer = if top { layout.top } else { layout.main };
    unsafe { paint(gps as *mut u8, &layer, x, y, &glyphs, &(*dict).texpos, false) };
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
            paint(gps, &layer(layout), x + p.dx, y + p.dy, &p.entry.glyphs, &dict.texpos, true);
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
            paint(gps, &layout.main, x + p.dx, y + p.dy, &p.entry.glyphs, &dict.texpos, true);
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

/// 自動換行（wrap_text 內部真正切行的函式；宗教說明、冒險日誌等也直接呼叫它）：原函式把接成一句的幾段字串
/// （vector<string*>）依寬度切成好幾行、接在 out 後面；替這次切出來的行訂畫字計畫（見 lines.rs）。
unsafe extern "C" fn hook_wrap_lines(out: *mut u8, input: *const u8, width: i32) {
    let orig: WrapFn = unsafe { std::mem::transmute(TRAMPOLINES[6].load(Ordering::Acquire)) };
    let before = unsafe { crate::lines::vec_len(out as usize) };
    unsafe { orig(out, input, width) };
    let dict = DICT.load(Ordering::Acquire);
    if dict.is_null() || !ENABLED.load(Ordering::Relaxed) {
        return;
    }
    unsafe {
        let parts = crate::lines::vec_items(input as usize);
        let text: Vec<u8> = parts.iter().flat_map(|&s| crate::lines::str_bytes(s)).copied().collect();
        let lines = crate::lines::vec_items(out as usize);
        crate::lines::plan(&lines[before.min(lines.len())..], &text, width, &*dict, false);
    }
}

/// std::string::resize(新長度, 填充字元)：遊戲把放不下的字截成「開頭...」時先縮短字串（172 處都這樣寫），縮短前記下原文（見 lookup）。
unsafe extern "C" fn hook_string_resize(s: *mut StdString, n: usize, fill: u8) {
    let orig: ResizeFn = unsafe { std::mem::transmute(TRAMPOLINES[9].load(Ordering::Acquire)) };
    if ENABLED.load(Ordering::Relaxed) && n >= 4 && n < unsafe { (*s).size } {
        let full = unsafe { (*s).bytes() }.to_vec();
        CUTS.with_borrow_mut(|cuts| {
            if cuts.len() >= CUTS_KEEP {
                cuts.pop_front();
            }
            cuts.push_back(full);
        });
    }
    unsafe { orig(s, n, fill) };
}

/// 冒險者開局畫面的外觀段落：遊戲把 wrap_lines 的切行寫在自己函式裡（寬 77），切好的行放在冒險者資料的行清單，
/// 後面接一行空白和 wrap_text 切的段落。原函式跑完後替外觀那段登記畫字計畫（見 plan_adventurer）。
unsafe extern "C" fn hook_adv_appearance(vs: *mut u8) {
    let orig: ScreenFn = unsafe { std::mem::transmute(TRAMPOLINES[10].load(Ordering::Acquire)) };
    unsafe {
        orig(vs);
        plan_adventurer(vs, &[(0x1370, 77)]);
    }
}

/// 冒險者開局畫面的性格、價值觀段落：同上，三個行清單各一段（寬 78）。
unsafe extern "C" fn hook_adv_personality(vs: *mut u8) {
    let orig: ScreenFn = unsafe { std::mem::transmute(TRAMPOLINES[11].load(Ordering::Acquire)) };
    unsafe {
        orig(vs);
        plan_adventurer(vs, &[(0x1398, 78), (0x13b0, 78), (0x13c8, 78)]);
    }
}

/// 冒險模式上方的訊息框（公告與交談紀錄）：遊戲先把每則報告依框寬切成幾行，再一行行用 addst 畫，整句比對只看得到片段，
/// 兩句以上的一則也比不到。畫之前替每則報告的這幾行登記畫字計畫（見 plan_report_lines）。
unsafe extern "C" fn hook_adv_log(ui: *mut u8, left: i32, right: i32) {
    let orig: LogFn = unsafe { std::mem::transmute(TRAMPOLINES[12].load(Ordering::Acquire)) };
    unsafe {
        plan_report_lines(ui as usize + 0x43d0);
        orig(ui, left, right);
    }
}

/// 交談畫面上方的交談紀錄：同上，行清單在 +0xb0。
unsafe extern "C" fn hook_adv_talk_log(ui: *mut u8, a: u32, left: i32, right: i32) {
    let orig: TalkLogFn = unsafe { std::mem::transmute(TRAMPOLINES[13].load(Ordering::Acquire)) };
    unsafe {
        plan_report_lines(ui as usize + 0xb0);
        orig(ui, a, left, right);
    }
}

/// 報告切成的行清單（vector<行*>）：每行開頭是那一行的 std::string，+0x20 指回報告，報告 +8 是整則原文。
/// 同一則報告的幾行連在一起；只有一行的報告，行字串是空的、畫的是報告本身的全文。整則（「講者: 內容」）查字典，
/// 查不到就逐句查，中文以這幾行最長的一行為寬重新換行（見 lines.rs）。
/// ponytail: 位移照 53.16 的 FUN_1408020c0（訊息框 +0x43d0）、FUN_140867e10（交談畫面 +0xb0）寫死；改版時特徵碼對不上就不攔，
/// 只有這兩個框維持英文。
unsafe fn plan_report_lines(vec: usize) {
    let dict = DICT.load(Ordering::Acquire);
    if dict.is_null() || !ENABLED.load(Ordering::Relaxed) {
        return;
    }
    let dict = unsafe { &*dict };
    let items = unsafe { crate::lines::vec_items(vec) };
    let report_of = |item: usize| unsafe { (item as *const usize).byte_add(0x20).read() };
    let mut i = 0;
    while i < items.len() {
        let report = report_of(items[i]);
        let j = i + items[i..].iter().take_while(|&&item| report_of(item) == report).count();
        if report != 0 {
            let group = &items[i..j];
            let text = unsafe { crate::lines::str_bytes(report + 8) };
            let single;
            let lines: &[usize] = match group {
                [only] if unsafe { crate::lines::str_bytes(*only) }.is_empty() => {
                    single = report + 8;
                    std::slice::from_ref(&single)
                }
                _ => group,
            };
            if text.iter().any(u8::is_ascii_alphabetic) && !unsafe { crate::lines::planned(lines) } {
                let width = lines.iter().map(|&s| unsafe { crate::lines::str_bytes(s) }.len()).max().unwrap_or(0) as i32;
                unsafe { crate::lines::plan(lines, text, width, dict, false) };
            }
        }
        i = j;
    }
}

/// 開局畫面目前選的冒險者（viewscreen +0x330 的 vector、+0x348 的索引）的行清單 (位移, 寬)：每個清單第一個只有空白的行之前是一段。
/// ponytail: 位移照 53.16 的 FUN_140639440／FUN_14063a7d0 寫死；改版時特徵碼對不上就不攔，只有這幾段維持英文。
unsafe fn plan_adventurer(vs: *mut u8, lists: &[(usize, i32)]) {
    let dict = DICT.load(Ordering::Acquire);
    if dict.is_null() || !ENABLED.load(Ordering::Relaxed) {
        return;
    }
    unsafe {
        let advs = crate::lines::vec_items(vs as usize + 0x330);
        let Some(&adv) = usize::try_from(read::<i32>(vs, 0x348)).ok().and_then(|i| advs.get(i)) else { return };
        for &(offset, width) in lists {
            let lines = crate::lines::vec_items(adv + offset);
            let n = lines.iter().position(|&s| crate::lines::str_bytes(s) == b" ").unwrap_or(lines.len());
            crate::lines::plan(&lines[..n], &crate::lines::join(&lines[..n]), width, &*dict, false);
        }
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

/// 名字轉英文：原函式照跑、英文照用；登記這個英文名的中文（見 names.rs）。
unsafe extern "C" fn hook_translate_name(name: *const u8, out: *mut StdString, capitalize: u8, only_first: u8) {
    let orig: NameFn = unsafe { std::mem::transmute(TRAMPOLINES[8].load(Ordering::Acquire)) };
    unsafe { orig(name, out, capitalize, only_first) };
    if ENABLED.load(Ordering::Relaxed) {
        unsafe { crate::names::note(name as usize, (*out).bytes()) };
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
    use super::{label_tiles, wrap_plan};
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

    #[test]
    fn wrapped_paragraphs_line_up_by_segment() {
        let mut d = Dict::default();
        d.load_tsv("First line.\t第一行。\nThe second paragraph is long.\t第二段。\n");
        let lines = |ls: &[&str]| ls.iter().map(|l| l.as_bytes().to_vec()).collect::<Vec<_>>();
        let row = |s: &str| Some(s.to_owned());
        // 英文第一段切成兩行、中文只要一列：第二行畫空白，空行不列，下一段照樣從自己的第一行畫起（不往上擠到不會畫的空行）
        assert_eq!(wrap_plan(&d, b"The second paragraph is long.\n\nFirst line.", 20, &lines(&["The second", "paragraph is long.", "", "First line."])),
                   Some(vec![(0, row("第二段。")), (1, None), (3, row("第一行。"))]));
        assert_eq!(wrap_plan(&d, b"First line.\n\nThe second paragraph is long.", 20, &lines(&["First line.", "", "The second", "paragraph is long."])),
                   Some(vec![(0, row("第一行。")), (2, row("第二段。")), (3, None)]));
        assert_eq!(wrap_plan(&d, b"First line.\nThird line.", 20, &lines(&["First line.", "Third line."])), None, "有一段翻不出來就整段照英文");
        assert_eq!(wrap_plan(&d, b"First line.", 20, &lines(&["First", "line.", "extra"])), None, "英文行對不回原文就照英文");
        assert_eq!(wrap_plan(&d, b"  First line.", 20, &lines(&["  First line."])), Some(vec![(0, row("  第一行。"))]), "段首縮排照留");
    }
}
