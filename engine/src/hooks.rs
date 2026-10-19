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
use crate::dict::{glyph_cells, Dict, Entry, Row};
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
/// 目前的字典；換語言時換成另一份（lib.rs 的 use_dict、commit），舊的照留，遊戲執行緒可能還在讀
pub static DICT: AtomicPtr<Dict> = AtomicPtr::new(std::ptr::null_mut());
pub static ENABLED: AtomicBool = AtomicBool::new(false);
pub static HITS: AtomicU64 = AtomicU64::new(0);
pub static MISSES: AtomicU64 = AtomicU64::new(0);
/// 目前的 DFHack 焦點字串（多個以換行分隔），Lua 每幀更新；依畫面套用的譯文看它（data/screens）
pub static FOCUS: Mutex<String> = Mutex::new(String::new());

/// 模組畫面（標題的模組管理員、建立世界與競技場的模組選擇：焦點 title/Mods、new_region/Mods、new_arena/Mods）。
/// 這些畫面上第三方模組的名稱、作者、說明和字典同字也照原文（見 Dict::keeps_player_name）
pub fn on_mods_screen() -> bool {
    FOCUS.lock().split('\n').any(|f| f.split('/').nth(1) == Some("Mods"))
}

/// 遊戲字型表 init.font.small_font_texpos、large_font_texpos 的位址（各 256 個 texpos，依 CP437 字元排）
pub static FONTS: OnceLock<[usize; 2]> = OnceLock::new();
/// 設定換過幾次（lib.rs 的 refresh）：段落、提示、彩色文字框、DFHack 段落登記的畫字計畫記下算的時候是第幾版（見 Stamp）
static EPOCH: AtomicU64 = AtomicU64::new(0);

/// 算畫字計畫時的設定版本與字典（字典換了就是換了語言，舊計畫的字圖編號對不上）。版本不同的計畫要照現在的設定重算
#[derive(Clone, Copy, PartialEq, Eq, Hash, Debug)]
pub struct Stamp {
    epoch: u64,
    dict: usize,
}

impl Stamp {
    pub fn of(dict: &Dict) -> Stamp {
        Stamp { epoch: EPOCH.load(Ordering::Acquire), dict: std::ptr::from_ref(dict) as usize }
    }

    /// 計畫是照這份字典算的（字圖編號對得上）
    pub fn same_dict(self, dict: &Dict) -> bool {
        self.dict == std::ptr::from_ref(dict) as usize
    }
}

/// 設定換了（選項、名字音譯、整句貼圖、字型、字的大小）：之前算好的畫字計畫都算舊的
pub fn bump_epoch() {
    EPOCH.fetch_add(1, Ordering::AcqRel);
}

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
/// (language_name, 輸出的原名)
type NativeNameFn = unsafe extern "C" fn(*const u8, *mut StdString);
/// (std::string, 新長度, 變長時的填充字元)
type ResizeFn = unsafe extern "C" fn(*mut StdString, usize, u8);
/// (viewscreen)
type ScreenFn = unsafe extern "C" fn(*mut u8);
/// (冒險模式介面, 框左緣, 框右緣)
type LogFn = unsafe extern "C" fn(*mut u8, i32, i32);
/// (交談畫面, ?, 框左緣, 框右緣)
type TalkLogFn = unsafe extern "C" fn(*mut u8, u32, i32, i32);
/// (冒險模式的通知清單, ?, 框左緣, 框右緣)
type AnnouncementsFn = unsafe extern "C" fn(*mut u8, u64, i32, i32);
/// render_things()：非同步執行緒每幀把畫面寫進緩衝區（整句貼圖在這裡收一幀的句子，見 overlay.rs）
type RenderThingsFn = unsafe extern "C" fn();
/// FUN_1408c2ca0(工作選項, vector<string*>* 提示框的行清單)：工坊「新增任務」的提示框（產出／需要）
type ToolTipFn = unsafe extern "C" fn(*mut u8, *mut u8);
/// FUN_14036a9c0(交易畫面, 神器, 勢力, 勢力, 交出, 奪取)：交易時交出或被奪神器，商人說的話切成行放進交易畫面 +0x3c0
type TradeSpeechFn = unsafe extern "C" fn(*mut u8, usize, usize, usize, u8, u8);
/// std::string::find 的內部實作（原文, 長度, 從第幾個字找, 要找的字, 長度）→ 找到的位置或 -1：遊戲的搜尋框用它比對清單項目
/// （已轉小寫、去掉重音的英文）和打的字
type FindFn = unsafe extern "C" fn(*const u8, usize, usize, *const u8, usize) -> usize;
/// FUN_140c39700(輸入框的字串, 長度上限, 收哪些字元的旗標, 按鍵, SDL 送來的文字) → 有沒有改：遊戲所有輸入框打字的地方
type TextInputFn = unsafe extern "C" fn(*mut StdString, i32, u64, *mut u8, *const u8) -> u64;

pub const HOOKS: [&str; 23] = [
    "addst", "addst_flag", "top_addst", "addcoloredst", "mtb_set_width", "add_anchored_tile", "wrap_lines", "ctb_add_string",
    "translate_name", "string_resize", "adv_appearance", "adv_personality", "adv_log", "adv_talk_log", "render_things", "native_name",
    "adv_announcements", "tool_tip", "world_site_history", "trade_artifact_speech", "dfhack_mtb_set_width", "stock_string_find",
    "text_input",
];
static TRAMPOLINES: [AtomicUsize; 23] = [const { AtomicUsize::new(0) }; 23];
/// wrap_text 的位址（只登記、不攔；它切行時呼叫 wrap_lines）：enable 時從呼叫它的程式碼找出遊戲啟動時就切好的提示（見 lines.rs）
pub static WRAP_TEXT: OnceLock<usize> = OnceLock::new();

/// (HOOKS 的序號, detour)
struct Detours(Vec<(usize, RawDetour)>);
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
        14 => hook_render_things as *const (),
        15 => hook_native_name as *const (),
        16 => hook_adv_announcements as *const (),
        17 => hook_tool_tip as *const (),
        18 => hook_world_site_history as *const (),
        19 => hook_trade_artifact_speech as *const (),
        20 => hook_dfhack_mtb_set_width as *const (),
        21 => hook_string_find as *const (),
        _ => hook_text_input as *const (),
    };
    let detour = unsafe { RawDetour::new(target as *const (), hook) }.map_err(|e| e.to_string())?;
    TRAMPOLINES[index].store(detour.trampoline() as *const () as usize, Ordering::Release);
    DETOURS.lock().0.push((index, detour));
    Ok(())
}

/// 沒有掛遊戲的 hook 也能開：本 MOD 讓給別的翻譯引擎時，只用 DFHack 視窗那條路（Lua 包住的 paintString）把標題畫面的切換說明畫成中文。
/// 關掉時 render_things 的 hook 不拆：其他 hook 都拆了，它照跑只換幀、計時（關著時量得到遊戲本身畫一幀多久，見 overlay::dump）
pub fn set_enabled(on: bool) -> Result<(), String> {
    let detours = DETOURS.lock();
    for (index, d) in &detours.0 {
        if !on && HOOKS[*index] == "render_things" {
            continue;
        }
        unsafe { if on { d.enable() } else { d.disable() } }.map_err(|e| e.to_string())?;
    }
    ENABLED.store(on, Ordering::Release);
    if !on {
        // 清掉收到一半的字與最後一幀（之後每幀 frame_end 都放空的一幀，見 overlay::record）
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
/// 結尾有幾格是補到原文寬度的空白（不是譯文本身的空白，見 overlay.rs 的 Chain）、
/// 原文比替身字串長出來、遊戲沒寫的格數（tail，靠左的譯文比英文短時；見 overlay.rs 的 chainable），
/// 以及段落照字型實際字寬縮短時的 (縮排格數, 記號格數, 文字)（Entry::compact）。整句貼圖（overlay.rs）都會用到。
struct Paint<'a> {
    shadow: &'a [u8],
    glyphs: &'a [(usize, u32)],
    align: u8,
    pad: usize,
    tail: usize,
    compact: Option<(usize, usize, &'a str)>,
}

impl<'a> Paint<'a> {
    fn of(entry: &'a Entry) -> Paint<'a> {
        let shadow = entry.shadow();
        let compact = entry.compact.map(|lead| (lead, shadow.len() - lead, entry.text.as_str()));
        Paint { shadow, glyphs: &entry.glyphs, align: entry.align, pad: 0, tail: 0, compact }
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
                crate::overlay::record(gps, top, row, x0 + lead as i32, y, shadow, &[], b'l', 0, 0, Some(text), dfhack);
            }
            return;
        }
        // 整句的列別；同一句混了不同列別就不交給整句貼圖
        let mut kind: Option<Option<Row>> = None;
        for &(offset, glyph) in p.glyphs {
            for half in 0..glyph_cells(glyph) {
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
            Some(Some(row)) => crate::overlay::record(gps, top, row, x0, y, p.shadow, p.glyphs, p.align, p.pad, p.tail, None, dfhack),
            // 沒有字圖的字串（傳說頁去掉黏著標點、照英文畫的連結字，只有英數的譯文）：也交給整句貼圖，
            // 和旁邊的段落文字接成一行緊排，不然會照遊戲的點陣字或從格子讀回來、留下英文字寬的空隙
            None if p.glyphs.is_empty() => {
                if let Some(index) = cell(0) {
                    crate::overlay::record(gps, top, row_at(index), x0, y, p.shadow, &[], p.align, p.pad, p.tail, None, dfhack);
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
    lookup_in(dict, key, y, 0)
}

/// 換一種寫法再查（截短前的原文、去掉行尾空白、截斷處前面那段）最多幾層：原文本身又以「...」「 ...」或空白結尾時
/// （「Abc ... ...」截成「Abc ...」、「Xyz... 」截成「Xyz...」），寫法換來換去會繞回原來那個，不設上限就把堆疊用完
const LOOKUP_DEPTH: u8 = 3;

fn lookup_in<'a>(dict: &'a Dict, key: &[u8], y: i32, depth: u8) -> Option<Found<'a>> {
    let screen = if dict.screens.is_empty() { None } else { dict.screen_entry(key, &FOCUS.lock(), y) };
    if let Some(entry) = screen.or_else(|| dict.entries.get(key)) {
        return Some(Found::Fixed(entry));
    }
    let deeper = depth < LOOKUP_DEPTH;
    // 遊戲截短的字串（`Start new conversation with the Human baro...`）：翻截短前的原文再截短
    if let Some(e) = deeper.then(|| cut_original(key)).flatten().filter(|full| !dict.keeps_player_name(full))
        .and_then(|full| lookup_in(dict, &full, y, depth + 1))
        .and_then(|f| dict.cut_entry(key, &f.text))
    {
        return Some(Found::Rule(e));
    }
    if let Some(e) = dict.rule_entry(key) {
        return Some(Found::Rule(e));
    }
    if !deeper {
        return None;
    }
    // 遊戲自己依寬度切開的一行（工坊任務提示框「需要：」的物品條件：FUN_1408c2ca0 照 29 格切）：切點的一個空白留在行尾，
    // 整行查不到時去掉它再查
    if let Some(line) = key.strip_suffix(b" ").filter(|l| !l.ends_with(b" ") && l.iter().any(u8::is_ascii_alphabetic)) {
        return lookup_in(dict, line, y, depth + 1);
    }
    // 截在字中間、最後一段是逗號後的職業或名字（物品頁「Owner: X, Stonecr...」、傳說人物分頁「名字, "綽號片...」，見 Dict::complete_cut）：
    // 補完整再翻、截回原來的格數
    if let Some(e) = dict.complete_cut(key).and_then(|full| lookup_in(dict, &full, y, depth + 1)).and_then(|f| dict.cut_entry(key, &f.text)) {
        return Some(Found::Rule(e));
    }
    // 遊戲直接改長度截短、沒經過 resize 的字串（冒險同伴清單的名字欄寫死 33 格：「Zebna Histekuthros, Donkey, ♂ ...」）：
    // 截在字與字之間時翻前面完整的那段（截在逗號後的去掉逗號：傳說分頁「名字, ...」），接「…」；截在字中間的照英文
    let head = key.strip_suffix(b" ...").filter(|h| h.iter().any(u8::is_ascii_alphabetic))?;
    let head = head.strip_suffix(b",").unwrap_or(head);
    lookup_in(dict, head, y, depth + 1).and_then(|f| dict.cut_entry(key, &format!("{}…", f.text))).map(Found::Rule)
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
/// half：遊戲畫成半格位移的兩列（分頁標籤，addst_flag 的 TOP_OF_TEXT／BOTTOM_OF_TEXT）。
unsafe fn translate_call(gps: *mut u8, kind: &str, key: &[u8], space: i32, half: bool, layer: fn(&Layout) -> Layer,
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
    // 玩家取的名字（倉儲區、小隊排程…）、玩家正在遊戲輸入框打的字照原樣畫，不記未翻譯
    if dict.keeps_player_name(key) || crate::typed::is_typed(key) {
        return false;
    }
    let Some(entry) = lookup(dict, key, y) else {
        note_miss(kind, key);
        return false;
    };
    // 字典的整句比原文寬（名稱檔的三字名「食人魔」、職位名「女領主」）：整句貼圖開著時照字寬壓窄放進原文寬度；
    // 放不下（或整句貼圖關著）照舊多畫幾格
    let entry = match entry {
        Found::Fixed(e) if e.compact.is_none() && e.shadow().len() > key.len() => {
            dict.squeezed_fixed(e, key.len()).map_or(Found::Fixed(e), Found::Rule)
        }
        found => found,
    };
    // 照字型實際字寬縮短的譯文（音譯的名字、按鍵名，見 Dict::rule_entry）：遊戲限制的長度放不下就照英文
    if entry.compact.is_some() && space > 0 && entry.shadow().len() > space as usize {
        return false;
    }
    HITS.fetch_add(1, Ordering::Relaxed);
    let (dimx, dimy): (i32, i32) = unsafe { (read(gps, layout.dimx), read(gps, layout.dimy)) };
    ENGLISH.lock().note(x0, y, key, dimx, dimy);
    let (cells, glyphs) = entry.fit(space);
    let (shadow, glyphs) = (&entry.shadow()[..cells], &entry.glyphs[..glyphs]);
    let n = key.len();
    let lay = layer(layout);
    let lower = |x: i32| unsafe { lower_at(gps, layout, &lay, x, y) };
    let column = COLUMNS.lock().note(lower, lay.screen == layout.top.screen, half, x0, n as i32);
    let fits = cells == entry.shadow().len() && (space <= 0 || space as usize >= n);
    let align = if fits { placed_align(lower, x0, n as i32, dimx, column, entry.align) } else { entry.align };
    // 字串檔的對齊和遊戲實際的不同（靠左、置中、靠右互換）時，譯文在原文寬度裡重排
    let relayout = align != entry.align;
    unsafe {
        if entry.compact.is_some() {
            // 從原文的起點靠左畫：格子貼透明記號，字由整句貼圖照實際字寬畫（同段落，見 paint）
            write(entry.shadow());
            paint(gps, &lay, x0, y, Paint::of(&entry), dict, true);
        } else if relayout {
            // 譯文實際佔的格子（字圖格的替身是空白，要算進去；置中的譯文前後補了空白，見 Dict::entry）
            let start = glyphs.iter().map(|g| g.0).chain(shadow.iter().position(|&c| c != b' ')).min().unwrap_or(0);
            let end = glyphs.iter().map(|g| g.0 + glyph_cells(g.1)).chain(shadow.iter().rposition(|&c| c != b' ').map(|p| p + 1)).max().unwrap_or(0);
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
            paint(gps, &lay, x0, y, Paint { shadow: &out, glyphs: &moved, align, pad, tail: 0, compact: None }, dict, true);
        } else {
            write(shadow);
            // 譯文比原文短時遊戲只寫了譯文那幾格：原文本來畫到哪一格也記下（Paint::tail），遊戲照英文字寬排的下一段
            // （選擇出發地點的金屬清單：addst 不限寬度，下一個從英文長度＋1 格起畫）才接得上。限了寬度的是表格的欄，
            // 下一欄的位置是固定的，不記
            let tail = if space > 0 { 0 } else { n.saturating_sub(cells) };
            paint(gps, &lay, x0, y, Paint { shadow, glyphs, align: entry.align, pad: 0, tail, compact: None }, dict, true);
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
/// 起點那格有貼圖）時：同一種按鈕、同一個起點，畫面上同時有長短很不一樣的字（column，見 Columns）是靠左排的清單（字串檔寫靠右的
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

/// 原文在框裡左右剩的格數 (左, 右) 看起來是置中：差一格以內；兩旁都還很寬時差兩格也算（選擇遊戲模式的 Legends 左 15 右 17），
/// 兩旁窄時差兩格就是靠左。整句貼圖也用它判斷置中的字要不要對準這張框的正中間（overlay.rs 的 box_shift）
pub(crate) fn centered_in((left, right): (i32, i32)) -> bool {
    let gap = (left - right).abs();
    gap <= 1 || (gap <= 2 && left.min(right) >= 4)
}

/// 按鈕上的字從哪一格開始：(頂層, 半格位移, 按鈕中段的左緣, 右緣, 起點) → 見過的最短、最長原文，分這一幀與上一幀記。
/// 靠左排的清單（工作分組、建造選單、冒險角色的按鈕、按鍵設定的連發選項、建立世界的選項）每一項都從按鈕左緣算起的同一格開始，
/// 選取中的按鈕貼圖不同也一樣；置中的按鈕字長短差兩格以上，起點就不會相同（只差一格的可能因為整數除剛好同一格，
/// 像「Fortress」與「Legends」）。右緣也比：清單的按鈕一樣寬，左緣剛好同一格的別種框（冒險角色的「Name」標籤和下面的分頁）
/// 不算同一欄。頂層（彈出視窗、提示框）和主層、半格位移的分頁標籤和一般列也分開算。
/// 只看畫面上現在有的字：這一幀，加上上一幀（清單前面幾項畫的時候，後面幾項這一幀還沒畫到）。以前記一整局，別的畫面、
/// 提示框剛好畫在同一個位置、長度不同的字，會讓之後的分頁標籤一直靠左（人物頁的「想法」）
#[derive(Default)]
pub struct Columns {
    now: HashMap<ColumnKey, (i32, i32)>,
    last: HashMap<ColumnKey, (i32, i32)>,
}

type ColumnKey = (bool, bool, i32, i32, i32);

pub static COLUMNS: LazyLock<Mutex<Columns>> = LazyLock::new(|| Mutex::new(Columns::default()));

impl Columns {
    /// 記下這一筆（lower 是這一列字底下的貼圖編號，不在按鈕上的不記；top：畫在頂層；half：半格位移的列），
    /// 回傳這個起點是不是靠左排的清單
    fn note(&mut self, lower: impl Fn(i32) -> i32, top: bool, half: bool, x0: i32, n: i32) -> bool {
        let mid = lower(x0);
        if mid == 0 {
            return false;
        }
        let left = x0 - (1..).take_while(|&d| lower(x0 - d) == mid).count() as i32;
        let right = x0 + (0..).take_while(|&d| lower(x0 + d) == mid).count() as i32;
        let key = (top, half, left, right, x0);
        let e = self.now.entry(key).or_insert((n, n));
        *e = (e.0.min(n), e.1.max(n));
        let (shortest, longest) = self.last.get(&key).map_or(*e, |&(s, l)| (s.min(e.0), l.max(e.1)));
        longest - shortest >= 2
    }

    /// 一幀畫完（render_things 結束）：這一幀變成上一幀
    fn next_frame(&mut self) {
        std::mem::swap(&mut self.now, &mut self.last);
        self.now.clear();
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

/// DFHack 的一段（dfhack_wrap 登記時的整段英文、寬度與切好的英文行）：設定換了以後照它重算（見 dh_line）
struct DhPara {
    text: Vec<u8>,
    width: usize,
    lines: Vec<Vec<u8>>,
}

/// 段落的一行：要畫的（Some(None)：中文行比英文少，這一行畫空白；None：這一段現在翻不出來，照原樣查，設定換了以後再試），
/// 和它屬於哪一段、照哪一版設定算的
struct DhLine {
    row: Option<Option<Arc<Entry>>>,
    para: Arc<DhPara>,
    stamp: Stamp,
}

/// DFHack 自動換行的段落（string:wrap 切的，見 dfhack_wrap）：英文行 → 同一列要畫的中文。
/// ponytail: 以英文行的文字當鍵、全部畫面共用：不同段落（或同一段的兩行）剛好有同一行英文時用最後登記的，別處單獨畫出同樣英文的字串
/// 也會換成這裡的中文或空白；表滿了整批清掉。真的撞到再改成依段落（WrappedLabel）登記。
static DH_LINES: LazyLock<Mutex<HashMap<Vec<u8>, DhLine>>> = LazyLock::new(|| Mutex::new(HashMap::new()));
const DH_LINES_LIMIT: usize = 20_000;

/// 一段的每一行：(英文第幾行, 要畫的)。翻得出來時是 Some(中文，None 是畫空白)；整段翻不出來、或有字缺字圖時每一行都是 None。
/// 只列有字的英文行（string:wrap 不畫空行）
fn dh_rows(dict: &Dict, para: &DhPara) -> Vec<(usize, Option<Option<Arc<Entry>>>)> {
    let translated: Option<Vec<_>> = wrap_plan(dict, &para.text, para.width, &para.lines).and_then(|plan| {
        plan.into_iter()
            .map(|(k, row)| Some((k, Some(match row {
                Some(row) => Some(Arc::new(dict.para_entry(&row)?)),
                None => None,
            }))))
            .collect()
    });
    translated.unwrap_or_else(|| {
        para.lines.iter().enumerate().filter(|(_, l)| l.iter().any(|c| !c.is_ascii_whitespace())).map(|(k, _)| (k, None)).collect()
    })
}

/// 登記過的段落行：Some(這一行要畫的中文，None 是畫空白)；沒登記、或這一段翻不出來時 None（照原樣查）。
/// 玩家改過設定或語言時照現在的設定重算整段
fn dh_line(dict: &Dict, text: &[u8]) -> Option<Option<Arc<Entry>>> {
    let mut table = DH_LINES.lock();
    let line = table.get(text)?;
    let stamp = Stamp::of(dict);
    if line.stamp != stamp {
        let para = line.para.clone();
        for (k, row) in dh_rows(dict, &para) {
            if let Some(l) = table.get_mut(&para.lines[k]).filter(|l| Arc::ptr_eq(&l.para, &para)) {
                l.row = row;
                l.stamp = stamp;
            }
        }
    }
    table.get(text)?.row.clone()
}

/// DFHack 要畫的一段字：有譯文就回傳影子字串（補到原長，頭尾空白照原文，視窗標題前後各有兩個空白），字圖留給 dfhack_stamp；
/// 沒有譯文、或譯文比原文寬時回傳 None，照畫英文。段落的一行（dfhack_wrap 登記過的）畫同一列的中文。
/// `center`：DFHack 依英文長度置中的字（分頁標籤、視窗標題，見 lua/loader.lua）：譯文放在原文（不含頭尾空白）的正中間，整句貼圖也置中畫。
pub fn dfhack_translate(text: &[u8], y: i32, center: bool) -> Option<Vec<u8>> {
    let dict = DICT.load(Ordering::Acquire);
    if dict.is_null() || !ENABLED.load(Ordering::Relaxed) {
        return None;
    }
    let dict = unsafe { &*dict };
    let lead = text.iter().take_while(|&&c| c == b' ').count();
    // 有縮排的段落（launcher 工具說明的清單、範例說明：helpdb 的 format_block 在每一行前面補縮排的空白）登記的是補空白前的行
    let line = dh_line(dict, text).map(|row| (row, 0)).or_else(|| (lead > 0).then(|| dh_line(dict, &text[lead..])).flatten().map(|row| (row, lead)));
    if let Some((row, at)) = line {
        let shadow = row.as_ref().map_or(&[][..], |e| e.shadow());
        let mut out = vec![b' '; text.len().max(at + shadow.len())];
        out[at..at + shadow.len()].copy_from_slice(shadow);
        let pad = out.len() - at - shadow.len();
        *DH_PENDING.lock() = row.map(|e| Pending {
            shadow: out.clone(),
            glyphs: e.glyphs.iter().map(|&(offset, glyph)| (offset + at, glyph)).collect(),
            align: b'l',
            pad,
            compact: e.compact.map(|indent| (at + indent, e.shadow().len() - indent, e.text.clone())),
        });
        return Some(out);
    }
    let key = text[lead..].trim_ascii_end();
    if !key.iter().any(u8::is_ascii_alphabetic) || keeps_command(dict, key) || dict.keeps_player_name(key) {
        return None;
    }
    let Some(entry) = lookup(dict, key, y) else {
        note_miss("dfhack", key);
        return None;
    };
    let pending = dfhack_place(&entry, text.len(), lead, key.len(), center)?;
    HITS.fetch_add(1, Ordering::Relaxed);
    let out = pending.shadow.clone();
    *DH_PENDING.lock() = Some(pending);
    Some(out)
}

/// DFHack 的大按鈕（widgets.makeButtonLabelText 一格一字畫圖格，見 lua/loader.lua 的 install_button）的第 `row` 列（從 0 算）：
/// 英文有字的 `rows` 列串成一句 `phrase` 查譯文，譯文依列數平分（每列 ⌈字數／列數⌉ 字），這一列分到的放在 `cells` 格的正中間。
/// 回傳影子字串，字圖留給 dfhack_stamp；這一列沒分到字時是一整列空白。整句翻不出來、或放不進時 None，按鈕照英文
pub fn dfhack_button(phrase: &[u8], row: usize, rows: usize, cells: usize, y: i32) -> Option<Vec<u8>> {
    let dict = DICT.load(Ordering::Acquire);
    if dict.is_null() || !ENABLED.load(Ordering::Relaxed) {
        return None;
    }
    let pending = button_row(unsafe { &*dict }, phrase, row, rows, cells, y);
    let out = pending.as_ref().map(|p| p.shadow.clone());
    *DH_PENDING.lock() = pending;
    out
}

fn button_row(dict: &Dict, phrase: &[u8], row: usize, rows: usize, cells: usize, y: i32) -> Option<Pending> {
    let Some(entry) = lookup(dict, phrase, y) else {
        if row == 0 {
            note_miss("dfhack", phrase);
        }
        return None;
    };
    let chars: Vec<char> = entry.text.chars().collect();
    let per = chars.len().div_ceil(rows.max(1));
    let part: String = chars.iter().skip(row * per).take(per).collect();
    let pending = dfhack_place(&dict.para_entry(&part)?, cells, 0, cells, true)?;
    if row == 0 {
        HITS.fetch_add(1, Ordering::Relaxed);
    }
    Some(pending)
}

/// DFHack 要畫的一段字（`text_len` 格：開頭 `lead` 格空白、原文 `key_len` 格）換成譯文 entry 時要寫的替身字串與要貼的字。
/// DFHack 的字位置由它的元件算好，字串檔的置中、靠右（給遊戲自己的按鈕用的，前面已補好空白）不套用：取譯文實際佔的格子
/// （字圖格的替身是空白，要算進去；照字型實際字寬縮短的譯文是全部記號格），靠左放在原文起點，或依 center 放在原文正中間。
/// 放不進原文的格子時 None。
fn dfhack_place(entry: &Entry, text_len: usize, lead: usize, key_len: usize, center: bool) -> Option<Pending> {
    let (shadow, glyphs) = (entry.shadow(), &entry.glyphs);
    // 照字型實際字寬縮短的譯文（音譯的名字、按鍵名，見 Dict::rule_entry）：替身全是空白，字由整句貼圖照 text 畫在記號格
    let cells = entry.compact.map(|indent| shadow.len() - indent);
    let (start, end) = match cells {
        Some(n) => (shadow.len() - n, shadow.len()),
        None => (
            glyphs.iter().map(|g| g.0).chain(shadow.iter().position(|&c| c != b' ')).min().unwrap_or(0),
            glyphs.iter().map(|g| g.0 + glyph_cells(g.1)).chain(shadow.iter().rposition(|&c| c != b' ').map(|p| p + 1)).max().unwrap_or(0),
        ),
    };
    let body = &shadow[start..end.max(start)];
    let at = if center { lead + key_len.saturating_sub(body.len()) / 2 } else { lead };
    if at + body.len() > text_len {
        return None;
    }
    let mut out = vec![b' '; text_len];
    out[at..at + body.len()].copy_from_slice(body);
    let glyphs = glyphs.iter().map(|&(offset, glyph)| (offset - start + at, glyph)).collect();
    let align = if center { b'c' } else { b'l' };
    // 譯文比原文短、空出來的格子才算補位；英文原本結尾的空白（欄位對齊、標籤與值之間）照算（見 Paint::pad）
    let pad = (lead + key_len).saturating_sub(at + body.len());
    Some(Pending { shadow: out, glyphs, align, pad, compact: cells.map(|n| (at, n, entry.text.clone())) })
}

/// DFHack 的 string:wrap（WrappedLabel、確認對話框都用它）把整段英文 text（`\n` 分段）依 width 格切成 lines，之後一行行畫：整段有譯文時依中文規則以同樣寬度重新換行，
/// 登記每一行英文要畫的中文（見 wrap_plan），回傳 true。缺一句就整段照英文，翻不出來的那一段記進未翻譯紀錄；
/// 這種段落也記下來（每一行照原樣查），玩家改了設定（段落缺句時其餘照翻）以後畫到時再試，不必等 DFHack 重新切行
pub fn dfhack_wrap(text: &[u8], width: usize, lines: &[Vec<u8>]) -> bool {
    let dict = DICT.load(Ordering::Acquire);
    if dict.is_null() || !ENABLED.load(Ordering::Relaxed) || lines.is_empty() || width == 0 {
        return false;
    }
    let dict = unsafe { &*dict };
    // 先記版本再算：算到一半玩家改了設定，這一段就算舊的，畫到時重算
    let stamp = Stamp::of(dict);
    let para = Arc::new(DhPara { text: text.to_vec(), width, lines: lines.to_vec() });
    let rows = dh_rows(dict, &para);
    let translated = rows.iter().any(|(_, row)| row.is_some());
    let mut table = DH_LINES.lock();
    if table.len() + rows.len() > DH_LINES_LIMIT {
        table.clear();
    }
    for (k, row) in rows {
        let line = DhLine { row, para: para.clone(), stamp };
        if translated {
            table.insert(lines[k].clone(), line);
        } else {
            // 翻不出來的段落不蓋掉別段登記的同一行英文
            table.entry(lines[k].clone()).or_insert(line);
        }
    }
    translated
}

/// refresh 用：清掉 dh_translate 查到、還沒貼的字（舊設定的字圖）
pub fn clear_pending() {
    *DH_PENDING.lock() = None;
}

/// DFHack 單獨畫出、或說明段落裡單獨一段的指令（launcher 說明窗格的「misery clear」「"enable misery"」）：這個畫面的 `*` 列
/// 讓全小寫的字照原文時（見 Dict::keeps_word），前後用雙引號包住的也照原文，不會被字典翻成「苦難透明」
fn keeps_command(dict: &Dict, text: &[u8]) -> bool {
    let text = text.trim_ascii();
    let bare = text.strip_prefix(b"\"").and_then(|t| t.strip_suffix(b"\"")).unwrap_or(text);
    !dict.verbatim_screens.is_empty() && dict.keeps_word(bare, &FOCUS.lock(), true)
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
        let zh = if para.iter().any(u8::is_ascii_alphabetic) && !keeps_command(dict, para) {
            let (pieces, missing) = dict.translate_sentences(para);
            if !missing.is_empty() {
                note_miss("dfhack", para);
            }
            pieces?.into_iter().map(|(t, _)| t).collect::<String>()
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
    let paint_of = Paint { shadow: &p.shadow, glyphs: &p.glyphs, align: p.align, pad: p.pad, tail: 0, compact };
    unsafe { paint(gps as *mut u8, &layer, x, y, paint_of, &*dict, false) };
}

fn main_layer(l: &Layout) -> Layer {
    l.main
}

fn top_layer(l: &Layout) -> Layer {
    l.top
}

/// 遊戲照原樣畫的字（沒有譯文）：記下按鈕上的字從哪一格開始（見 Columns；DFHack 的字不記，它的對齊不看 placed_align），
/// 也交給整句貼圖（overlay.rs），不用遊戲的點陣字，版面不動。(x0, y) 是畫之前的游標；space > 0 時遊戲最多畫 space 格；
/// dfhack 是 DFHack 畫的字。
unsafe fn record_plain(gps: *mut u8, layer: fn(&Layout) -> Layer, x0: i32, y: i32, text: &[u8], space: i32, dfhack: bool) {
    let Some(layout) = LAYOUT.get() else { return };
    let layer = layer(layout);
    let top = layer.screen == layout.top.screen;
    let n = if space > 0 { text.len().min(space as usize) } else { text.len() };
    let row = unsafe { row_of(gps, layout, &layer, x0, y) };
    if !dfhack {
        unsafe { COLUMNS.lock().note(|x| lower_at(gps, layout, &layer, x, y), top, row != Row::Whole, x0, n as i32) };
    }
    if !crate::overlay::ON.load(Ordering::Relaxed) {
        return;
    }
    crate::overlay::record(gps, top, row, x0, y, &text[..n], &[], b'l', 0, 0, None, dfhack);
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
    let dict = DICT.load(Ordering::Acquire);
    if dict.is_null() {
        return false;
    }
    let dict = unsafe { &*dict };
    let plan = unsafe { crate::mtb::word_plan(s as usize, dict).or_else(|| crate::lines::line_plan(s as usize, dict)) };
    let Some(plan) = plan else { return false };
    flush_label();
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
    let Some(layout) = LAYOUT.get() else { return true };
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
    let t0 = std::time::Instant::now();
    crate::overlay::frame_begin();
    crate::typed::refresh();
    ENGLISH.lock().begin();
    unsafe { orig() };
    unsafe { swap_tiles() };
    ENGLISH.lock().end();
    COLUMNS.lock().next_frame();
    crate::overlay::frame_end();
    crate::overlay::note_render(t0.elapsed());
}

/// 畫面上要換掉的貼圖：遊戲畫工作指令條件的圖形按鈕（「Type」「Mat」「Adj」的英文畫在圖裡）用自己記下的原版貼圖編號，
/// 改不到它查的表，所以每幀畫完後在畫面緩衝區（主層與底層）把那幾格換成本包畫的中文圖（見 lua/loader.lua 的 place_buttons）
struct TileSwaps {
    gps: usize,
    /// 只在焦點以它開頭的畫面換
    focus: String,
    /// (原版 texpos, 本包 texpos)，依原版排序
    map: Vec<(i32, i32)>,
}

static TILE_SWAPS: Mutex<Option<TileSwaps>> = Mutex::new(None);

/// 換上這一批（map 空的是不換）
pub fn set_tile_swaps(gps: usize, focus: String, mut map: Vec<(i32, i32)>) {
    map.sort_unstable();
    *TILE_SWAPS.lock() = (gps != 0 && !map.is_empty()).then_some(TileSwaps { gps, focus, map });
}

unsafe fn swap_tiles() {
    let swaps = TILE_SWAPS.lock();
    let (Some(s), Some(layout)) = (swaps.as_ref(), LAYOUT.get()) else { return };
    if !ENABLED.load(Ordering::Relaxed) || !FOCUS.lock().split('\n').any(|f| f.starts_with(s.focus.as_str())) {
        return;
    }
    let (lo, hi) = (s.map[0].0, s.map[s.map.len() - 1].0);
    unsafe {
        let gps = s.gps as *const u8;
        let (dimx, dimy): (i32, i32) = (read(gps, layout.dimx), read(gps, layout.dimy));
        let cells = (dimx.max(0) * dimy.max(0)) as usize;
        for array in [layout.main.texpos, layout.main.lower] {
            let tex: *mut i32 = read(gps, array);
            if tex.is_null() {
                continue;
            }
            for i in 0..cells {
                let t = *tex.add(i);
                if (lo..=hi).contains(&t)
                    && let Ok(k) = s.map.binary_search_by_key(&t, |m| m.0)
                {
                    *tex.add(i) = s.map[k].1;
                }
            }
        }
    }
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
            || translate_call(gps, "addst", key, space, false, main_layer, |b| orig(gps, &StdString::borrowed(b), just, space))
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
            || translate_call(gps, "addst_flag", key, space, sflag & (FLAG_TOP_OF_TEXT | FLAG_BOTTOM_OF_TEXT) != 0, main_layer,
                              |b| orig(gps, &StdString::borrowed(b), just, space, sflag))
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
            || translate_call(gps, "top_addst", key, 0, false, top_layer, |b| orig(gps, &StdString::borrowed(b), just, space))
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
        translate_call(gps, "addcoloredst", key, 0, false, main_layer, |b| {
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
    let dict = DICT.load(Ordering::Acquire);
    if dict.is_null() {
        return false;
    }
    let dict = unsafe { &*dict };
    let Some(plan) = (unsafe { crate::ctb::line_plan(s as usize, dict) }) else { return false };
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
    let Some(layout) = LAYOUT.get() else { return true };
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
    unsafe { set_width_then_rewrite(mtb, width, orig) };
}

/// 段落排版（遊戲的與 DFHack 的 MTB_set_width 共用）：orig 依寬度排完英文單字後，整段有譯文就訂好每個單字的畫字計畫
unsafe fn set_width_then_rewrite(mtb: *mut u8, width: i32, orig: SetWidthFn) {
    unsafe { orig(mtb, width) };
    let dict = DICT.load(Ordering::Acquire);
    if dict.is_null() || !ENABLED.load(Ordering::Relaxed) {
        return;
    }
    // 重訂連結段落的計畫前讓遊戲照英文重排：排版函式只在寬度和上次不同時才排，先把記下的寬度改成別的值
    let relayout = || unsafe {
        if let Some(l) = crate::mtb::MTB_LAYOUT.get() {
            *(mtb.add(l.current_width) as *mut i32) = !width;
            orig(mtb, width);
        }
    };
    unsafe { crate::mtb::rewrite(mtb as usize, &*dict, |key| note_miss("mtb", key), relayout) };
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
        crate::lines::plan(&lines[before.min(lines.len())..], &text, width, &*dict, false, |s| note_miss("wrap", s));
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

/// 搜尋框比對清單項目（std::string::find）：原函式找不到、要找的字是玩家在遊戲輸入框打的字時，照字典把項目翻成中文再比拼音
/// （見 pinyin.rs）；比中就回傳 0（遊戲只看找不找得到）。這個函式到處都用（音效路徑、狼人的種族名…），
/// 所以只比從頭找、要找的字在輸入框裡的，其他照原函式的結果
unsafe extern "C" fn hook_string_find(hay: *const u8, len: usize, pos: usize, needle: *const u8, nlen: usize) -> usize {
    let orig: FindFn = unsafe { std::mem::transmute(TRAMPOLINES[21].load(Ordering::Acquire)) };
    let found = unsafe { orig(hay, len, pos, needle, nlen) };
    if found != usize::MAX || pos != 0 || len == 0 || nlen == 0 || nlen > 64 {
        return found;
    }
    let needle = unsafe { std::slice::from_raw_parts(needle, nlen) };
    let dict = DICT.load(Ordering::Acquire);
    if dict.is_null() || !crate::typed::searching(needle) {
        return found;
    }
    let hay = unsafe { std::slice::from_raw_parts(hay, len) };
    if crate::pinyin::search(unsafe { &*dict }, hay, needle) { 0 } else { found }
}

/// 遊戲的輸入框打字（FUN_140c39700）：改完就記下框裡的字，搜尋框接著重新比對清單時認得出是玩家打的字（見 typed::searching）
unsafe extern "C" fn hook_text_input(s: *mut StdString, max: i32, flags: u64, keys: *mut u8, text: *const u8) -> u64 {
    let orig: TextInputFn = unsafe { std::mem::transmute(TRAMPOLINES[22].load(Ordering::Acquire)) };
    let changed = unsafe { orig(s, max, flags, keys, text) };
    if changed != 0 {
        crate::typed::set_live(unsafe { (*s).bytes() });
    }
    changed
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

/// 冒險模式的通知清單（按鍵 A_ANNOUNCEMENTS 開的整份通知）：同上，行清單在 +8（FUN_140864970 依寬 85 切行）。
unsafe extern "C" fn hook_adv_announcements(panel: *mut u8, a: u64, left: i32, right: i32) {
    let orig: AnnouncementsFn = unsafe { std::mem::transmute(TRAMPOLINES[16].load(Ordering::Acquire)) };
    unsafe {
        plan_report_lines(panel as usize + 8);
        orig(panel, a, left, right);
    }
}

/// 工坊「新增任務」的提示框（FUN_1408c2ca0：「Produces:」產出、「Requires:」需要）：遊戲把需要的每個條件照 29 格自己切行，
/// 切在空白處時空白留在行尾；前 30 字沒有空白的長條件從字中間切開（「C」「alcite/limestone/chalk/marbl 」「e boulders」）。
/// 原函式跑完後把「Requires:」底下切開的幾行依字典接回整句（見 tool_tip_groups），替兩行以上的一組登記畫字計畫；單獨一行的照舊由畫字 hook 翻。
/// 中文比原文少用的行（畫空白）移到清單最後，不夾在兩個條件之間（「方解石／石灰岩／白堊／大理石石塊」下面空兩行才是「空的袋子」）
unsafe extern "C" fn hook_tool_tip(job: *mut u8, out: *mut u8) {
    let orig: ToolTipFn = unsafe { std::mem::transmute(TRAMPOLINES[17].load(Ordering::Acquire)) };
    unsafe { orig(job, out) };
    let dict = DICT.load(Ordering::Acquire);
    if dict.is_null() || !ENABLED.load(Ordering::Relaxed) {
        return;
    }
    let dict = unsafe { &*dict };
    let items = unsafe { crate::lines::vec_items(out as usize) };
    let texts: Vec<&[u8]> = items.iter().map(|&s| unsafe { crate::lines::str_bytes(s) }).collect();
    let Some(start) = texts.iter().position(|t| *t == b"Requires:").map(|k| k + 1) else { return };
    let end = texts[start..].iter().position(|t| t.is_empty()).map_or(texts.len(), |k| start + k);
    for (a, b, text) in tool_tip_groups(&texts[start..end], |t| dict.translate_sentences(t).0.is_some()) {
        let group = &items[start + a..start + b];
        if group.len() > 1 && !unsafe { crate::lines::planned(group) } {
            unsafe { crate::lines::plan(group, &text, 29, dict, false, |s| note_miss("wrap", s)) };
        }
    }
    unsafe { crate::lines::blanks_last(out as usize) };
}

/// 提示框「需要：」底下的條件行分組（見 hook_tool_tip）：從每一行起往後試接成一句（最多 6 行），行尾是空白的行還沒完、
/// 字從中間切開的第一行（只剩一個字、下一行 29 字以遊戲插進去的空白結尾）也還沒完；接起來 found（字典查得到）就成一組，
/// 字從中間切開的那組另試去掉插進去的空白。都查不到時這一行自成一組。回傳 (起行, 迄行（不含）, 接回的原文)
fn tool_tip_groups(lines: &[&[u8]], found: impl Fn(&[u8]) -> bool) -> Vec<(usize, usize, Vec<u8>)> {
    let cut_word = |k: usize| lines[k].len() == 1 && lines.get(k + 1).is_some_and(|l| l.len() == 29 && l.ends_with(b" "));
    let mut out = Vec::new();
    let mut k = 0;
    while k < lines.len() {
        let mut hit = None;
        for m in k..lines.len().min(k + 6) {
            if (lines[m].ends_with(b" ") && m + 1 < lines.len()) || (m == k && cut_word(k)) {
                continue;
            }
            let text = lines[k..=m].concat();
            if found(&text) {
                hit = Some((m, text));
                break;
            }
            if cut_word(k) {
                let mut fixed = lines[k].to_vec();
                fixed.extend_from_slice(&lines[k + 1][..28]);
                for line in &lines[k + 2..=m] {
                    fixed.extend_from_slice(line);
                }
                if found(&fixed) {
                    hit = Some((m, fixed));
                    break;
                }
            }
        }
        let (m, text) = hit.unwrap_or_else(|| (k, lines[k].to_vec()));
        out.push((k, m + 1, text));
        k = m + 1;
    }
    out
}

/// 世界畫面（FUN_1401c17e0）上遊戲自己切行的兩份文字（內嵌的切行迴圈，wrap_lines 的 hook 攔不到），一行行用 addst 畫：
/// 游標停在據點時的新聞與傳聞（畫面 +0x8310 `rumor_text`，FUN_1401da280 每則照 53 格切行，則與則之間空一行），
/// 與派遣報告的內文（+0x8388 `mission_text_box`，每行的顏色在 +0x83a0，同一則事件同色）。畫之前把同一則的幾行接回整句登記畫字計畫；
/// 派遣報告沒有空行分隔：相鄰兩行同色、而且下一行的第一個字放不進上一行（遊戲是被迫換行），才算同一則。
/// ponytail: 位移照 53.16 寫死；相鄰兩則同色、上一則最後一行又剛好很滿時會接成一句，翻譯照逐句不受影響，只是兩則的分行不見
unsafe extern "C" fn hook_world_site_history(vs: *mut u8) {
    let orig: ScreenFn = unsafe { std::mem::transmute(TRAMPOLINES[18].load(Ordering::Acquire)) };
    unsafe {
        plan_site_history(vs as usize);
        orig(vs);
    }
}

unsafe fn plan_site_history(vs: usize) {
    let dict = DICT.load(Ordering::Acquire);
    if dict.is_null() || !ENABLED.load(Ordering::Relaxed) {
        return;
    }
    let dict = unsafe { &*dict };
    let rumors = unsafe { crate::lines::vec_items(vs + 0x8310) };
    for group in rumors.split(|&s| unsafe { crate::lines::str_bytes(s) }.is_empty()) {
        if group.len() > 1 && !unsafe { crate::lines::planned(group) } {
            unsafe { crate::lines::plan(group, &crate::lines::join(group), 53, dict, false, |s| note_miss("wrap", s)) };
        }
    }
    unsafe { crate::lines::blanks_last(vs + 0x8310) };
    let lines = unsafe { crate::lines::vec_items(vs + 0x8388) };
    let colors: &[i32] = unsafe {
        let (begin, end): (usize, usize) = (read(vs as *const u8, 0x83a0), read(vs as *const u8, 0x83a8));
        if begin == 0 || end < begin || (end - begin) / 4 != lines.len() {
            return;
        }
        std::slice::from_raw_parts(begin as *const i32, lines.len())
    };
    let texts: Vec<&[u8]> = lines.iter().map(|&s| unsafe { crate::lines::str_bytes(s) }).collect();
    for (a, b) in history_events(&texts, colors, 53) {
        let group = &lines[a..b];
        if group.len() > 1 && !unsafe { crate::lines::planned(group) } {
            unsafe { crate::lines::plan(group, &crate::lines::join(group), 53, dict, false, |s| note_miss("wrap", s)) };
        }
    }
}

/// 據點歷史的行依事件分組（見 hook_world_site_history）：(起行, 迄行（不含）)。遊戲切行時把切點的空白留在上一行行尾，
/// 比的時候先去掉
fn history_events(lines: &[&[u8]], colors: &[i32], width: usize) -> Vec<(usize, usize)> {
    let forced = |prev: &[u8], next: &[u8]| prev.trim_ascii_end().len() + 1 + next.split(|&c| c == b' ').next().map_or(0, <[u8]>::len) > width;
    let mut out = Vec::new();
    let mut i = 0;
    while i < lines.len() {
        let mut j = i + 1;
        while j < lines.len() && colors[j] == colors[i] && !lines[j].is_empty() && forced(lines[j - 1], lines[j]) {
            j += 1;
        }
        out.push((i, j));
        i = j;
    }
    out
}

/// 交易畫面交出或被奪神器時商人說的話（FUN_14036a9c0）：遊戲照 68 格自己切行，接在交易畫面 +0x3c0 的行清單後面
/// （清單原本有字時先接一行空行）。原函式跑完後把新加的幾行接回整句登記畫字計畫
unsafe extern "C" fn hook_trade_artifact_speech(trade: *mut u8, item: usize, civ_a: usize, civ_b: usize, give: u8, seize: u8) {
    let orig: TradeSpeechFn = unsafe { std::mem::transmute(TRAMPOLINES[19].load(Ordering::Acquire)) };
    let list = trade as usize + 0x3c0;
    let before = unsafe { crate::lines::vec_len(list) };
    unsafe { orig(trade, item, civ_a, civ_b, give, seize) };
    let dict = DICT.load(Ordering::Acquire);
    if dict.is_null() || !ENABLED.load(Ordering::Relaxed) {
        return;
    }
    unsafe {
        let items = crate::lines::vec_items(list);
        let mut added = &items[before.min(items.len())..];
        if let [first, rest @ ..] = added {
            if crate::lines::str_bytes(*first).is_empty() {
                added = rest;
            }
        }
        if added.len() > 1 && !crate::lines::planned(added) {
            crate::lines::plan(added, &crate::lines::join(added), 68, &*dict, false, |s| note_miss("wrap", s));
        }
    }
}

/// DFHack 的段落排版（dfhack.dll 匯出的 Gui::MTB_set_width）：dfhack.gui.showPopupAnnouncement 開的彈出視窗用 DFHack 自己的
/// 排版、不經遊戲的 mtb_set_width；參數與段落結構和遊戲的相同，照樣訂畫字計畫（見 set_width_then_rewrite）
unsafe extern "C" fn hook_dfhack_mtb_set_width(mtb: *mut u8, width: i32) {
    let orig: SetWidthFn = unsafe { std::mem::transmute(TRAMPOLINES[20].load(Ordering::Acquire)) };
    unsafe { set_width_then_rewrite(mtb, width, orig) };
}

/// 報告切成的行清單（vector<行*>）：每行開頭是那一行的 std::string，+0x20 指回報告，報告 +8 是整則原文。
/// 同一則報告的幾行連在一起；只有一行的報告，行字串是空的、畫的是報告本身的全文。整則（「講者: 內容」）查字典，
/// 查不到就逐句查，中文以這幾行最長的一行為寬重新換行（見 lines.rs）。
/// ponytail: 位移照 53.16 的 FUN_1408020c0（訊息框 +0x43d0）、FUN_140867e10（交談畫面 +0xb0）、FUN_140883f40（通知清單 +8）寫死；
/// 改版時特徵碼對不上就不攔，只有這幾個框維持英文。
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
                unsafe { crate::lines::plan(lines, text, width, dict, false, |s| note_miss("wrap", s)) };
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
            crate::lines::plan(&lines[..n], &crate::lines::join(&lines[..n]), width, &*dict, false, |s| note_miss("wrap", s));
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

/// 名字轉原文：原函式照跑、原文照用；記下這個原名（名字音譯關著也記，打開選項就查得到音譯，見 names::note_native）。
/// 沒有原生語言的名字（language_name +0x6c 是 -1）只記單一個字、又不是這個世界書名的（狗頭人的名字「Chrakadogus」）：
/// 書冊、卷軸類神器也沒有語言，「原文」就是英文書名，記了以後書名的第一個字（The、Fortress）也算原名，句型的片段會照抄
/// （「哥布林Fortress」，見 lua/loader.lua 的 note_world_names）。
/// ponytail: 讀檔登記書名（set_titles）之前遊戲就組了單字書名神器的原文時，那個書名會記成原名；目前沒看到這種時序
unsafe extern "C" fn hook_native_name(name: *const u8, out: *mut StdString) {
    let orig: NativeNameFn = unsafe { std::mem::transmute(TRAMPOLINES[15].load(Ordering::Acquire)) };
    unsafe { orig(name, out) };
    if !ENABLED.load(Ordering::Relaxed) {
        return;
    }
    let native = unsafe { (*out).bytes() };
    if unsafe { read::<i32>(name, 0x6c) } >= 0 || (!native.contains(&b' ') && !crate::names::is_title(native)) {
        crate::names::note_native(native);
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
        Some(r) => match entry.glyphs.iter().find(|&&(off, g)| (off..off + glyph_cells(g)).contains(&r)) {
            Some(&(off, g)) => dict.tile(g, r - off, Row::Label).unwrap_or_else(|| font(shadow[r])),
            None => font(shadow[r]),
        },
        None => font(b' '),
    };
    Some((0..n).map(tile).collect())
}

#[cfg(test)]
mod tests {
    use super::{button_row, dfhack_place, history_events, label_tiles, lookup, placed_align, tool_tip_groups, wrap_plan, Columns, MissLog, CUTS, MISS_BYTES, MISS_LIMIT};
    use crate::dict::Dict;

    #[test]
    fn a_string_cut_between_words_translates_the_words_before_the_cut() {
        // 冒險同伴清單的名字欄寫死 33 格，遊戲直接改長度截短、沒經過 resize（「Zebna Histekuthros, Donkey, ♂ ...」）
        let mut d = Dict::default();
        let chars = "斑驢…";
        assert!(d.set_glyphs(chars, vec![0; chars.chars().count() * 2]));
        d.load_tsv("Zebna, Donkey\t斑驢\n");
        let text = |k: &[u8]| lookup(&d, k, 0).map(|f| f.text.clone());
        assert_eq!(text(b"Zebna, Donkey ...").as_deref(), Some("斑驢…"), "截在字與字之間：翻前面那段，接「…」");
        assert_eq!(text(b"Zebna, Mule ..."), None, "前面那段翻不出來照英文");
        assert_eq!(text(b"Zebna, Mu..."), None, "截在字中間不猜");
    }

    #[test]
    fn a_line_cut_by_the_game_keeps_the_space_at_the_cut_and_still_translates() {
        // 工坊任務提示框「需要：」：遊戲照 29 格自己切行，切點的一個空白留在行尾（「Non-economic hard stone 」、下一行「boulders」）
        let mut d = Dict::default();
        let chars = "非經濟堅硬石頭";
        assert!(d.set_glyphs(chars, vec![0; chars.chars().count() * 2]));
        d.load_tsv("Non-economic hard stone\t非經濟堅硬石頭\n");
        let text = |k: &[u8]| lookup(&d, k, 0).map(|f| f.text.clone());
        assert_eq!(text(b"Non-economic hard stone ").as_deref(), Some("非經濟堅硬石頭"));
        assert_eq!(text(b"Non-economic hard stone  "), None, "兩格以上的空白是欄位補位，不當成切點");
    }

    #[test]
    fn rewriting_the_key_stops_when_the_original_leads_back_to_it() {
        // 截短前的原文本身又以「 ...」或「... 」結尾：找原文 → 去掉結尾 → 又找到同一筆原文，會繞回來；層數有上限，不會把堆疊用完
        let d = Dict::default();
        CUTS.with_borrow_mut(|cuts| cuts.extend([b"Abc ... ...".to_vec(), b"Xyz... ".to_vec()]));
        assert!(lookup(&d, b"Abc ...", 0).is_none());
        assert!(lookup(&d, b"Xyz...", 0).is_none());
    }

    #[test]
    fn tool_tip_conditions_cut_by_the_game_are_joined_back() {
        // 工坊提示框「需要：」：遊戲照 29 格切行；前 30 字沒有空白的字從中間切開，還插進一個空白
        let known: &[&[u8]] = &[b"Calcite/limestone/chalk/marble boulders", b"Non-economic hard stone boulders", b"Bone", b"Bone blocks", b"Wood log"];
        let found = |t: &[u8]| known.contains(&t);
        let groups = |lines: &[&[u8]]| tool_tip_groups(lines, found).into_iter().map(|(a, b, t)| (a, b, String::from_utf8(t).unwrap())).collect::<Vec<_>>();
        assert_eq!(groups(&[b"Wood log", b"C", b"alcite/limestone/chalk/marbl ", b"e boulders"]),
                   [(0, 1, "Wood log".into()), (1, 4, "Calcite/limestone/chalk/marble boulders".into())], "去掉遊戲插進去的空白");
        assert_eq!(groups(&[b"Non-economic hard ", b"stone boulders"]), [(0, 2, "Non-economic hard stone boulders".into())]);
        assert_eq!(groups(&[b"Bone ", b"blocks"]), [(0, 2, "Bone blocks".into())], "行尾是空白的行還沒完，短的 Bone 不先命中");
        assert_eq!(groups(&[b"Abc", b"def"]), [(0, 1, "Abc".into()), (1, 2, "def".into())], "查不到就一行一組");
    }

    #[test]
    fn site_history_lines_join_only_where_the_game_was_forced_to_wrap() {
        // 據點歷史寬 10：「aaaa bbbb」後面接的「cccc」放不下才換行（同一則）；「dd」放得下卻換行，是下一則
        let lines: &[&[u8]] = &[b"aaaa bbbb", b"cccc", b"dd ee", b"ff"];
        assert_eq!(history_events(lines, &[7, 7, 7, 7], 10), [(0, 2), (2, 3), (3, 4)]);
        assert_eq!(history_events(lines, &[7, 3, 3, 3], 10), [(0, 1), (1, 2), (2, 3), (3, 4)], "顏色不同就是不同則");
        assert_eq!(history_events(&[b"aaaa bbb ", b"c"], &[7, 7], 10), [(0, 1), (1, 2)], "行尾留著切點的空白：去掉再比，c 放得下就是下一則");
    }

    #[test]
    fn dfhack_names_narrowed_by_font_width_are_drawn_by_the_overlay() {
        // 整句貼圖開著：「幽默感不太在意」照字寬 10 格（一字兩格要 14 格）。DFHack 畫「 Abcdefghij 」（開頭一格空白）時，
        // 字要交給整句貼圖畫在原文那 10 格，不能只寫一整段空白替身
        crate::metrics::test_compact();
        let mut d = Dict::default();
        let chars = "他很有幽默感。不太在意";
        assert!(d.set_glyphs(chars, vec![0; chars.chars().count() * 8 + 1]));
        let entry = d.para_entry("幽默感不太在意").expect("compact entry");
        let p = dfhack_place(&entry, 12, 1, 10, false).expect("fits");
        assert_eq!(p.shadow, b"            ");
        assert_eq!(p.compact, Some((1, 10, "幽默感不太在意".to_owned())));
        assert!(dfhack_place(&entry, 10, 1, 8, false).is_none(), "放不進原文的格子照英文");
    }

    #[test]
    fn a_big_button_splits_the_translation_evenly_over_its_rows_and_centers_each_row() {
        // civ-alert 的大按鈕：「 Activate 」「 civilian 」「  alert   」三列各 10 格，查整句「Activate civilian alert」
        let mut d = Dict::default();
        let chars = "發布平民警報繼續";
        assert!(d.set_glyphs(chars, vec![0; chars.chars().count() * 2]));
        d.load_tsv("Activate civilian alert\t發布平民警報\nContinue\t繼續\n");
        let cells = |phrase: &[u8], row, rows, width| {
            button_row(&d, phrase, row, rows, width, 0).map(|p| (p.shadow.len(), p.glyphs.iter().map(|g| g.0).collect::<Vec<_>>()))
        };
        for row in 0..3 {
            assert_eq!(cells(b"Activate civilian alert", row, 3, 10), Some((10, vec![3, 5])), "每列分到兩個字，放在 10 格正中間");
        }
        assert_eq!(cells(b"Continue", 0, 1, 15), Some((15, vec![5, 7])));
        assert_eq!(cells(b"Continue", 2, 3, 15), Some((15, vec![])), "分不到字的列畫空白");
        assert_eq!(cells(b"Clear civilian alert", 0, 3, 10), None, "整句翻不出來照英文");
    }

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
        let mut c = Columns::default();
        // 工作分組：按鈕中段 8–33 格，字都從第 13 格開始；選取中的那一項按鈕貼圖不同，左緣一樣在第 8 格
        let item = |x: i32| if (8..=33).contains(&x) { 78 } else { 0 };
        let selected = |x: i32| if (8..=33).contains(&x) { 92 } else { 0 };
        assert!(!c.note(item, false, false, 13, 6), "清單第一項還看不出來");
        assert!(c.note(selected, false, false, 13, 15), "同一欄（按鈕左緣、起點都一樣）、長度差兩格以上；選取中的按鈕貼圖不同也算");
        let keys = |x: i32| if (38..=60).contains(&x) { 90 } else { 0 };
        assert!(!c.note(keys, false, false, 40, 14));
        assert!(c.note(keys, false, false, 40, 12), "按鍵設定的「Delayed repeat」與「Don't repeat」差兩格");
        let menu = |x: i32| if (76..=114).contains(&x) { 814 } else { 0 };
        assert!(!c.note(menu, false, false, 91, 8));
        assert!(!c.note(menu, false, false, 91, 7), "只差一格：置中的按鈕整數除剛好同一格（Fortress、Legends）");
        assert!(!c.note(|_| 0, false, false, 13, 30), "不在按鈕上不記");
        let narrow = |x: i32| if (8..=20).contains(&x) { 78 } else { 0 };
        assert!(!c.note(narrow, false, false, 13, 15), "左緣、起點相同但按鈕寬度不同的是別種框");
        c.next_frame();
        assert!(c.note(item, false, false, 13, 6), "下一幀先畫第一項：後面幾項這一幀還沒畫到，看上一幀的");
    }

    #[test]
    fn only_texts_on_screen_now_make_a_list() {
        let mut c = Columns::default();
        // 人物頁的「想法」分頁：半格位移的兩列，中段 129–135 格，Thoughts 8 格從第 129 格起
        let tab = |x: i32| if (129..=135).contains(&x) { 126523 } else { 0 };
        assert!(!c.note(tab, false, true, 129, 8));
        assert!(!c.note(tab, false, false, 129, 2), "同一位置的一般列（清單、按鈕）和分頁標籤不算同一欄");
        assert!(!c.note(tab, true, true, 129, 2), "頂層（提示框、說明視窗）也不算");
        assert!(c.note(tab, false, true, 129, 2), "同一種字同一幀長短差兩格是清單");
        c.next_frame();
        c.next_frame();
        assert!(!c.note(tab, false, true, 129, 8), "以前別的畫面在同一位置畫過長度不同的字，不影響現在的分頁標籤");
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
