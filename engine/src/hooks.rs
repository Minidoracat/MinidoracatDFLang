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
use std::io::{Read, Write};
use std::sync::atomic::{AtomicBool, AtomicPtr, AtomicU64, AtomicUsize, Ordering};
use std::sync::{Arc, LazyLock, OnceLock};

use parking_lot::Mutex;
use retour::RawDetour;

use crate::cp437;
use crate::dict::{Dict, Entry, Row};
use crate::mtb::Plan;

/// screentexpos_flag 位元（DFHack Screen.cpp）：ADDCOLOR 以該格前景色為貼圖上色；
/// TOP_OF_TEXT／BOTTOM_OF_TEXT 是「文字往下位移半格」的上下兩列，遊戲用半格字形分別畫上半與下半。
const FLAG_ADDCOLOR: u32 = 0x2;
const FLAG_TOP_OF_TEXT: u32 = 0x8;
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
    /// screentexpos_lower／screentexpos_top_lower：字底下的貼圖（按鈕、面板的九宮格底圖）
    pub lower: usize,
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
/// render_things()：非同步執行緒每幀把畫面寫進緩衝區（整句貼圖在這裡收一幀的句子，見 overlay.rs）
type RenderThingsFn = unsafe extern "C" fn();

pub const HOOKS: [&str; 15] = [
    "addst", "addst_flag", "top_addst", "addcoloredst", "mtb_set_width", "add_anchored_tile", "wrap_lines", "ctb_add_string",
    "translate_name", "string_resize", "adv_appearance", "adv_personality", "adv_log", "adv_talk_log", "render_things",
];
static TRAMPOLINES: [AtomicUsize; 15] = [const { AtomicUsize::new(0) }; 15];
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
        13 => hook_adv_talk_log as *const (),
        _ => hook_render_things as *const (),
    };
    let detour = unsafe { RawDetour::new(target as *const (), hook) }.map_err(|e| e.to_string())?;
    TRAMPOLINES[index].store(detour.trampoline() as *const () as usize, Ordering::Release);
    DETOURS.lock().0.push(detour);
    Ok(())
}

/// 沒有掛遊戲的 hook 也能開：本 MOD 讓給別的翻譯引擎時，只用 DFHack 視窗那條路（Lua 包住的 paintString）把標題畫面的切換說明畫成中文
pub fn set_enabled(on: bool) -> Result<(), String> {
    let detours = DETOURS.lock();
    for d in &detours.0 {
        unsafe { if on { d.enable() } else { d.disable() } }.map_err(|e| e.to_string())?;
    }
    ENABLED.store(on, Ordering::Release);
    if !on {
        // render_things 的 hook 停了，整句貼圖不會再換幀：清掉收到一半的字與最後一幀（見 overlay::record）
        crate::overlay::reset();
    }
    Ok(())
}

pub fn hook_count() -> usize {
    DETOURS.lock().0.len()
}

// ---- 未翻譯字串記錄（翻譯補漏用） ----

/// 紀錄檔最多幾筆（不重複的原文）、幾位元組，到了就不再寫（status 的 misses_full）。
/// 開發期八十多次實機測試合計才 2,854 筆不重複、215 KB；一般遊玩每小時約一百到一千筆，兩萬筆約幾十小時，
/// 再多大多是一直變的數字、名字。位元組上限防傳說頁的整段（一段可到 7 KB）把檔案撐大。
const MISS_LIMIT: usize = 20_000;
const MISS_BYTES: u64 = 4 << 20;

struct MissLog {
    /// 檔案裡已有的原文（CP437）：開檔時讀進來，跨次啟動不重寫
    seen: HashSet<Vec<u8>>,
    file: File,
    bytes: u64,
}

impl MissLog {
    /// 開檔（沒有就建），讀進已記過的原文。上次寫到一半、最後一行沒換行時先補換行。
    fn open(path: &str) -> std::io::Result<MissLog> {
        let mut file = File::options().create(true).read(true).append(true).open(path)?;
        let mut old = Vec::new();
        file.read_to_end(&mut old)?;
        if old.last().is_some_and(|&b| b != b'\n') {
            file.write_all(b"\n")?;
            old.push(b'\n');
        }
        let seen = String::from_utf8_lossy(&old)
            .lines()
            .filter_map(|line| cp437::encode(line.split_once('\t')?.1))
            .collect();
        Ok(MissLog { seen, file, bytes: old.len() as u64 })
    }

    fn full(&self) -> bool {
        self.seen.len() >= MISS_LIMIT || self.bytes >= MISS_BYTES
    }

    /// 沒記過的原文寫一行（一次寫入）；回傳之後還能不能再記（滿了是 false）
    fn note(&mut self, kind: &str, key: &[u8]) -> bool {
        if self.full() {
            return false;
        }
        // 先查再複製：同一個字串每幀都會來，只有第一次配置
        if !self.seen.contains(key) {
            let line = format!("{kind}\t{}\n", cp437::decode(key));
            if self.file.write_all(line.as_bytes()).is_ok() {
                self.bytes += line.len() as u64;
            }
            self.seen.insert(key.to_vec());
        }
        !self.full()
    }
}

static MISS_LOG: Mutex<Option<MissLog>> = Mutex::new(None);
/// 正在記而且還沒滿；note_miss 只看它，關著或滿了就不鎖 MISS_LOG
static MISS_LOG_ON: AtomicBool = AtomicBool::new(false);

/// 開始記到 path（nil 停止）。已有的紀錄照留，記過的原文不再寫。
pub fn log_misses(path: Option<&str>) -> std::io::Result<()> {
    let mut log = MISS_LOG.lock();
    *log = path.map(MissLog::open).transpose()?;
    MISS_LOG_ON.store(log.as_ref().is_some_and(|l| !l.full()), Ordering::Release);
    Ok(())
}

/// status 用：(正在記, 檔案裡有幾筆, 滿了沒)
pub fn miss_log_state() -> (bool, usize, bool) {
    MISS_LOG.lock().as_ref().map_or((false, 0, false), |l| (true, l.seen.len(), l.full()))
}

fn note_miss(kind: &str, key: &[u8]) {
    MISSES.fetch_add(1, Ordering::Relaxed);
    if !MISS_LOG_ON.load(Ordering::Relaxed) || !key.iter().any(u8::is_ascii_alphabetic) {
        return;
    }
    if MISS_LOG.lock().as_mut().is_some_and(|log| !log.note(kind, key)) {
        MISS_LOG_ON.store(false, Ordering::Release);
    }
}

/// status 用：DFHack 段落行的筆數
pub fn dh_lines_len() -> usize {
    DH_LINES.lock().len()
}

// ---- 貼字圖 ----

unsafe fn read<T: Copy>(gps: *const u8, offset: usize) -> T {
    unsafe { *(gps.add(offset) as *const T) }
}

unsafe fn write<T>(gps: *mut u8, offset: usize, v: T) {
    unsafe { *(gps.add(offset) as *mut T) = v }
}

/// 要貼的一段字：寫進畫面的替身字串（glyphs 的格位從它開頭算）、對原文的對齊（Entry::align）、
/// 結尾有幾格是補到原文寬度的空白（不是譯文本身的空白，見 overlay.rs 的 Chain），
/// 以及段落照字型實際字寬縮短時的 (縮排格數, 記號格數, 文字)（Entry::compact）。整句貼圖（overlay.rs）都會用到。
struct Paint<'a> {
    shadow: &'a [u8],
    glyphs: &'a [(usize, u32)],
    align: u8,
    pad: usize,
    compact: Option<(usize, usize, &'a str)>,
}

impl<'a> Paint<'a> {
    fn of(entry: &'a Entry) -> Paint<'a> {
        let shadow = entry.shadow();
        let compact = entry.compact.map(|lead| (lead, shadow.len() - lead, entry.text.as_str()));
        Paint { shadow, glyphs: &entry.glyphs, align: entry.align, pad: 0, compact }
    }
}

/// 把 (x0 + 格位, y) 上的佔位格換成字圖（段落照字寬縮短的換成透明記號）。條件與遊戲 addchar 相同：在裁切框內（`clip`）、不超出畫面緩衝區。
/// DFHack 的 paintString 不看遊戲的裁切框（裁切由它的 Lua Painter 先做），貼它的字時 `clip` 給 false，整句貼圖也記成 DFHack 的字。
unsafe fn paint(gps: *mut u8, layer: &Layer, x0: i32, y: i32, p: Paint, dict: &Dict, clip: bool) {
    let Some(layout) = LAYOUT.get() else { return };
    unsafe {
        let dimx: i32 = read(gps, layout.dimx);
        let dimy: i32 = read(gps, layout.dimy);
        let dfhack = !clip;
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
        // 能貼的格子：(格位, 那一格在畫面緩衝區的索引)
        let cell = |k: usize| -> Option<usize> {
            let x = x0 + k as i32;
            if x < 0 || x >= dimx || x < clip[0] || x > clip[1] {
                return None;
            }
            let index = (x * dimy + y) as usize;
            (screen.add(index * 8) < limit).then_some(index)
        };
        // 半格位移的文字（分頁標籤等）遊戲在上下兩列各寫一次，字圖也分上下兩半（見 atlas::render）
        let row_at = |index: usize| {
            let f = *flag.add(index);
            if f & FLAG_BOTTOM_OF_TEXT != 0 {
                Row::Bottom
            } else if f & FLAG_TOP_OF_TEXT != 0 {
                Row::Top
            } else {
                Row::Whole
            }
        };
        let top = layer.screen == layout.top.screen;
        if let Some((lead, cells, text)) = p.compact {
            let Some(marker) = dict.marker() else { return };
            let mut row = None;
            for k in lead..lead + cells {
                let Some(index) = cell(k) else { continue };
                row.get_or_insert(row_at(index));
                *tex.add(index) = marker;
            }
            if let Some(row) = row {
                let shadow = &p.shadow[lead..(lead + cells).min(p.shadow.len())];
                crate::overlay::record(gps, top, row, x0 + lead as i32, y, shadow, &[], b'l', 0, Some(text), dfhack);
            }
            return;
        }
        // 整句的列別；同一句混了不同列別就不交給整句貼圖
        let mut kind: Option<Option<Row>> = None;
        for &(offset, glyph) in p.glyphs {
            for half in 0..2 {
                let Some(index) = cell(offset + half) else { continue };
                let row = row_at(index);
                kind = match kind {
                    None => Some(Some(row)),
                    Some(Some(k)) if k == row => kind,
                    _ => Some(None),
                };
                let Some(t) = dict.tile(glyph, half, row) else { continue };
                *tex.add(index) = t;
                *flag.add(index) |= FLAG_ADDCOLOR;
            }
        }
        match kind {
            Some(Some(row)) => crate::overlay::record(gps, top, row, x0, y, p.shadow, p.glyphs, p.align, p.pad, None, dfhack),
            // 沒有字圖的字串（傳說頁去掉黏著標點、照英文畫的連結字，只有英數的譯文）：也交給整句貼圖，
            // 和旁邊的段落文字接成一行緊排，不然會照遊戲的點陣字或從格子讀回來、留下英文字寬的空隙
            None if p.glyphs.is_empty() => {
                if let Some(index) = cell(0) {
                    crate::overlay::record(gps, top, row_at(index), x0, y, p.shadow, &[], p.align, p.pad, None, dfhack);
                }
            }
            _ => {}
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
/// 開頭要有字母：單獨的「...」（搜尋框沒輸入時的提示）不是截短的字串，不然會配上隨便一筆剛縮短的名字（「M…」）
fn cut_original(key: &[u8]) -> Option<Vec<u8>> {
    let head = key.strip_suffix(b"...").filter(|h| h.iter().any(u8::is_ascii_alphabetic))?;
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
    let (dimx, dimy): (i32, i32) = unsafe { (read(gps, layout.dimx), read(gps, layout.dimy)) };
    ENGLISH.lock().note(x0, y, key, dimx, dimy);
    let (cells, glyphs) = entry.fit(space);
    let (shadow, glyphs) = (&entry.shadow()[..cells], &entry.glyphs[..glyphs]);
    let n = key.len();
    let lay = layer(layout);
    let lower = |x: i32| unsafe { lower_at(gps, layout, &lay, x, y) };
    let column = COLUMNS.lock().note(lower, x0, n as i32);
    let fits = cells == entry.shadow().len() && (space <= 0 || space as usize >= n);
    let align = if fits { placed_align(lower, x0, n as i32, dimx, column, entry.align) } else { entry.align };
    // 字串檔的對齊和遊戲實際的不同（靠左、置中、靠右互換）時，譯文在原文寬度裡重排
    let relayout = align != entry.align;
    unsafe {
        if relayout {
            // 譯文實際佔的格子（字圖格的替身是空白，要算進去；置中的譯文前後補了空白，見 Dict::entry）
            let start = glyphs.iter().map(|g| g.0).chain(shadow.iter().position(|&c| c != b' ')).min().unwrap_or(0);
            let end = glyphs.iter().map(|g| g.0 + 2).chain(shadow.iter().rposition(|&c| c != b' ').map(|p| p + 1)).max().unwrap_or(0);
            let body = &shadow[start..end.max(start)];
            let at = match align {
                b'c' => n.saturating_sub(body.len()) / 2,
                b'r' => n.saturating_sub(body.len()),
                _ => 0,
            };
            let mut out = vec![b' '; n.max(at + body.len())];
            out[at..at + body.len()].copy_from_slice(body);
            let moved: Vec<(usize, u32)> = glyphs.iter().map(|&(off, g)| (off - start + at, g)).collect();
            write(&out);
            let pad = out.len() - at - body.len();
            paint(gps, &lay, x0, y, Paint { shadow: &out, glyphs: &moved, align, pad, compact: None }, dict, true);
        } else {
            write(shadow);
            paint(gps, &lay, x0, y, Paint { shadow, glyphs, align: entry.align, pad: 0, compact: None }, dict, true);
        }
    }
    true
}

/// (x, y) 字底下（lower 層：按鈕、面板的九宮格底圖）的貼圖編號；畫面外是 0
unsafe fn lower_at(gps: *const u8, layout: &Layout, layer: &Layer, x: i32, y: i32) -> i32 {
    unsafe {
        let (dimx, dimy): (i32, i32) = (read(gps, layout.dimx), read(gps, layout.dimy));
        let lower: *const i32 = read(gps, layer.lower);
        if lower.is_null() || !(0..dimx).contains(&x) || !(0..dimy).contains(&y) {
            return 0;
        }
        *lower.add((x * dimy + y) as usize)
    }
}

/// 遊戲畫從 x0 起 n 格原文的對齊方式（listed 是字串檔寫的）。字畫在按鈕或視窗上（lower 是這一列字底下的貼圖編號，
/// 起點那格有貼圖）時：同一種按鈕、同一個起點畫過長短很不一樣的字（column，見 Columns）是靠左排的清單（字串檔寫靠右的
/// 點數欄也照靠左）；否則原文在同一張中段貼圖的範圍裡、或在整排連著的貼圖（視窗：說明視窗左上角的問號框也算進去）裡
/// 左右剩的格數差兩格以內是置中（遊戲置中時有時偏一格；原文太長蓋到右緣時右邊是負的），右邊只剩一格以內、左邊空很多的是靠右
/// （存檔清單的「Folder: region1」）。在按鈕上但不是這兩種的是靠左（字串檔寫置中也照靠左，靠右照寫的）。
/// 不在按鈕上時，原文在畫面正中間（容許差一格：有些畫面照少一兩欄的寬度置中；左右各留一些，不是整列）的算置中，其他照字串檔。
fn placed_align(lower: impl Fn(i32) -> i32, x0: i32, n: i32, dimx: i32, column: bool, listed: u8) -> u8 {
    let mid = lower(x0);
    if mid != 0 {
        let margins = |inside: &dyn Fn(i32) -> bool| {
            let left = (1..).take_while(|&d| inside(x0 - d)).count() as i32;
            (left, (0..).take_while(|&d| inside(x0 + d)).count() as i32 - n)
        };
        // 左右剩的格數差一格以內；兩旁都還很寬時差兩格也算（選擇遊戲模式的 Legends 左 15 右 17），兩旁窄時差兩格就是靠左
        let centered_in = |(left, right): (i32, i32)| {
            let gap = (left - right).abs();
            gap <= 1 || (gap <= 2 && left.min(right) >= 4)
        };
        let button = margins(&|x| lower(x) == mid);
        let centered = !column && (centered_in(button) || centered_in(margins(&|x| lower(x) != 0)));
        return match listed {
            _ if centered => b'c',
            _ if !column && button.1 <= 1 && button.0 >= 4 => b'r',
            b'c' => b'l',
            b'r' if column => b'l',
            _ => listed,
        };
    }
    if listed == b'l' && x0 > 0 && x0 + n < dimx && (2 * x0 + n - dimx).abs() <= 2 {
        return b'c';
    }
    listed
}

/// 按鈕上的字從哪一格開始：(按鈕中段的左緣, 右緣, 起點) → 第一次見到的原文長度、是否見過長短差兩格以上的字。
/// 靠左排的清單（工作分組、建造選單、冒險角色的按鈕、按鍵設定的連發選項、建立世界的選項）每一項都從按鈕左緣算起的同一格開始，
/// 選取中的按鈕貼圖不同也一樣；置中的按鈕字長短差兩格以上，起點就不會相同（只差一格的可能因為整數除剛好同一格，
/// 像「Fortress」與「Legends」）。右緣也比：清單的按鈕一樣寬，左緣剛好同一格的別種框（冒險角色的「Name」標籤和下面的分頁）
/// 不算同一欄。一直記著：換分頁時同一顆按鈕換字（冒險角色每一頁底下的「Accept …」），之前見過別的長度就知道是靠左
pub struct Columns(HashMap<(i32, i32, i32), (i32, bool)>);

pub static COLUMNS: LazyLock<Mutex<Columns>> = LazyLock::new(|| Mutex::new(Columns(HashMap::new())));

impl Columns {
    /// 記下這一筆（lower 是這一列字底下的貼圖編號；不在按鈕上的不記），回傳這個起點是不是靠左排的清單
    fn note(&mut self, lower: impl Fn(i32) -> i32, x0: i32, n: i32) -> bool {
        let mid = lower(x0);
        if mid == 0 {
            return false;
        }
        let left = x0 - (1..).take_while(|&d| lower(x0 - d) == mid).count() as i32;
        let key = (left, x0 + (0..).take_while(|&d| lower(x0 + d) == mid).count() as i32, x0);
        // ponytail: 換畫面、改解析度留下的舊位置不清，記滿了整個清掉重記
        if self.0.len() >= 4096 && !self.0.contains_key(&key) {
            self.0.clear();
        }
        let e = self.0.entry(key).or_insert((n, false));
        if (e.0 - n).abs() >= 2 {
            e.1 = true;
        }
        e.1
    }
}

// ---- DFHack 視窗的字（Lua 包住 dfhack.screen.paintString，見 lua/loader.lua） ----
//
// DFHack 的介面（overlay、gui/ 腳本）不經遊戲的畫字函式，直接寫畫面緩衝區。Lua 依序呼叫：dfhack_translate 查字典、
// 原本的 paintString 畫影子字串（格數、顏色、裁切、點擊範圍都照英文），dfhack_stamp 貼字圖。都在主執行緒。

/// dfhack_translate 查到、等 dfhack_stamp 貼上的譯文（影子字串、字圖格位都從 paintString 那段字的開頭算；pad 見 Paint）
struct Pending {
    shadow: Vec<u8>,
    glyphs: Vec<(usize, u32)>,
    align: u8,
    pad: usize,
    compact: Option<(usize, usize, String)>,
}

static DH_PENDING: Mutex<Option<Pending>> = Mutex::new(None);

/// DFHack 自動換行的段落（string:wrap 切的，見 dfhack_wrap）：英文行 → 同一列要畫的中文（None：中文行比英文少，這一行畫空白）。
/// ponytail: 以英文行的文字當鍵、全部畫面共用：不同段落（或同一段的兩行）剛好有同一行英文時用最後登記的，別處單獨畫出同樣英文的字串
/// 也會換成這裡的中文或空白；表滿了整批清掉。真的撞到再改成依段落（WrappedLabel）登記。
static DH_LINES: LazyLock<Mutex<HashMap<Vec<u8>, Option<Arc<Entry>>>>> = LazyLock::new(|| Mutex::new(HashMap::new()));
const DH_LINES_LIMIT: usize = 20_000;

/// DFHack 要畫的一段字：有譯文就回傳影子字串（補到原長，頭尾空白照原文，視窗標題前後各有兩個空白），字圖留給 dfhack_stamp；
/// 沒有譯文、或譯文比原文寬時回傳 None，照畫英文。段落的一行（dfhack_wrap 登記過的）畫同一列的中文。
/// `center`：DFHack 依英文長度置中的字（分頁標籤、視窗標題，見 lua/loader.lua）：譯文放在原文（不含頭尾空白）的正中間，整句貼圖也置中畫。
pub fn dfhack_translate(text: &[u8], y: i32, center: bool) -> Option<Vec<u8>> {
    let dict = DICT.load(Ordering::Acquire);
    if dict.is_null() || !ENABLED.load(Ordering::Relaxed) {
        return None;
    }
    if let Some(row) = DH_LINES.lock().get(text).cloned() {
        let shadow = row.as_ref().map_or(&[][..], |e| e.shadow());
        let mut out = vec![b' '; text.len().max(shadow.len())];
        out[..shadow.len()].copy_from_slice(shadow);
        let pad = out.len() - shadow.len();
        *DH_PENDING.lock() = row.map(|e| Pending {
            shadow: out.clone(),
            glyphs: e.glyphs.clone(),
            align: b'l',
            pad,
            compact: e.compact.map(|lead| (lead, e.shadow().len() - lead, e.text.clone())),
        });
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
    // DFHack 的字位置由它的元件算好，字串檔的置中、靠右（給遊戲自己的按鈕用的，前面已補好空白）不套用：
    // 取譯文實際佔的格子（字圖格的替身是空白，要算進去），靠左放在原文起點，或依 center 放在原文正中間
    let (shadow, glyphs) = (entry.shadow(), &entry.glyphs);
    let start = glyphs.iter().map(|g| g.0).chain(shadow.iter().position(|&c| c != b' ')).min().unwrap_or(0);
    let end = glyphs.iter().map(|g| g.0 + 2).chain(shadow.iter().rposition(|&c| c != b' ').map(|p| p + 1)).max().unwrap_or(0);
    let body = &shadow[start..end.max(start)];
    let at = if center { lead + key.len().saturating_sub(body.len()) / 2 } else { lead };
    if at + body.len() > text.len() {
        return None;
    }
    HITS.fetch_add(1, Ordering::Relaxed);
    let mut out = vec![b' '; text.len()];
    out[at..at + body.len()].copy_from_slice(body);
    let glyphs: Vec<(usize, u32)> = glyphs.iter().map(|&(offset, glyph)| (offset - start + at, glyph)).collect();
    let align = if center { b'c' } else { b'l' };
    // 譯文比原文短、空出來的格子才算補位；英文原本結尾的空白（欄位對齊、標籤與值之間）照算（見 Paint::pad）
    let pad = (lead + key.len()).saturating_sub(at + body.len());
    *DH_PENDING.lock() = Some(Pending { shadow: out.clone(), glyphs, align, pad, compact: None });
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
                let Some(entry) = dict.para_entry(&row) else { return false };
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
    // 一列已經佔幾格：整句貼圖開著時照字型的實際字寬（見 metrics::cells_of），和 para_entry 的替身字串一樣長
    let cells = crate::metrics::cells_of;
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
    let Some(p) = DH_PENDING.lock().take() else { return };
    let dict = DICT.load(Ordering::Acquire);
    let Some(layout) = LAYOUT.get() else { return };
    if dict.is_null() || gps == 0 || (p.glyphs.is_empty() && p.compact.is_none()) {
        return;
    }
    let layer = if top { layout.top } else { layout.main };
    let compact = p.compact.as_ref().map(|(lead, cells, text)| (*lead, *cells, text.as_str()));
    let paint_of = Paint { shadow: &p.shadow, glyphs: &p.glyphs, align: p.align, pad: p.pad, compact };
    unsafe { paint(gps as *mut u8, &layer, x, y, paint_of, &*dict, false) };
}

fn main_layer(l: &Layout) -> Layer {
    l.main
}

fn top_layer(l: &Layout) -> Layer {
    l.top
}

/// 遊戲照原樣畫的字（沒有譯文）：記下按鈕上的字從哪一格開始（見 Columns），也交給整句貼圖（overlay.rs），
/// 不用遊戲的點陣字，版面不動。(x0, y) 是畫之前的游標；space > 0 時遊戲最多畫 space 格；dfhack 是 DFHack 畫的字。
unsafe fn record_plain(gps: *mut u8, layer: fn(&Layout) -> Layer, x0: i32, y: i32, text: &[u8], space: i32, dfhack: bool) {
    let Some(layout) = LAYOUT.get() else { return };
    let layer = layer(layout);
    let n = if space > 0 { text.len().min(space as usize) } else { text.len() };
    unsafe { COLUMNS.lock().note(|x| lower_at(gps, layout, &layer, x, y), x0, n as i32) };
    if !crate::overlay::ON.load(Ordering::Relaxed) {
        return;
    }
    let row = unsafe { row_of(gps, layout, &layer, x0, y) };
    crate::overlay::record(gps, layer.screen == layout.top.screen, row, x0, y, &text[..n], &[], b'l', 0, None, dfhack);
}

/// (x, y) 那一格的列別（一般、半格位移的上列或下列）；格子不在畫面裡時當一般列
unsafe fn row_of(gps: *const u8, layout: &Layout, layer: &Layer, x: i32, y: i32) -> Row {
    unsafe {
        let dimx: i32 = read(gps, layout.dimx);
        let dimy: i32 = read(gps, layout.dimy);
        let flag: *const u32 = read(gps, layer.flag);
        if flag.is_null() || x < 0 || y < 0 || x >= dimx || y >= dimy {
            return Row::Whole;
        }
        let f = *flag.add((x * dimy + y) as usize);
        if f & FLAG_BOTTOM_OF_TEXT != 0 {
            Row::Bottom
        } else if f & FLAG_TOP_OF_TEXT != 0 {
            Row::Top
        } else {
            Row::Whole
        }
    }
}

/// 目前的游標 (screenx, screeny)
unsafe fn cursor(gps: *const u8) -> (i32, i32) {
    LAYOUT.get().map_or((0, 0), |l| unsafe { (read(gps, l.screenx), read(gps, l.screeny)) })
}

/// DFHack 照原樣畫的一段字（Lua 包住的 paintString 沒有譯文時，見 lua/loader.lua）
pub fn dfhack_plain(gps: usize, x: i32, y: i32, top: bool, text: &[u8]) {
    unsafe { record_plain(gps as *mut u8, if top { top_layer } else { main_layer }, x, y, text, 0, true) };
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
            let (x0, y) = unsafe { cursor(gps) };
            unsafe { draw(&*s) };
            unsafe { record_plain(gps, layer, x0, y, (*s).bytes(), 0, false) };
            return true;
        }
        Plan::Missing(text) => {
            note_miss("wrap", &text);
            let (x0, y) = unsafe { cursor(gps) };
            unsafe { draw(&*s) };
            unsafe { record_plain(gps, layer, x0, y, (*s).bytes(), 0, false) };
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
            paint(gps, &layer(layout), x + p.dx, y + p.dy, Paint::of(&p.entry), dict, true);
        }
        for (off, c) in layout.color.iter().zip(color) {
            write(gps, *off, c);
        }
        write(gps, layout.screenx, x);
        write(gps, layout.screeny, y);
    }
    true
}

unsafe extern "C" fn hook_render_things() {
    let orig: RenderThingsFn = unsafe { std::mem::transmute(TRAMPOLINES[14].load(Ordering::Acquire)) };
    crate::overlay::frame_begin();
    ENGLISH.lock().begin();
    unsafe { orig() };
    ENGLISH.lock().end();
    crate::overlay::frame_end();
}

// ---- 翻譯前的英文字元 ----
//
// DFHack 有些腳本讀畫面上的字找版面位置（overall-mood 找人物頁的「Overview」分頁、stressicons 找頂列的「Pop」）。
// 翻成中文以後找不到，疊加介面就畫到預設位置。這裡記下每一幀翻譯過的格子原本是哪個英文字元，
// Lua 包住 dfhack.screen.readTile 回報英文（見 lua/loader.lua 的 install_read_tile）。

/// [這一幀正在寫的, 上一幀寫完的]：(x*dimy+y → 英文字元，0 是沒翻譯；dimy)
pub struct English([(Vec<u8>, i32); 2]);

pub static ENGLISH: Mutex<English> = Mutex::new(English([(Vec::new(), 0), (Vec::new(), 0)]));

impl English {
    fn begin(&mut self) {
        self.0[0].0.fill(0);
    }

    fn end(&mut self) {
        self.0.swap(0, 1);
    }

    /// 原文 key 從 (x0, y) 起寫在這一幀的畫面上（譯文較短時，原文的格數照樣記滿）
    fn note(&mut self, x0: i32, y: i32, key: &[u8], dimx: i32, dimy: i32) {
        let (cells, d) = &mut self.0[0];
        let size = (dimx * dimy).max(0) as usize;
        if *d != dimy || cells.len() != size {
            *cells = vec![0; size];
            *d = dimy;
        }
        if !(0..dimy).contains(&y) {
            return;
        }
        for (k, &b) in key.iter().enumerate() {
            let x = x0 + k as i32;
            if (0..dimx).contains(&x) {
                cells[(x * dimy + y) as usize] = b;
            }
        }
    }

    /// 上一幀 (x, y) 原本的英文字元
    pub fn at(&self, x: i32, y: i32) -> Option<u8> {
        let (cells, dimy) = &self.0[1];
        if x < 0 || y < 0 || y >= *dimy {
            return None;
        }
        cells.get((x * dimy + y) as usize).copied().filter(|&b| b != 0)
    }
}

unsafe extern "C" fn hook_addst(gps: *mut u8, s: *const StdString, just: i32, space: i32) {
    let orig: AddstFn = unsafe { std::mem::transmute(TRAMPOLINES[0].load(Ordering::Acquire)) };
    let key = unsafe { (*s).bytes() };
    let done = unsafe {
        paragraph_word(gps, s, main_layer, |st| orig(gps, st, just, space))
            || translate_call(gps, "addst", key, space, main_layer, |b| orig(gps, &StdString::borrowed(b), just, space))
    };
    if !done {
        let (x0, y) = unsafe { cursor(gps) };
        unsafe { orig(gps, s, just, space) };
        unsafe { record_plain(gps, main_layer, x0, y, key, space, false) };
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
        let (x0, y) = unsafe { cursor(gps) };
        unsafe { orig(gps, s, just, space, sflag) };
        unsafe { record_plain(gps, main_layer, x0, y, key, space, false) };
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
        let (x0, y) = unsafe { cursor(gps) };
        unsafe { orig(gps, s, just, space) };
        unsafe { record_plain(gps, top_layer, x0, y, key, space, false) };
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
        let (x0, y) = unsafe { cursor(gps) };
        unsafe { orig(gps, s, colors) };
        unsafe { record_plain(gps, main_layer, x0, y, key, 0, false) };
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
            let (x0, y) = unsafe { cursor(gps) };
            draw(s, unsafe { s.add(color) });
            unsafe { record_plain(gps, main_layer, x0, y, CStr::from_ptr(s.cast()).to_bytes(), 0, false) };
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
            paint(gps, &layout.main, x + p.dx, y + p.dy, Paint::of(&p.entry), dict, true);
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
    /// 遊戲畫這些字的貼圖時往下位移的像素（add_anchored_tile 的 ay）
    lift: i32,
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
            let done = label.replace(Label { gps: gps as usize, x0: x, y, lift: ay, font, text: vec![ch] });
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
    let Some(tiles) = label_tiles(label.text.len(), &entry, dict, |c| unsafe { font_tex(label.font, c) }) else {
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
    crate::overlay::record_label(label.gps as *mut u8, label.x0, label.y, &tiles, &entry.text, label.lift);
}

/// 標籤每一格換成的貼圖：譯文在原文寬度裡置中，中文字放標籤用字圖（小一號，見 atlas::render）的左右半格，
/// 其他字用字型表的字，多出來的格放空白。譯文比原文寬就回傳 None（照畫英文）。
fn label_tiles(n: usize, entry: &Entry, dict: &Dict, font: impl Fn(u8) -> i32) -> Option<Vec<i32>> {
    let shadow = entry.shadow();
    let pad = n.checked_sub(shadow.len())? / 2;
    let tile = |k: usize| match k.checked_sub(pad).filter(|&r| r < shadow.len()) {
        Some(r) => match entry.glyphs.iter().find(|&&(off, _)| r == off || r == off + 1) {
            Some(&(off, g)) => dict.tile(g, r - off, Row::Label).unwrap_or_else(|| font(shadow[r])),
            None => font(shadow[r]),
        },
        None => font(b' '),
    };
    Some((0..n).map(tile).collect())
}

#[cfg(test)]
mod tests {
    use super::{label_tiles, placed_align, wrap_plan, Columns, MissLog, MISS_BYTES, MISS_LIMIT};
    use crate::dict::Dict;
    use std::collections::HashMap;

    #[test]
    fn alignment_follows_where_the_game_put_the_english() {
        // 一列 40 格；按鈕：第 2 格左緣、3–14 格中段、第 15 格右緣
        let button = |x: i32| match x {
            2 => 7,
            3..=14 => 8,
            15 => 9,
            _ => 0,
        };
        assert_eq!(placed_align(button, 4, 10, 40, false, b'l'), b'c', "Export XML：兩旁各剩一格是置中");
        assert_eq!(placed_align(button, 4, 8, 40, false, b'l'), b'l', "兩旁窄（左 1 右 3）差兩格是靠左（建立世界的「Everywhere」）");
        let wide = |x: i32| if (76..=114).contains(&x) { 814 } else { 0 };
        assert_eq!(placed_align(wide, 91, 7, 192, false, b'c'), b'c', "兩旁寬時差兩格也算置中（選擇遊戲模式的 Legends 左 15 右 17）");
        assert_eq!(placed_align(button, 4, 6, 40, false, b'c'), b'l', "冒險角色的「隨機性格」：遊戲靠左畫在按鈕上，字串檔寫置中也照靠左");
        assert_eq!(placed_align(button, 4, 9, 40, true, b'l'), b'l', "靠左排的清單裡剛好看起來置中的一項（工作分組的 Plant gatherers）");
        assert_eq!(placed_align(button, 8, 6, 40, false, b'r'), b'r', "不置中時靠右照字串檔");
        assert_eq!(placed_align(button, 8, 6, 40, false, b'l'), b'r', "右邊只剩一格、左邊空很多是靠右（存檔清單的「Folder: region1」）");
        assert_eq!(placed_align(button, 4, 3, 40, true, b'r'), b'l', "字串檔寫靠右的點數，在長短不同的同一欄裡是靠左（冒險裝備的「100 pts」）");
        // 說明視窗：1–6 格是問號框（各格貼圖不同），7–40 格是視窗本體；標題在整排（1–40）的正中間
        let window = |x: i32| match x {
            1..=6 => 50 + x,
            7..=40 => 8,
            _ => 0,
        };
        assert_eq!(placed_align(window, 16, 10, 80, false, b'l'), b'c', "說明視窗的標題（工作指令、冒險模式）");
        // 沒有按鈕：原文在畫面正中間算置中（差一格也算），整列的字不算
        assert_eq!(placed_align(|_| 0, 15, 10, 40, false, b'l'), b'c');
        assert_eq!(placed_align(|_| 0, 14, 10, 40, false, b'l'), b'c');
        assert_eq!(placed_align(|_| 0, 2, 10, 40, false, b'l'), b'l');
        assert_eq!(placed_align(|_| 0, 0, 40, 40, false, b'l'), b'l');
        assert_eq!(placed_align(|_| 0, 2, 10, 40, false, b'c'), b'c', "其他照字串檔");
    }

    #[test]
    fn a_start_shared_by_texts_of_different_length_is_a_left_aligned_list() {
        let mut c = Columns(HashMap::new());
        // 工作分組：按鈕中段 8–33 格，字都從第 13 格開始；選取中的那一項按鈕貼圖不同，左緣一樣在第 8 格
        let item = |x: i32| if (8..=33).contains(&x) { 78 } else { 0 };
        let selected = |x: i32| if (8..=33).contains(&x) { 92 } else { 0 };
        assert!(!c.note(item, 13, 6), "清單第一項還看不出來");
        assert!(c.note(selected, 13, 15), "同一欄（按鈕左緣、起點都一樣）、長度差兩格以上；選取中的按鈕貼圖不同也算");
        let keys = |x: i32| if (38..=60).contains(&x) { 90 } else { 0 };
        assert!(!c.note(keys, 40, 14));
        assert!(c.note(keys, 40, 12), "按鍵設定的「Delayed repeat」與「Don't repeat」差兩格");
        let menu = |x: i32| if (76..=114).contains(&x) { 814 } else { 0 };
        assert!(!c.note(menu, 91, 8));
        assert!(!c.note(menu, 91, 7), "只差一格：置中的按鈕整數除剛好同一格（Fortress、Legends）");
        assert!(!c.note(|_| 0, 13, 30), "不在按鈕上不記");
        assert!(c.note(item, 13, 6), "記住看過的長度：之後再畫第一項就知道（換分頁時同一顆按鈕換字也是）");
        let narrow = |x: i32| if (8..=20).contains(&x) { 78 } else { 0 };
        assert!(!c.note(narrow, 13, 15), "左緣、起點相同但按鈕寬度不同的是別種框");
    }

    #[test]
    fn label_is_centered_in_the_english_width() {
        let mut d = Dict::default();
        assert!(d.set_glyphs("關鍵", vec![10, 11, 20, 21]));
        d.load_tsv("Crucial\t關鍵\nC1\tA關\n");
        let font = |c: u8| 500 + c as i32;
        let sp = font(b' ');
        // Crucial 7 格、關鍵 4 格：左邊空 1 格，右邊空 2 格
        assert_eq!(label_tiles(7, &d.entries[&b"Crucial"[..]], &d, font), Some(vec![sp, 10, 11, 20, 21, sp, sp]));
        // 英數照字型表；放不下就不換
        assert_eq!(label_tiles(3, &d.entries[&b"C1"[..]], &d, font), Some(vec![font(b'A'), 10, 11]));
        assert_eq!(label_tiles(2, &d.entries[&b"C1"[..]], &d, font), None);
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

    /// 測試用的紀錄檔（每個測試一個檔名），結束時刪掉
    struct TempLog(std::path::PathBuf);

    impl TempLog {
        fn new(name: &str, content: &[u8]) -> TempLog {
            let path = std::env::temp_dir().join(format!("dflang-misses-{}-{name}.tsv", std::process::id()));
            std::fs::write(&path, content).unwrap();
            TempLog(path)
        }

        fn path(&self) -> &str {
            self.0.to_str().unwrap()
        }

        fn text(&self) -> String {
            std::fs::read_to_string(&self.0).unwrap()
        }
    }

    impl Drop for TempLog {
        fn drop(&mut self) {
            let _ = std::fs::remove_file(&self.0);
        }
    }

    #[test]
    fn miss_log_keeps_strings_already_in_the_file() {
        // 上次寫到一半（最後一行沒換行）；CP437 的 ë（0x89）照解碼後的字比對
        let tmp = TempLog::new("reopen", "addst\tHello\nmtb\tUrist ë".as_bytes());
        let mut log = MissLog::open(tmp.path()).unwrap();
        assert_eq!(log.seen.len(), 2);
        assert!(log.note("addst", b"Hello") && log.note("wrap", b"Urist \x89"), "記過的不再寫");
        assert!(log.note("dfhack", b"New one") && log.note("addst", b"New one"));
        drop(log);
        let mut log = MissLog::open(tmp.path()).unwrap();
        assert_eq!(log.seen.len(), 3, "重開時讀回這次新記的");
        assert!(log.note("addst", b"New one"));
        drop(log);
        assert_eq!(tmp.text(), "addst\tHello\nmtb\tUrist ë\ndfhack\tNew one\n", "補上換行、每句只記一次");
    }

    #[test]
    fn miss_log_stops_at_the_limit() {
        let lines: String = (0..MISS_LIMIT - 1).map(|i| format!("addst\tLine {i}\n")).collect();
        let tmp = TempLog::new("limit", lines.as_bytes());
        let mut log = MissLog::open(tmp.path()).unwrap();
        assert!(!log.full());
        assert!(!log.note("addst", b"Last one"), "記到上限就回報滿了");
        assert!(!log.note("addst", b"Too many"));
        drop(log);
        let text = tmp.text();
        assert!(text.ends_with("addst\tLast one\n") && !text.contains("Too many"), "滿了之後不再寫");
        assert!(MissLog::open(tmp.path()).unwrap().full(), "重開時照樣是滿的");

        let big = TempLog::new("bytes", format!("mtb\t{}\n", "a".repeat(MISS_BYTES as usize)).as_bytes());
        let mut log = MissLog::open(big.path()).unwrap();
        assert!(log.full() && !log.note("addst", b"More"), "檔案大小到上限也停");
    }
}
