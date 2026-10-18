//! 整句貼圖：畫面上的字改成一行一張貼圖，用本包的字型畫，不受「一個中文字佔兩格」與遊戲點陣字限制。
//! 預設開啟；設定視窗或 dflang overlay off 改回一字兩格的字圖（lua/dflang.lua），隨時可切。
//!
//! DF 53.16 每幀由非同步執行緒在 render_things 裡把字寫進畫面緩衝區，主執行緒再用 renderer 的 vtable 逐格畫
//! （update_all → 每格 update_tile → 錨定貼圖 → 頂層開著時每格 update_top_tile），兩邊交棒、不會同時跑；
//! 文字格每幀整個重畫，沒有只畫變動格。做法：
//! - 寫字時（hooks.rs）照舊：有譯文的寫替身字串、貼字圖，沒有譯文的照原樣畫；兩種都記下這一幀寫在哪裡（record）。
//! - 主執行緒 update_all 開始時逐格核對：格子還是這次寫的內容（字圖的 texpos，或同一個字元而且沒有貼圖）才算數；
//!   畫這些格時先把字元換成空白、texpos 清掉，遊戲只畫底色，畫完再還原。
//! - 主層畫完（頂層開始前，或 update_all 結束）、頂層畫完，各把每一行的貼圖照「格子還算數」切段貼上。
//!
//! 一行怎麼排：
//! - 譯文：依字型的字寬緊排，依對齊置中、靠右，或從第一個非空白格起。對齊照遊戲實際把原文擺在哪裡（hooks.rs 的
//!   placed_align：按鈕上置中、畫面正中間），半格位移的列（分頁標籤）也一樣；錨定貼圖標籤一律置中。
//! - 遊戲接著游標連續寫的幾段（「範圍：」＋「114」＋「至」＋「768」、「v」＋「53.16」）、只隔一格空白的幾段（DFHack 的
//!   「標籤」＋「值」、傳說的「黑暗」＋「要塞」、段落照英文逐字畫的字），或下一段從譯文原文的結尾畫起的（遊戲照英文字寬排好
//!   每個字：選擇出發地點的金屬清單）接成一行緊排，每段用自己的顏色；隔一格的兩段都只有數字時不接（頂部資訊列的心情人數各自
//!   對準一個表情）。段落文字和緊貼在旁邊的連結字也接成一行。
//! - 段落照字型的實際字寬換行（metrics.rs）：格子貼透明記號，字照段落文字從第一格緊排。比格子寬的一行（換了格子大小後的段落、
//!   比原文寬一點的名字與按鍵名）壓成長體放進格子：字距與字形本身一起橫向縮放（見 State::strip）。
//! - 其他沒有譯文的字：單一空白隔開的字詞接成一組，從原本的格子畫起（數字對齊原本的結尾，數值框裡靠左的照起始格），
//!   彩色文字每個字留在原格。
//! - 方框、箭頭、品質記號這類圖形字元照遊戲原樣畫（見 is_text），兩旁的字從自己原本的格子畫起。
//! - 只有一列高的按鈕（排序標題）上下緣線畫在字的同一格裡，字改小一號；框的邊緣列照預設大小（見 size_for）。

use std::cell::RefCell;
use std::collections::HashMap;
use std::ffi::c_void;
use std::hash::{Hash, Hasher};
use std::sync::atomic::{AtomicBool, AtomicU8, AtomicU64, AtomicUsize, Ordering};
use std::sync::{Arc, OnceLock};

use parking_lot::{Mutex, RwLock};

use crate::atlas::{self, Glyphs};
use crate::dict::Row;
use crate::hooks::{DICT, LAYOUT};

pub static ON: AtomicBool = AtomicBool::new(false);
/// 整句貼圖用的字型（lib.rs 的 set_typeface 換：遊戲啟動時、換字重或語言時）
static TYPEFACE: RwLock<Option<Arc<Vec<atlas::Font>>>> = RwLock::new(None);
/// 字型、字的大小或字典（語言）換過幾次：主執行緒看到變了，就丟掉照舊樣子畫的點陣與貼圖（見 State::ready）
static STYLE: AtomicU64 = AtomicU64::new(0);
static GPS: AtomicUsize = AtomicUsize::new(0);

/// 字身相對格高（十六分之幾）、往上提的比例。遊戲的點陣字（16 像素高的格子）字頂在第 2 列、基線在第 12 列，按鈕與方框的上下緣線畫在
/// 格子最上、最下兩三列。上提一像素時，16 像素的格子裡中文筆畫（hinting 後，覆蓋率 > 1/4）在 13 是第 2～14 列（相鄰兩列的字之間空 3 列），
/// 14 是第 1～14 列（空 2 列、字底不變）、15 是 1～15（空 1 列）、16 是 0～15（兩列相黏）。預設 14：比 13 大、字底不會多壓到下緣線。
/// 玩家可用設定視窗或 dflang overlay size 改成 13～16（set_em，隨時可改）；框的邊緣列照 13 畫（SNUG_EM，見 size_for）。
/// 段落量字寬也用這個大小（lib.rs set_typeface → atlas::advances）
pub const DEFAULT_EM: u8 = 14;
/// 實測緣線畫在字的同一格裡也不壓線的大小（冒險角色名字框的兩列、存檔清單排序標題、出發準備分頁），也是設定的下限
const SNUG_EM: u8 = 13;
static EM: AtomicU8 = AtomicU8::new(DEFAULT_EM);
const RAISE_PER_CELL: f32 = 1.0 / 16.0;
/// 只有一列高的按鈕（排序標題這類，見 size_for）：上下緣線畫在字的同一格裡，字改小一號（和錨定貼圖標籤一樣）放在兩條線中間
const SMALL_EM_PER_CELL: f32 = 0.75;
const SMALL_RAISE_PER_CELL: f32 = 1.0 / 16.0;
/// 字的三種大小（State::glyphs 的索引）：玩家設定的大小、小一號、框的邊緣列（SNUG_EM，見 size_for）
const NORMAL: usize = 0;
const SMALL: usize = 1;
const SNUG: usize = 2;

/// 設定字身（十六分之幾格高，13～16）；超出範圍的不算。段落的字寬要另外重量（lib.rs 的 set_typeface）
pub fn set_em(sixteenths: u8) -> bool {
    let ok = (SNUG_EM..=16).contains(&sixteenths);
    if ok && EM.swap(sixteenths, Ordering::Relaxed) != sixteenths {
        restyle();
    }
    ok
}

pub fn em_per_cell() -> f32 {
    f32::from(EM.load(Ordering::Relaxed)) / 16.0
}

/// 換字型（字重或語言換了）
pub fn set_typeface(fonts: Arc<Vec<atlas::Font>>) {
    *TYPEFACE.write() = Some(fonts);
    restyle();
}

/// 字的樣子變了（字型、字的大小、字典的字圖編號）：主執行緒下一次畫字前丟掉快取（見 State::ready）
pub fn restyle() {
    STYLE.fetch_add(1, Ordering::Release);
}

fn per_cell(size: usize) -> f32 {
    [em_per_cell(), SMALL_EM_PER_CELL, f32::from(SNUG_EM) / 16.0][size]
}

/// 一行用哪種大小。rows 是字底下的框在上一列、下一列還有沒有同一組的貼圖（State::box_rows；字底下沒有框是 None），
/// grow 是字身比 13（SNUG_EM）大（預設 14 就是）：
/// - 只有一列高的按鈕（上下都不是同一組：排序標題這類）：上下緣線畫在字的同一格裡，小一號
/// - 框的最上、最下一列（冒險角色名字框的兩列）、錨定貼圖標籤：緣線畫在字的同一格裡、標籤從固定的上緣畫，照實測不壓線的 13
/// - 其他（框的中間列、沒有框、半格位移的分頁標籤）：玩家設定的大小
fn size_for(row: Row, rows: Option<(bool, bool)>, grow: bool) -> usize {
    match (row, rows) {
        (Row::Whole, Some((false, false))) => SMALL,
        (Row::Label, _) | (Row::Whole, Some((true, false) | (false, true))) if grow => SNUG,
        _ => NORMAL,
    }
}

/// 自己建的一行貼圖上限，滿了丟掉最久沒用的
const CACHE_LIMIT: usize = 2048;
/// 目前快取的貼圖張數（status 用；快取在主執行緒的 thread_local 裡，別的執行緒看不到）
static TEXTURES: AtomicUsize = AtomicUsize::new(0);

pub fn texture_count() -> usize {
    TEXTURES.load(Ordering::Relaxed)
}

/// 交給整句貼圖畫的一般字元：可見 ASCII、CP437 的拉丁字母（矮人名字的 ë、ü…）、♂♀ 與譯名的間隔號 ·（names.rs）；
/// 方框、箭頭、☼ 這類圖形字元與 ¢£¥₧ƒ 照遊戲原樣畫。字型子集要有這些字（scripts/build.py）。
pub fn is_text(b: u8) -> bool {
    (0x20..=0x7e).contains(&b) || (0x80..=0x9a).contains(&b) || (0xa0..=0xa5).contains(&b) || matches!(b, 0x0b | 0x0c | 0xfa)
}

/// 全形標點（「：」「，」「。」「）」…；簡中的引號“”‘’在中文字型裡也是全形）
fn full_width_punctuation(c: char) -> bool {
    matches!(c, '\u{2018}'..='\u{201f}' | '\u{3000}'..='\u{303f}' | '\u{ff01}'..='\u{ff0f}' | '\u{ff1a}'..='\u{ff20}' | '\u{ff3b}'..='\u{ff40}' | '\u{ff5b}'..='\u{ff65}')
}

/// 中文字（含全形標點）：CJK 區段以後的字
fn wide(c: char) -> bool {
    c >= '\u{2e80}'
}

/// 接成一行時兩段中間的一個空白不留：前面是全形標點（本身右邊就有空），或前後都是中文字（中文詞之間不空格）
fn drop_space(before: Option<char>, after: Option<char>) -> bool {
    before.is_some_and(full_width_punctuation) || (before.is_some_and(wide) && after.is_some_and(wide))
}

/// 全形的前括號、前引號（筆畫在字身右半）
fn open_bracket(c: char) -> bool {
    matches!(c, '（' | '「' | '『' | '【' | '〔' | '〈' | '《' | '［' | '｛' | '“' | '‘')
}

/// 全形的後括號、後引號（筆畫在字身左半）
fn close_bracket(c: char) -> bool {
    matches!(c, '）' | '」' | '』' | '】' | '〕' | '〉' | '》' | '］' | '｝' | '”' | '’')
}

/// 本幀第 i 段：遊戲寫字時記下的（base），或從格子讀回來接在旁邊的（extra，索引接在 base 後面）
fn run_at<'a>(base: &'a [Run], extra: &'a [Run], i: usize) -> &'a Run {
    base.get(i).unwrap_or_else(|| &extra[i - base.len()])
}

struct Run {
    top: bool,
    row: Row,
    x0: i32,
    y: i32,
    shadow: Vec<u8>,
    /// 一字兩格的譯文字圖；沒有譯文的字和照字寬縮短的段落（text）是空的
    glyphs: Vec<(usize, u32)>,
    align: u8,
    /// 結尾有幾格是補到原文寬度的空白（不是譯文本身的空白）：接成一行緊排時不佔位
    pad: usize,
    /// 原文比寫下的替身字串長出來的格數（靠左的譯文比英文短，遊戲只寫了譯文那幾格；見 hooks.rs 的 Paint::tail）：
    /// 遊戲照英文字寬排位置時，下一段從原文的結尾畫起（見 chainable），接成一行時這幾格和 pad 一樣不佔位
    tail: usize,
    /// 照字型實際字寬縮短的段落（Entry::compact）：格子貼的是透明記號，字照這段文字從第一格緊排；
    /// 錨定貼圖標籤（anchor）也照這段文字畫
    text: Option<String>,
    /// 錨定貼圖標籤（row 是 Row::Label）：每格換上的貼圖編號、遊戲畫這張貼圖時往下位移的像素
    anchor: Option<(Vec<i32>, i32)>,
    /// DFHack 畫的字（位置由 DFHack 的元件算好，一律從起點靠左，見 right_in_box）
    dfhack: bool,
}

impl Run {
    fn plain(&self) -> bool {
        self.glyphs.is_empty() && self.text.is_none()
    }

    fn end(&self) -> i32 {
        self.x0 + self.shadow.len() as i32
    }

    /// 每個字：(起始格, 字)。句型拼出的譯文字圖不一定依格位排序，先依格位建表；縮短的段落每個字都從第一格起排
    fn items(&self, chars: &[char]) -> Vec<(usize, char)> {
        if let Some(text) = &self.text {
            return text.chars().map(|c| (0, c)).collect();
        }
        let mut at = vec![None; self.shadow.len()];
        for &(off, g) in &self.glyphs {
            if let Some(slot) = at.get_mut(off) {
                *slot = Some(g);
            }
        }
        let mut items = Vec::new();
        let mut k = 0;
        while k < self.shadow.len() {
            if let Some(g) = at[k] {
                items.push((k, chars.get(g as usize).copied().unwrap_or('?')));
                k += 2;
            } else {
                items.push((k, crate::cp437::TABLE[self.shadow[k] as usize]));
                k += 1;
            }
        }
        items
    }

    /// 結尾補位的空白有幾格（字圖佔的兩格、縮短段落的記號格不算）
    fn trailing_pad(&self) -> usize {
        if self.text.is_some() {
            return 0;
        }
        let covered = self.glyphs.iter().map(|&(off, _)| off + 2).max().unwrap_or(0);
        self.shadow.iter().rev().take_while(|&&c| c == b' ').count().min(self.shadow.len().saturating_sub(covered))
    }
}

static BUILD: Mutex<Vec<Run>> = Mutex::new(Vec::new());
static FRAME: Mutex<Option<Arc<Vec<Run>>>> = Mutex::new(None);

/// 寫字時呼叫（非同步執行緒）：一段字；top 是頂層，row 是一般列或半格位移的上下列，glyphs 空的是沒有譯文的字，
/// pad 是結尾補位的格數（見 Run::pad），tail 是原文比替身字串長出來的格數（見 Run::tail），
/// text 是照字寬縮短的段落（見 Run::text），dfhack 是 DFHack 畫的字。
#[allow(clippy::too_many_arguments)]
pub fn record(gps: *mut u8, top: bool, row: Row, x0: i32, y: i32, shadow: &[u8], glyphs: &[(usize, u32)], align: u8, pad: usize, tail: usize, text: Option<&str>, dfhack: bool) {
    // 翻譯關著時不記（整句貼圖不畫），免得 DFHack 照畫的字白白堆進這一幀
    if !ON.load(Ordering::Relaxed) || !crate::hooks::ENABLED.load(Ordering::Acquire) {
        return;
    }
    if glyphs.is_empty() && text.is_none() && !shadow.iter().any(|&b| b != b' ' && is_text(b)) {
        return;
    }
    GPS.store(gps as usize, Ordering::Relaxed);
    let (shadow, glyphs, text) = (shadow.to_vec(), glyphs.to_vec(), text.map(str::to_owned));
    BUILD.lock().push(Run { top, row, x0, y, shadow, glyphs, align, pad, tail, text, anchor: None, dfhack });
}

/// 錨定貼圖標籤（出發準備畫面的分頁，見 hooks.rs finish_label）換好貼圖時呼叫（非同步執行緒）：(x0, y) 起每格換上的貼圖、
/// 譯文，lift 是遊戲畫這張貼圖時往下位移的像素。分頁寬度照英文標籤算，譯文在標籤寬度裡置中。
pub fn record_label(gps: *mut u8, x0: i32, y: i32, tiles: &[i32], text: &str, lift: i32) {
    if !ON.load(Ordering::Relaxed) || !crate::hooks::ENABLED.load(Ordering::Acquire) {
        return;
    }
    GPS.store(gps as usize, Ordering::Relaxed);
    let shadow = vec![b' '; tiles.len()];
    let anchor = Some((tiles.to_vec(), lift));
    let run = Run { top: false, row: Row::Label, x0, y, shadow, glyphs: Vec::new(), align: b'c', pad: 0, tail: 0, text: Some(text.to_owned()), anchor, dfhack: false };
    BUILD.lock().push(run);
}

/// render_things 開始、結束（非同步執行緒）。沒跑 render_things 的幀畫面緩衝區沒變，沿用上一份。
pub fn frame_begin() {
    BUILD.lock().clear();
}

pub fn frame_end() {
    let runs = std::mem::take(&mut *BUILD.lock());
    // 握著 FRAME 再看開關：翻譯剛關掉時，關之前就開始的這一幀不能蓋掉 reset 放的空幀
    let mut frame = FRAME.lock();
    *frame = Some(Arc::new(if crate::hooks::ENABLED.load(Ordering::Acquire) { runs } else { Vec::new() }));
}

/// 翻譯關掉時呼叫（ENABLED 已設成 false 之後）：丟掉還沒收完的字，最後一幀換成空的（update_all 照空的一幀清掉格子的記號，
/// 遊戲照原樣畫）。之後 render_things 的 hook 照跑，frame_end 每幀都放空的一幀；再開時下一幀照常收。
pub fn reset() {
    *BUILD.lock() = Vec::new();
    *FRAME.lock() = Some(Arc::new(Vec::new()));
}

/// status 用：本幀收到幾段字、還沒收完的有幾段（翻譯關著時兩個都應該是 0）
pub fn run_counts() -> (usize, usize) {
    let frame = FRAME.lock().as_ref().map_or(0, |f| f.len());
    (frame, BUILD.lock().len())
}

/// 除錯：每幀的耗時（微秒，指數平均）：[整個 update_all, 其中整句貼圖自己花的（核對＋貼圖）, 整個 render_things（含畫字 hook）]
static TIMING: Mutex<[f64; 3]> = Mutex::new([0.0; 3]);

fn note_time(total: f64, ours: f64) {
    let mut t = TIMING.lock();
    t[0] = t[0] * 0.95 + total * 0.05;
    t[1] = t[1] * 0.95 + ours * 0.05;
}

/// render_things 跑完時呼叫（非同步執行緒）：翻譯關著時也量，對照得出畫字 hook 多花的時間
pub fn note_render(spent: std::time::Duration) {
    let mut t = TIMING.lock();
    t[2] = t[2] * 0.95 + spent.as_secs_f64() * 1e6 * 0.05;
}

/// 除錯：耗時，加上本幀記下的字（頂層, 列別, x0, y, 替身字串, 字圖數, 對齊, 補位格數, 能不能接在前一段後面, 照字寬縮短的段落文字）
pub fn dump() -> String {
    let t = *TIMING.lock();
    let head = format!("update_all {:.0} us, overlay {:.0} us, render_things {:.0} us\n", t[0], t[1], t[2]);
    let Some(runs) = FRAME.lock().clone() else { return head };
    head + &runs
        .iter()
        .enumerate()
        .map(|(i, r)| {
            let chain = i > 0 && chainable(&runs[i - 1], r);
            format!(
                "{}\t{:?}\t{}\t{}\t[{}]\t{}\t{}\t{}\t{}\t{}\n",
                r.top as u8, r.row, r.x0, r.y, crate::cp437::decode(&r.shadow), r.glyphs.len(), r.align as char, r.pad, if chain { "C" } else { "" },
                r.text.as_deref().unwrap_or("")
            )
        })
        .collect::<String>()
}

/// 除錯：本幀畫面上的字，一段一行「x,y 旗標 內容」（文字格，和 dev_game.py click 同單位、同順序），依列、起始格排序；
/// 中文從字圖換回字，沒翻的照英文。
/// 旗標 T 是頂層（彈出視窗）、D 是 DFHack 畫的字。半格位移的下列和上列是同一段字，不重複列。
/// 給 dev_game.py screen：確認走到哪個畫面、還有沒有英文時不必截圖。
pub fn screen_text(dict: &crate::dict::Dict) -> String {
    let Some(runs) = FRAME.lock().clone() else { return String::new() };
    let mut chars = vec![' '; dict.glyph_of.len()];
    for (&c, &g) in &dict.glyph_of {
        if let Some(slot) = chars.get_mut(g as usize) {
            *slot = c;
        }
    }
    let mut lines: Vec<(i32, i32, String)> = runs
        .iter()
        .filter(|r| r.row != Row::Bottom)
        .filter_map(|r| {
            let text: String = r.items(&chars).into_iter().map(|(_, c)| c).collect();
            let body = text.trim_start_matches(' ');
            let x = r.x0 + (text.len() - body.len()) as i32;
            let body = body.trim_end();
            let flags = format!("{}{}", if r.top { 'T' } else { ' ' }, if r.dfhack { 'D' } else { ' ' });
            (!body.is_empty()).then(|| (r.y, x, format!("{x:>3},{:<2} {flags} {body}\n", r.y)))
        })
        .collect();
    lines.sort_by_key(|&(y, x, _)| (y, x));
    lines.into_iter().map(|(_, _, line)| line).collect()
}

// ---- SDL ----

#[repr(C)]
struct Rect {
    x: i32,
    y: i32,
    w: i32,
    h: i32,
}

type Tex = *mut c_void;

struct Sdl {
    create: unsafe extern "C" fn(*mut c_void, u32, i32, i32, i32) -> Tex,
    update: unsafe extern "C" fn(Tex, *const Rect, *const c_void, i32) -> i32,
    blend: unsafe extern "C" fn(Tex, i32) -> i32,
    color: unsafe extern "C" fn(Tex, u8, u8, u8) -> i32,
    copy: unsafe extern "C" fn(*mut c_void, Tex, *const Rect, *const Rect) -> i32,
    destroy: unsafe extern "C" fn(Tex),
}

const PIXELFORMAT_ARGB8888: u32 = 0x1636_2004;
const BLENDMODE_BLEND: i32 = 1;

#[link(name = "kernel32")]
unsafe extern "system" {
    fn GetModuleHandleW(name: *const u16) -> *mut c_void;
    fn VirtualProtect(addr: *mut c_void, size: usize, prot: u32, old: *mut u32) -> i32;
}

fn sdl() -> Option<&'static Sdl> {
    static SDL: OnceLock<Option<Sdl>> = OnceLock::new();
    SDL.get_or_init(|| unsafe {
        let name: Vec<u16> = "SDL2.dll\0".encode_utf16().collect();
        let m = GetModuleHandleW(name.as_ptr()) as *const u8;
        if m.is_null() {
            return None;
        }
        let f = |n: &str| crate::lua::export(m, n);
        Some(Sdl {
            create: std::mem::transmute(f("SDL_CreateTexture")?),
            update: std::mem::transmute(f("SDL_UpdateTexture")?),
            blend: std::mem::transmute(f("SDL_SetTextureBlendMode")?),
            color: std::mem::transmute(f("SDL_SetTextureColorMod")?),
            copy: std::mem::transmute(f("SDL_RenderCopy")?),
            destroy: std::mem::transmute(f("SDL_DestroyTexture")?),
        })
    })
    .as_ref()
}

// ---- renderer vtable ----

type TileFn = unsafe extern "C" fn(*mut u8, i32, i32);
type AllFn = unsafe extern "C" fn(*mut u8);
/// renderer_2d 的 vtable 槽：0 update_tile、2 update_top_tile、6 update_all、1 update_anchor_tile（df-structures；53.16 實測位址相符）
const SLOTS: [usize; 4] = [0, 2, 6, 1];
static ORIG: [AtomicUsize; 4] = [const { AtomicUsize::new(0) }; 4];

/// 換掉 renderer vtable 的四個槽。改指標是一次對齊的 8 位元組寫入，主執行緒正在畫也不會讀到一半。
pub unsafe fn install(renderer: usize) -> Result<(), String> {
    if sdl().is_none() {
        return Err("SDL2.dll 找不到需要的函式".into());
    }
    let hooks = [
        hook_update_tile as *const () as usize,
        hook_update_top_tile as *const () as usize,
        hook_update_all as *const () as usize,
        hook_update_anchor_tile as *const () as usize,
    ];
    unsafe {
        let vtable = *(renderer as *const *mut usize);
        for (i, (&slot, &hook)) in SLOTS.iter().zip(&hooks).enumerate() {
            let p = vtable.add(slot);
            if *p == hook {
                continue;
            }
            ORIG[i].store(*p, Ordering::Release);
            let mut old = 0u32;
            if VirtualProtect(p.cast(), 8, 0x04, &mut old) == 0 {
                return Err("VirtualProtect 失敗".into());
            }
            AtomicUsize::from_ptr(p).store(hook, Ordering::SeqCst);
            VirtualProtect(p.cast(), 8, old, &mut old);
        }
    }
    Ok(())
}

// ---- 主執行緒 ----

/// 一行：本幀連續的幾段字（runs 的索引）。Single：一段譯文；Mono：一段沒有譯文的字（每字照原格）；Chain：接成一行緊排
#[derive(Clone, Copy, PartialEq)]
enum Kind {
    Single,
    Mono,
    Chain,
}

struct Line {
    runs: Vec<usize>,
    top: bool,
    row: Row,
    y: i32,
    x0: i32,
    len: i32,
    kind: Kind,
}

/// 接成一行（Chain）時每一格是什麼：字或照算的空白、譯文補到原文寬度的空白（Run::pad）、前一段原文比譯文長出來的格子（Run::tail）
#[derive(Clone, Copy, PartialEq)]
enum Fill {
    Text,
    Pad,
    Tail,
}

/// 下一段可以接成同一行：同一層、同一列、都靠左、兩段都只有一般字元（中間夾著方框、圖形字元的不接，免得緊排後蓋到），
/// 而且 b 緊接在 a 後面，a 結尾連同中間的空格不超過一格空白（譯文較短空出來的補位不算，見 Run::pad：「標籤：」後面接的值
/// 才會跟著靠過來；欄位補位的空白照算，欄位不接）：
/// - b 從 a 寫的結尾那格畫起（遊戲接著游標寫的：「v」＋「53.16」），或隔一格空白（DFHack 的「標籤」「 」「值」分三段畫，
///   空白那段沒有字不記；段落照英文逐字畫的字）。隔一格的兩段都只有數字時不接：各自對準別的東西，頂部資訊列的心情人數
///   （「210」「52」「78」各自對準一個表情）接成一行緊排會整排往左擠。
/// - b 從 a 原文的結尾（再隔零或一格）畫起：遊戲照英文字寬排好每個字的位置（見 Run::tail：選擇出發地點的金屬清單）。
///   不限寬度畫的表格欄，英文剛好排到下一欄前一格的那一列也會接上（一般欄位限了寬度，不記 Run::tail；
///   實機走過要塞的資訊面板、庫存、人物頁各分頁與 DFHack 視窗都沒有這種列）。
fn chainable(a: &Run, b: &Run) -> bool {
    let all_text = |r: &Run| r.shadow.iter().all(|&c| is_text(c));
    let spaces = a.trailing_pad().saturating_sub(a.pad) as i32;
    let near = |gap: i32| (0..=1).contains(&gap) && spaces + gap < 2;
    let gap = b.x0 - a.end();
    let numbers = gap == 1 && is_number(&a.shadow) && is_number(&b.shadow);
    let next = (near(gap) && !numbers) || (a.tail > 0 && near(gap - a.tail as i32));
    a.top == b.top && a.row == b.row && a.y == b.y && next && a.align == b'l' && b.align == b'l' && all_text(a) && all_text(b)
}

/// 只有數字的一段（數量、日期，可夾小數點、正負號這類符號）：遊戲照欄位或圖示的位置擺（見 chainable、strip 的 Mono）
fn is_number(s: &[u8]) -> bool {
    s.iter().any(u8::is_ascii_digit) && s.iter().all(|b| b.is_ascii_digit() || b".,-+%/:".contains(b))
}

/// 一個字點陣化後的方框：遮罩寬 w、高為格高；org 是字的原點在方框裡的 x
struct GlyphBox {
    mask: Vec<u8>,
    w: usize,
    org: i32,
    adv: f32,
}

struct Strip {
    tex: Tex,
    /// 每段字在貼圖裡的像素範圍（Chain 依段上色用）
    ranges: Vec<(i32, i32)>,
    last: u64,
}

#[derive(Default)]
struct State {
    runs: Option<Arc<Vec<Run>>>,
    lines: Vec<Line>,
    /// [主層, 頂層, 錨定貼圖標籤]（見 layer_of）：x*dimy+y → 1 + 本幀第幾段（0：不算數）
    owner: [Vec<u32>; 3],
    dimy: i32,
    /// 主層的字貼過了（頂層開始畫之前、或 update_all 結束）
    flushed: bool,
    /// 本幀遊戲有畫頂層（top_in_use 時才會呼叫 update_top_tile）
    top_pass: bool,
    frame: u64,
    cache: HashMap<u64, Strip>,
    /// [玩家設定的大小, 小一號, 預設大小]：字型（依目前格高）與字的點陣快取；size 選用哪一組（見 size_for）。
    /// Glyphs 借用 typeface 的字型資料：換字型時先清掉這裡（見 ready），typeface 也宣告在後面、後釋放
    glyphs: [Option<(i32, Glyphs<'static>)>; 3],
    boxes: [HashMap<char, Option<GlyphBox>>; 3],
    size: usize,
    /// 這一行沒有譯文、畫在數值框的左邊（見 left_in_box）：數字照起始格畫
    left_numbers: bool,
    /// 接成一行的字畫在框的右邊（見 right_in_box）：緊排以後也靠右
    right_box: bool,
    /// 置中的一段字畫在按鈕、分頁上，而且原文在這張框裡置中（見 box_shift）：框的正中間比原文的正中間偏多少（兩倍，格數）
    box_shift: i32,
    chars: Vec<char>,
    /// 從格子讀回來、接在本幀某一行旁邊的字（見 begin）
    extra: Vec<Run>,
    /// 除錯：頂層開始前貼主層花的時間（update_all 結束時併入耗時）
    spent: std::time::Duration,
    /// 目前畫字用的字型（TYPEFACE 的那一份）與 STYLE：STYLE 變了就重新準備（見 restyle_if_needed）
    typeface: Option<Arc<Vec<atlas::Font>>>,
    style: u64,
    /// 這一幀用的字典（begin 時取）：核對格子與 glyph 編號換回字都用這一份，換語言時一幀裡不會混用兩份字典；
    /// chars 是照 chars_dict 那一份建的。字典換下來也不釋放（lib.rs 的 LOADED），留著指標不會懸空
    dict: usize,
    chars_dict: usize,
}

thread_local! {
    static STATE: RefCell<State> = RefCell::new(State::default());
}

/// 一格在畫面緩衝區的 (字元與顏色 8 位元組, texpos) 指標；top 是頂層
unsafe fn cell(gps: *const u8, idx: usize, top: bool) -> Option<(*mut u8, *mut i32)> {
    let l = LAYOUT.get()?;
    let layer = if top { l.top } else { l.main };
    unsafe {
        let screen = *(gps.add(layer.screen) as *const *mut u8);
        let tex = *(gps.add(layer.texpos) as *const *mut i32);
        (!screen.is_null() && !tex.is_null()).then(|| (screen.add(idx * 8), tex.add(idx)))
    }
}

/// 一段字的格子記在哪一層的 owner：主層 0、頂層 1、錨定貼圖標籤 2
fn layer_of(top: bool, row: Row) -> usize {
    if row == Row::Label { 2 } else { top as usize }
}

/// 字的點陣與一行貼圖在格子上方多留的列數：字往上提以後，重音符號、較高的中文字上緣會超出格子
fn margin(dy: i32) -> i32 {
    dy / 4
}

/// 置中的字從哪個像素起畫（相對這一段的第一格）：span 是原文佔的寬度、width 是字寬，shift 是框的正中間比原文的正中間
/// 偏多少的兩倍（像素，見 box_shift）。字不超出原文的格子（格子以外不是這一段的，貼不上）；比原文寬時照原文的正中間
fn centered_start(span: i32, width: i32, shift: i32) -> i32 {
    if width >= span {
        return (span - width) / 2;
    }
    ((span + shift - width) / 2).clamp(0, span - width)
}

/// 一個字的點陣：高度是格高加上方的 margin 列，s 是大小（見 size_for），kx 是字形橫向縮放（長體，見 State::strip）。
/// 字寬照 atlas::narrow（間隔號半個字寬）：點陣仍照字型的字寬置中畫，org 照排版的字寬算，點落在半個字寬的正中間
fn glyph_box(glyphs: &Glyphs, c: char, dy: i32, s: usize, kx: f32) -> Option<GlyphBox> {
    let em = (dy as f32 * per_cell(s)).round();
    let full = glyphs.advance(c)?;
    let w = full.ceil() as usize + 4;
    let m = margin(dy);
    let mut mask = vec![0u8; w * (dy + m) as usize];
    // Glyphs::draw 把字身放在方框正中間：方框多了上面 m 列，基線要多往下 m / 2 才落在格子裡同一個位置。
    // 小一號的字放在按鈕上下緣線中間（比格子正中間高一點）
    let raise = if s == SMALL { SMALL_RAISE_PER_CELL } else { RAISE_PER_CELL };
    let shift = m as f32 / 2.0 - (dy as f32 * raise).round();
    glyphs.draw_scaled(c, kx, w, (dy + m) as usize, shift, &mut mask).ok()?.then_some(())?;
    let adv = atlas::narrow(c, full, em) * kx;
    Some(GlyphBox { mask, w, org: ((w as f32 - adv) / 2.0).round() as i32, adv })
}

impl State {
    /// update_all 開始：逐格核對本幀每段字的格子還是不是它寫的，再把連續的幾段接成行。
    unsafe fn begin(&mut self) {
        self.frame += 1;
        self.flushed = false;
        self.top_pass = false;
        self.lines.clear();
        // 先取字典再看 STYLE：換字典前會先 restyle（lib.rs 的 publish_dict），看到新字典就一定看到作廢，不會用舊字典畫的貼圖
        let dict = DICT.load(Ordering::Acquire);
        self.restyle_if_needed();
        self.dict = dict as usize;
        self.runs = FRAME.lock().clone();
        let gps = GPS.load(Ordering::Relaxed) as *const u8;
        let (Some(l), Some(runs)) = (LAYOUT.get(), self.runs.clone()) else { return };
        if gps.is_null() || dict.is_null() {
            return;
        }
        let dict = unsafe { &*dict };
        let (dimx, dimy): (i32, i32) = unsafe { (*(gps.add(l.dimx) as *const i32), *(gps.add(l.dimy) as *const i32)) };
        self.dimy = dimy;
        for owner in &mut self.owner {
            owner.clear();
            owner.resize((dimx * dimy).max(0) as usize, 0);
        }
        let anchored = unsafe { *(gps.add(l.anchored) as *const *const i32) };
        for (r, run) in runs.iter().enumerate() {
            // 錨定貼圖標籤：錨點格還是標籤換上的貼圖才算數（下面那格遊戲不畫）
            if let Some((tiles, _)) = &run.anchor {
                for (k, &want) in tiles.iter().enumerate() {
                    let x = run.x0 + k as i32;
                    if anchored.is_null() || x < 0 || x >= dimx || run.y < 0 || run.y >= dimy {
                        continue;
                    }
                    let idx = (x * dimy + run.y) as usize;
                    if unsafe { *anchored.add(idx) } == want {
                        self.owner[2][idx] = r as u32 + 1;
                    }
                }
                continue;
            }
            // 每格要看到的 texpos：一字兩格的字圖、照字寬縮短的段落的透明記號，或沒有（一般字元，看字元本身）
            let mut glyph_at = vec![if run.text.is_some() { dict.marker() } else { None }; run.shadow.len()];
            for &(off, g) in &run.glyphs {
                for half in 0..2 {
                    if let Some(slot) = glyph_at.get_mut(off + half) {
                        *slot = dict.tile(g, half, run.row);
                    }
                }
            }
            for (k, &ch) in run.shadow.iter().enumerate() {
                let x = run.x0 + k as i32;
                if x < 0 || x >= dimx || run.y < 0 || run.y >= dimy || (glyph_at[k].is_none() && !is_text(ch)) {
                    continue;
                }
                let idx = (x * dimy + run.y) as usize;
                let Some((c, t)) = (unsafe { cell(gps, idx, run.top) }) else { return };
                // 字圖格看 texpos；一般字元格要字元相同、而且沒有貼圖（後來被畫上圖示的格子不搶）
                let mine = unsafe {
                    match glyph_at[k] {
                        Some(want) => *t == want,
                        None => *c == ch && *t == 0,
                    }
                };
                if mine {
                    self.owner[run.top as usize][idx] = r as u32 + 1;
                }
            }
        }
        for (r, run) in runs.iter().enumerate() {
            if let Some(line) = self.lines.last_mut() {
                let prev = &runs[*line.runs.last().unwrap()];
                // 遊戲接著游標連續寫的字（「v」＋「53.16」、「?」＋「: 」＋「快速入門指南」）都接成一行
                if chainable(prev, run) {
                    line.runs.push(r);
                    line.len = run.end() - line.x0;
                    line.kind = Kind::Chain;
                    continue;
                }
            }
            self.lines.push(Line {
                runs: vec![r],
                top: run.top,
                row: run.row,
                y: run.y,
                x0: run.x0,
                len: run.shadow.len() as i32,
                kind: if run.plain() { Kind::Mono } else { Kind::Single },
            });
        }
        // 段落裡的連結字（遊戲另外畫，記下的順序不一定緊接著）：和緊貼在前後的段落文字（照字寬縮短的段）接成一行緊排，
        // 連結字後面才不會留下英文字寬的空隙
        let starts: HashMap<(bool, u8, i32, i32), usize> =
            self.lines.iter().enumerate().map(|(j, l)| ((l.top, l.row as u8, l.y, l.x0), j)).collect();
        let mut lines: Vec<Option<Line>> = std::mem::take(&mut self.lines).into_iter().map(Some).collect();
        for i in 0..lines.len() {
            while let Some(a) = &lines[i] {
                let last = &runs[*a.runs.last().unwrap()];
                let next = [0, 1].into_iter().find_map(|gap| {
                    starts.get(&(a.top, a.row as u8, a.y, a.x0 + a.len + gap)).copied().filter(|&j| {
                        lines[j].as_ref().is_some_and(|b| {
                            let first = &runs[b.runs[0]];
                            j != i && (last.text.is_some() || first.text.is_some()) && chainable(last, first)
                        })
                    })
                });
                let Some(b) = next.and_then(|j| lines[j].take()) else { break };
                let a = lines[i].as_mut().unwrap();
                a.runs.extend(b.runs);
                a.len = b.x0 + b.len - a.x0;
                a.kind = Kind::Chain;
            }
        }
        self.lines = lines.into_iter().flatten().collect();
        // 接起來的兩段中間空著的格子（隔一格空白、前一段原文比譯文長出來的格子，見 chainable）：是空白就算後一段的，
        // 緊排以後跨過這些格子的字才不會被切掉
        for line in self.lines.iter().filter(|l| l.kind == Kind::Chain) {
            if line.y < 0 || line.y >= dimy {
                continue;
            }
            for pair in line.runs.windows(2) {
                let (a, b) = (&runs[pair[0]], &runs[pair[1]]);
                for x in a.end().max(0)..b.x0.min(dimx) {
                    let idx = (x * dimy + line.y) as usize;
                    let Some((c, t)) = (unsafe { cell(gps, idx, line.top) }) else { continue };
                    if unsafe { (*c == b' ' || *c == 0) && *t == 0 } {
                        self.owner[line.top as usize][idx] = pair[1] as u32 + 1;
                    }
                }
            }
        }
        // 遊戲用別的方式（逐字）緊接著畫在旁邊的一般字元（暱稱的引號、♂）：從格子讀回來接進這一行，緊排時一起移動。
        // 置中、靠右的譯文前後有補位空白，碰不到旁邊的字
        self.extra.clear();
        let grab = |owner: &[u32], x: i32, y: i32, top: bool| -> Option<u8> {
            if x < 0 || x >= dimx {
                return None;
            }
            let idx = (x * dimy + y) as usize;
            if owner[idx] != 0 {
                return None;
            }
            let (c, t) = unsafe { cell(gps, idx, top) }?;
            let ch = unsafe { *c };
            (unsafe { *t } == 0 && ch != b' ' && is_text(ch)).then_some(ch)
        };
        for li in 0..self.lines.len() {
            let line = &self.lines[li];
            let first = run_at(&runs, &self.extra, line.runs[0]);
            if line.row != Row::Whole || (line.kind == Kind::Single && first.align != b'l') {
                continue;
            }
            let (top, y, x0, x1) = (line.top, line.y, line.x0, line.x0 + line.len);
            let owner = &self.owner[top as usize];
            let mut left: Vec<u8> = std::iter::successors(Some(x0 - 1), |x| Some(x - 1)).map_while(|x| grab(owner, x, y, top)).collect();
            left.reverse();
            let right: Vec<u8> = (x1..).map_while(|x| grab(owner, x, y, top)).collect();
            if left.is_empty() && right.is_empty() {
                continue;
            }
            let base = runs.len();
            let add = |x: i32, bytes: Vec<u8>, extra: &mut Vec<Run>, owner: &mut [u32]| -> usize {
                let id = base + extra.len();
                for k in 0..bytes.len() as i32 {
                    owner[((x + k) * dimy + y) as usize] = id as u32 + 1;
                }
                extra.push(Run { top, row: Row::Whole, x0: x, y, shadow: bytes, glyphs: Vec::new(), align: b'l', pad: 0, tail: 0, text: None, anchor: None, dfhack: false });
                id
            };
            let (nl, nr) = (left.len() as i32, right.len() as i32);
            let owner = &mut self.owner[top as usize];
            let l_id = (!left.is_empty()).then(|| add(x0 - nl, left, &mut self.extra, owner));
            let r_id = (!right.is_empty()).then(|| add(x1, right, &mut self.extra, owner));
            let line = &mut self.lines[li];
            if let Some(id) = l_id {
                line.runs.insert(0, id);
                line.x0 -= nl;
            }
            if let Some(id) = r_id {
                line.runs.push(id);
            }
            line.len += nl + nr;
            line.kind = Kind::Chain;
        }
    }

    /// (x, y) 在 layer 層（見 layer_of）算數時是哪一段（1 + 第幾段），不算數是 0
    fn owner_of(&self, x: i32, y: i32, layer: usize) -> u32 {
        if x < 0 || y < 0 || self.dimy <= 0 || y >= self.dimy {
            return 0;
        }
        self.owner[layer].get((x * self.dimy + y) as usize).copied().unwrap_or(0)
    }

    fn owns(&self, x: i32, y: i32, layer: usize) -> bool {
        self.owner_of(x, y, layer) != 0
    }

    unsafe fn fg(&self, gps: *const u8, x: i32, y: i32, top: bool) -> [u8; 3] {
        let idx = (x * self.dimy + y) as usize;
        unsafe { cell(gps, idx, top).map_or([255; 3], |(c, _)| [*c.add(1), *c.add(2), *c.add(3)]) }
    }

    /// (x, y) 字底下（lower 層：按鈕、面板的九宮格底圖）的貼圖編號；畫面外是 0
    unsafe fn lower(&self, gps: *const u8, top: bool, x: i32, y: i32) -> i32 {
        let Some(l) = LAYOUT.get() else { return 0 };
        let layer = if top { l.top } else { l.main };
        let lower = unsafe { *(gps.add(layer.lower) as *const *const i32) };
        let idx = (x * self.dimy + y) as usize;
        if lower.is_null() || x < 0 || y < 0 || y >= self.dimy || idx >= self.owner[0].len() { 0 } else { unsafe { *lower.add(idx) } }
    }

    /// 字底下的框（lower 層的九宮格）在上一列、下一列還有沒有同一組的貼圖；字底下沒有框是 None。
    /// 同一組九宮格的貼圖編號相近（量過：三列高的按鈕上下列差 6、方框差 64）
    unsafe fn box_rows(&self, gps: *const u8, line: &Line) -> Option<(bool, bool)> {
        let x = line.x0 + line.len / 2;
        let at = |y: i32| unsafe { self.lower(gps, line.top, x, y) };
        let mid = at(line.y);
        let same = |y: i32| at(y) != 0 && (at(y) - mid).abs() <= 64;
        (mid != 0).then(|| (same(line.y - 1), same(line.y + 1)))
    }

    /// 這一行在字底下那張框（lower 層同一種貼圖連成的一段）裡左右各剩幾格；起點那格沒有框是 None
    unsafe fn box_margins(&self, gps: *const u8, line: &Line) -> Option<(i32, i32)> {
        let at = |x: i32| unsafe { self.lower(gps, line.top, x, line.y) };
        let mid = at(line.x0);
        if mid == 0 {
            return None;
        }
        let left = (1..).take_while(|&d| at(line.x0 - d) == mid).count() as i32;
        Some((left, (0..).take_while(|&d| at(line.x0 + d) == mid).count() as i32 - line.len))
    }

    /// 沒有譯文的一行畫在數值框裡、靠框的左邊（設定、建立世界的數值欄）：數字照原本的起始格畫，
    /// 不像清單的數量欄那樣對齊原本的結尾格（見 strip 的 Mono）
    unsafe fn left_in_box(&self, gps: *const u8, line: &Line) -> bool {
        unsafe { self.box_margins(gps, line) }.is_some_and(|(left, right)| left < right)
    }

    /// 接成一行的字畫在框的右邊：右邊只剩一格以內、左邊空很多（存檔清單的「資料夾：」＋「region1」，
    /// 單獨一段的見 hooks.rs 的 placed_align）。DFHack 的字不算：它的快捷鍵列第二欄剛好排到框的右緣，其實是靠左（flush 判斷）
    unsafe fn right_in_box(&self, gps: *const u8, line: &Line) -> bool {
        unsafe { self.box_margins(gps, line) }.is_some_and(|(left, right)| right <= 1 && left >= 4)
    }

    /// 置中的一段字對準框的正中間，不是原文的正中間：遊戲的分頁把英文從中段起點畫起、最後一格壓進右邊框，
    /// 英文本身比分頁正中偏右半格，比英文短的中文看得出來。原文在框裡置中（hooks.rs 的 centered_in，和判斷置中同一個條件）
    /// 才換：說明視窗的標題置中在整排（含問號框），不在中段那張框裡置中，照原文的正中間。DFHack 的字位置由 DFHack 算好，不動
    unsafe fn box_shift(&self, gps: *const u8, line: &Line, run: &Run) -> i32 {
        if line.kind != Kind::Single || run.align != b'c' || run.dfhack {
            return 0;
        }
        match unsafe { self.box_margins(gps, line) } {
            Some((left, right)) if crate::hooks::centered_in((left, right)) => right - left,
            _ => 0,
        }
    }

    /// 這一層畫完：每行的貼圖照「格子還算數」切段貼上。Single、Mono 依格子的前景色分段上色（字在原格附近），
    /// Chain 的字已經移位，改成每段用自己第一個算數格的顏色。
    unsafe fn flush(&mut self, r: *mut u8, top: bool) {
        if !top {
            self.flushed = true;
        }
        let (Some(sdl), Some(runs)) = (sdl(), self.runs.clone()) else { return };
        let gps = GPS.load(Ordering::Relaxed) as *const u8;
        if gps.is_null() {
            return;
        }
        let (rend, dx, dy, ox, oy) = unsafe {
            (
                *(r.add(0x108) as *const *mut c_void),
                *(r.add(0x168) as *const i32),
                *(r.add(0x16c) as *const i32),
                *(r.add(0x170) as *const i32),
                *(r.add(0x174) as *const i32),
            )
        };
        if rend.is_null() || dx < 1 || dy < 1 {
            return;
        }
        let lines = std::mem::take(&mut self.lines);
        let extra = std::mem::take(&mut self.extra);
        for line in lines.iter().filter(|l| l.top == top) {
            let mine = |o: u32| o != 0 && line.runs.contains(&(o as usize - 1));
            let layer = layer_of(top, line.row);
            let owned: Vec<bool> = (0..line.len).map(|k| mine(self.owner_of(line.x0 + k, line.y, layer))).collect();
            if !owned.contains(&true) {
                continue;
            }
            // 沒有譯文的字裡有好幾種顏色（彩色文字）時每個字留在原格，顏色才對得上
            let mono = line.kind == Kind::Mono && {
                let mut colors = (0..line.len)
                    .filter(|&k| owned[k as usize] && run_at(&runs, &extra, line.runs[0]).shadow[k as usize] != b' ')
                    .map(|k| unsafe { self.fg(gps, line.x0 + k, line.y, top) });
                colors.next().is_some_and(|c| colors.any(|o| o != c))
            };
            let rows = if line.row == Row::Whole { unsafe { self.box_rows(gps, line) } } else { None };
            self.size = size_for(line.row, rows, em_per_cell() > per_cell(SNUG));
            // 框的下緣線畫在這一列的格子裡（冒險角色名字框的第二列：上一列還是同一組、下一列不是）：字往上挪一點，字底才不會壓到下緣線
            let lift = if rows == Some((true, false)) { (dy / 16).max(1) } else { 0 };
            self.left_numbers = line.kind == Kind::Mono && unsafe { self.left_in_box(gps, line) };
            self.right_box = line.kind == Kind::Chain
                && !line.runs.iter().any(|&ri| run_at(&runs, &extra, ri).dfhack)
                && unsafe { self.right_in_box(gps, line) };
            self.box_shift = unsafe { self.box_shift(gps, line, run_at(&runs, &extra, line.runs[0])) };
            let strip = unsafe { self.strip(sdl, rend, line, &runs, &extra, dx, dy, mono) };
            (self.size, self.left_numbers, self.right_box, self.box_shift) = (NORMAL, false, false, 0);
            let Some((tex, ranges)) = strip else { continue };
            // 半格位移的文字：貼圖往下移半格，上列畫貼圖的上半段、下列畫下半段（見 atlas::render 的上下列字圖）；
            // 錨定貼圖標籤照遊戲畫那張貼圖的位置往下移。貼圖上面多留 margin 列（字的上緣超出格子的部分），畫在格子上方
            let (sy, h, ty) = match line.row {
                Row::Top => (0, dy - dy / 2, dy / 2),
                Row::Bottom => (dy - dy / 2, dy / 2, 0),
                Row::Label => (0, dy, run_at(&runs, &extra, line.runs[0]).anchor.as_ref().map_or(0, |a| a.1)),
                Row::Whole => (0, dy, 0),
            };
            let m = margin(dy);
            let (sy, h, ty) = if sy == 0 { (0, h + m, ty - m) } else { (sy + m, h, ty) };
            let base_x = ox + line.x0 * dx;
            let base_y = oy + line.y * dy + ty - lift;
            let draw = |px0: i32, px1: i32, [cr, cg, cb]: [u8; 3]| {
                if px1 <= px0 {
                    return;
                }
                let src = Rect { x: px0, y: sy, w: px1 - px0, h };
                let dst = Rect { x: base_x + px0, y: base_y, w: px1 - px0, h };
                unsafe {
                    (sdl.color)(tex, cr, cg, cb);
                    (sdl.copy)(rend, tex, &src, &dst);
                }
            };
            // 算數的格子連成的區段 [a, b)
            let mut spans = Vec::new();
            let mut k = 0;
            while k < line.len as usize {
                if !owned[k] {
                    k += 1;
                    continue;
                }
                let a = k;
                while k < line.len as usize && owned[k] {
                    k += 1;
                }
                spans.push((a as i32, k as i32));
            }
            if line.kind == Kind::Chain {
                for (i, &ri) in line.runs.iter().enumerate() {
                    let run = run_at(&runs, &extra, ri);
                    let first = (0..run.shadow.len() as i32)
                        .map(|k| run.x0 + k)
                        .find(|&x| self.owner_of(x, run.y, layer) == ri as u32 + 1 && run.shadow[(x - run.x0) as usize] != b' ')
                        .or_else(|| (0..run.shadow.len() as i32).map(|k| run.x0 + k).find(|&x| self.owner_of(x, run.y, layer) == ri as u32 + 1));
                    let color = first.map_or([255; 3], |x| unsafe { self.fg(gps, x, run.y, top) });
                    let (p0, p1) = ranges[i];
                    for &(a, b) in &spans {
                        draw(p0.max(a * dx), p1.min(b * dx), color);
                    }
                }
            } else {
                for &(a, b) in &spans {
                    let mut k = a;
                    while k < b {
                        let fg = unsafe { self.fg(gps, line.x0 + k, line.y, top) };
                        let s = k;
                        while k < b && unsafe { self.fg(gps, line.x0 + k, line.y, top) } == fg {
                            k += 1;
                        }
                        draw(s * dx, k * dx, fg);
                    }
                }
            }
        }
        self.lines = lines;
        self.extra = extra;
    }

    /// 字型、字的大小或字典換了（STYLE 變了）：照舊樣子畫的點陣與貼圖都丟掉，換上新字型。只在每幀開頭（begin）看，
    /// 一幀裡用的都是同一套；Glyphs 借用舊字型，要在換掉字型之前丟掉
    fn restyle_if_needed(&mut self) {
        let style = STYLE.load(Ordering::Acquire);
        if self.typeface.is_some() && self.style == style {
            return;
        }
        if let Some(sdl) = sdl() {
            for strip in self.cache.values() {
                unsafe { (sdl.destroy)(strip.tex) };
            }
        }
        self.cache.clear();
        TEXTURES.store(0, Ordering::Relaxed);
        self.glyphs = [None, None, None];
        self.boxes.iter_mut().for_each(HashMap::clear);
        self.typeface = TYPEFACE.read().clone();
        self.style = style;
    }

    /// 字型（依目前格高）與 glyph id → 字 的對照（照這一幀的字典）；第一次用時、或格高、字典換了以後重新準備
    fn ready(&mut self, dy: i32) -> Option<()> {
        if self.chars_dict != self.dict || self.chars.is_empty() {
            let dict = unsafe { (self.dict as *const crate::dict::Dict).as_ref() }?;
            self.chars = vec![' '; dict.glyph_of.len()];
            for (&c, &g) in &dict.glyph_of {
                self.chars[g as usize] = c;
            }
            self.chars_dict = self.dict;
        }
        // self.typeface 持有這份字型，換字型前會先清掉 glyphs（見上面），借用活不過它
        let fonts: &'static [atlas::Font] = unsafe { &*std::ptr::from_ref(self.typeface.as_deref()?.as_slice()) };
        for s in [NORMAL, SMALL, SNUG] {
            if self.glyphs[s].as_ref().is_none_or(|(h, _)| *h != dy) {
                let em = (dy as f32 * per_cell(s)).round();
                self.glyphs[s] = Some((dy, Glyphs::with_px(fonts, em).ok()?));
                self.boxes[s].clear();
            }
        }
        Some(())
    }

    /// 一個字的點陣（快取，大小照 size）；字型沒有這個字時是 None
    fn glyph_box(&mut self, c: char, dy: i32) -> Option<&GlyphBox> {
        let s = self.size;
        if !self.boxes[s].contains_key(&c) {
            let made = glyph_box(&self.glyphs[s].as_ref()?.1, c, dy, s, 1.0);
            self.boxes[s].insert(c, made);
        }
        self.boxes[s].get(&c)?.as_ref()
    }

    /// 字寬（像素，大小照 size）：空白至少三分之一字寬（字型的空白只有約 0.22 字寬，中英數字之間看起來黏在一起）
    fn advance(&mut self, c: char, dy: i32) -> f32 {
        let s = self.size;
        let em = (dy as f32 * per_cell(s)).round();
        let adv = self.glyph_box(c, dy).map_or(em * 0.5, |g| g.adv);
        if c == ' ' {
            let space = self.glyphs[s].as_ref().and_then(|(_, g)| g.advance(' ')).unwrap_or(0.0);
            space.max(em / 3.0)
        } else {
            adv
        }
    }

    /// 全形標點連著畫（「（要塞）：」「：「」）時字身裡的空白疊在一起，看起來空了一個字：前一個少算半個字寬
    /// （中文排版的標點擠壓）。只擠「後括號接標點」「標點接前括號」，兩邊的筆畫都在字身的另一半，不會碰在一起
    fn squeeze(&mut self, c: char, next: Option<char>, dy: i32) -> f32 {
        let squeeze = next.is_some_and(|n| (close_bracket(c) && full_width_punctuation(n)) || (full_width_punctuation(c) && open_bracket(n)));
        if squeeze { self.advance(c, dy) / 2.0 } else { 0.0 }
    }

    /// 靠左的一行以前括號開頭（「「謹慎準備」出發確認」「（衝鋒）碰撞」）：括號的左半字身是空的，字看起來比同一欄的
    /// 其他列縮進去；往左挪半個字寬，筆畫對齊別列的第一個字
    fn lead_trim(&mut self, c: char, dy: i32) -> f32 {
        if open_bracket(c) { self.advance(c, dy) / 2.0 } else { 0.0 }
    }

    /// 一行的貼圖（快取）與每段字在貼圖裡的像素範圍。mono：沒有譯文的字每個留在原格正中間（彩色文字）
    #[allow(clippy::too_many_arguments)]
    unsafe fn strip(&mut self, sdl: &Sdl, rend: *mut c_void, line: &Line, base: &[Run], extra: &[Run], dx: i32, dy: i32, mono: bool) -> Option<(Tex, Vec<(i32, i32)>)> {
        let mut h = std::collections::hash_map::DefaultHasher::new();
        (line.kind as u8, line.row as u8, mono, self.size, self.left_numbers, self.right_box, self.box_shift, dx, dy).hash(&mut h);
        for &ri in &line.runs {
            let run = run_at(base, extra, ri);
            (&run.shadow, &run.glyphs, run.align, run.x0 - line.x0, &run.text).hash(&mut h);
        }
        let key = h.finish();
        let frame = self.frame;
        if let Some(s) = self.cache.get_mut(&key) {
            s.last = frame;
            return Some((s.tex, s.ranges.clone()));
        }
        self.ready(dy)?;
        let chars = std::mem::take(&mut self.chars);
        // (原點 x, 字)
        let mut placed: Vec<(f32, char)> = Vec::new();
        let mut ranges = Vec::new();
        let span = line.len * dx;
        match line.kind {
            // 沒有譯文的字：單一空白隔開的字詞接成一組緊排；兩格以上的空白（對齊欄位用的補位）或圖形字元分開各組。
            // 每組從原本的起始格畫起，只有數字的組（數量、日期）對齊原本的結尾格，欄位的左緣、數字的右緣都不動；
            // 數值框裡靠左畫的數字（left_numbers，設定、建立世界的數值欄）也照起始格
            Kind::Mono if mono => {
                for (k, &b) in run_at(base, extra, line.runs[0]).shadow.iter().enumerate() {
                    if b == b' ' || !is_text(b) {
                        continue;
                    }
                    let c = crate::cp437::TABLE[b as usize];
                    let adv = self.advance(c, dy);
                    placed.push((k as f32 * dx as f32 + (dx as f32 - adv) / 2.0, c));
                }
                ranges.push((0, span));
            }
            Kind::Mono => {
                let s = &run_at(base, extra, line.runs[0]).shadow;
                let mut k = 0;
                while k < s.len() {
                    if s[k] == b' ' || !is_text(s[k]) {
                        k += 1;
                        continue;
                    }
                    let start = k;
                    let (mut j, mut end) = (k, k);
                    while j < s.len() && is_text(s[j]) {
                        if s[j] == b' ' {
                            let gap = s[j..].iter().take_while(|&&c| c == b' ').count();
                            if gap >= 2 {
                                break;
                            }
                            j += gap;
                            continue;
                        }
                        j += 1;
                        end = j;
                    }
                    let group: Vec<char> = s[start..end].iter().map(|&b| crate::cp437::TABLE[b as usize]).collect();
                    let mut pen = 0f32;
                    let mut rel = Vec::with_capacity(group.len());
                    for &c in &group {
                        rel.push((pen, c));
                        pen += self.advance(c, dy);
                    }
                    let numeric = is_number(&s[start..end]);
                    let x = if numeric && !self.left_numbers { end as f32 * dx as f32 - pen } else { start as f32 * dx as f32 };
                    placed.extend(rel.into_iter().map(|(p, c)| (x + p, c)));
                    k = end.max(start + 1);
                }
                ranges.push((0, span));
            }
            Kind::Single => {
                let run = run_at(base, extra, line.runs[0]);
                let items = run.items(&chars);
                // 遊戲照原樣畫在原格的圖形字元（品質記號 ≡、☼ 這類）：那幾格不算數，字不能整句緊排跨過去
                let graphic = |&(k, _): &(usize, char)| {
                    run.text.is_none() && !run.glyphs.iter().any(|&(off, _)| off == k) && !is_text(run.shadow[k])
                };
                let first = items.iter().position(|&(_, c)| c != ' ');
                let last = items.iter().rposition(|&(_, c)| c != ' ');
                if items.iter().any(graphic) {
                    // 圖形字元隔開的每段字從自己原本的起始格緊排；兩邊都夾著圖形字元的段放在中間（≡銅斧≡）
                    let mut i = 0;
                    while i < items.len() {
                        if graphic(&items[i]) || items[i].1 == ' ' {
                            i += 1;
                            continue;
                        }
                        let (s, cell) = (i, items[i].0);
                        let mut pen = 0f32;
                        let mut seg = Vec::new();
                        while i < items.len() && !graphic(&items[i]) {
                            let c = items[i].1;
                            seg.push((pen, c));
                            pen += self.advance(c, dy) - self.squeeze(c, items.get(i + 1).map(|&(_, n)| n), dy);
                            i += 1;
                        }
                        let mut x = (cell as i32 * dx) as f32;
                        if let (Some(next), true) = (items.get(i), s > 0 && graphic(&items[s - 1])) {
                            let room = ((next.0 - cell) as i32 * dx) as f32;
                            let width = seg.iter().rev().find(|&&(_, c)| c != ' ').map_or(0.0, |&(p, c)| p + self.advance(c, dy));
                            x += ((room - width) / 2.0).max(0.0);
                        }
                        placed.extend(seg.into_iter().map(|(p, c)| (x + p, c)));
                    }
                } else if let (Some(first), Some(last)) = (first, last) {
                    let lead = items[first].0;
                    let end = items.get(last + 1).map_or(run.shadow.len(), |i| i.0);
                    let mut pen = 0f32;
                    let mut rel = Vec::new();
                    for (j, &(_, c)) in items.iter().enumerate().take(last + 1).skip(first) {
                        rel.push((pen, c));
                        pen += self.advance(c, dy) - self.squeeze(c, items.get(j + 1).map(|&(_, n)| n), dy);
                    }
                    let width = pen.ceil() as i32;
                    // 分頁標籤（半格位移的列）也照對齊：置中的已由 placed_align 判斷、補到英文標籤的寬度，對準框的正中間（box_shift）；
                    // 冒險裝備清單的分類標題這類半格位移的字靠左
                    let start = match run.align {
                        b'c' => centered_start(span, width, self.box_shift * dx) as f32,
                        b'r' => (end as i32 * dx - width) as f32,
                        _ => (lead as i32 * dx) as f32 - self.lead_trim(items[first].1, dy),
                    };
                    placed.extend(rel.into_iter().map(|(x, c)| (start + x, c)));
                }
                ranges.push((0, span));
            }
            // 接成一行：從第一個非空白格起緊排；譯文補到原文寬度的空白不佔位；兩格以上的空白（欄位補位）
            // 跳到下一個字原本的格子（不往回），欄位不跑掉
            Kind::Chain => {
                // (第幾段, 格位, 字, 這格是什麼)；兩段中間空著的格子（DFHack 標籤與值之間的空白）算一般空白，
                // 前一段原文比譯文長出來的格子（Run::tail）另外標出來
                let mut all: Vec<(usize, i32, char, Fill)> = Vec::new();
                let mut prev_end: Option<(i32, i32)> = None;
                for (i, &ri) in line.runs.iter().enumerate() {
                    let run = run_at(base, extra, ri);
                    if let Some((end, tail_end)) = prev_end {
                        all.extend((end..run.x0).map(|x| (i, x - line.x0, ' ', if x < tail_end { Fill::Tail } else { Fill::Text })));
                    }
                    prev_end = Some((run.end(), run.end() + run.tail as i32));
                    let items = run.items(&chars);
                    // 結尾補位的空白（Run::pad）不佔位；譯文本身結尾的空白（「 至 」）照算
                    let last = items.len().saturating_sub(run.pad);
                    all.extend(items.into_iter().enumerate().map(|(j, (k, c))| (i, run.x0 - line.x0 + k as i32, c, if j >= last { Fill::Pad } else { Fill::Text })));
                }
                let mut ranges_px: Vec<(f32, f32)> = vec![(f32::MAX, f32::MIN); line.runs.len()];
                let mut pen: Option<f32> = None;
                let mut n = 0;
                while n < all.len() {
                    let (i, cell, c, _) = all[n];
                    if c == ' ' {
                        let gap = all[n..].iter().take_while(|&&(_, _, c, _)| c == ' ').count();
                        // 只剩補位的空白（後面還有字）：不佔位
                        let pad_only = all[n..n + gap].iter().all(|&(_, _, _, f)| f != Fill::Text) && n + gap < all.len();
                        if let (Some(p), false) = (pen.as_mut(), pad_only) {
                            let real = all[n..n + gap].iter().filter(|&&(_, _, _, f)| f == Fill::Text).count();
                            let (before, after) = (placed.last().map(|&(_, c)| c), all.get(n + gap).map(|&(_, _, c, _)| c));
                            // 遊戲照英文字寬一個個排的詞（前一段有 Run::tail：金屬清單「鐵 金 銀」）是分開的幾項，中文字中間的空白照留
                            let words = all[n..n + gap].iter().any(|&(_, _, _, f)| f == Fill::Tail);
                            let drop = if words { before.is_some_and(full_width_punctuation) } else { drop_space(before, after) };
                            if real >= 2 {
                                if let Some(&(_, next, _, _)) = all.get(n + gap) {
                                    *p = p.max(next as f32 * dx as f32);
                                }
                            } else if real == 1 && !drop {
                                // 一個空白照字型的空白寬；接在全形標點後面（「長期心情：」＋空白＋「滿足」）、夾在兩個中文字中間
                                // （傳說的「Dark」「Fortress」各自翻成「黑暗」「要塞」）的不留
                                let r = &mut ranges_px[i];
                                r.0 = r.0.min(*p);
                                *p += self.advance(' ', dy);
                                r.1 = r.1.max(*p);
                            }
                        }
                        n += gap;
                        continue;
                    }
                    let trim = if pen.is_none() { self.lead_trim(c, dy) } else { 0.0 };
                    let p = pen.get_or_insert(cell as f32 * dx as f32 - trim);
                    let adv = self.advance(c, dy) - self.squeeze(c, all.get(n + 1).map(|&(_, _, n, _)| n), dy);
                    placed.push((*p, c));
                    let r = &mut ranges_px[i];
                    r.0 = r.0.min(*p);
                    *p += adv;
                    r.1 = r.1.max(*p);
                    n += 1;
                }
                if let Some(end) = pen.filter(|_| self.right_box) {
                    let shift = (span as f32 - end).max(0.0);
                    placed.iter_mut().for_each(|p| p.0 += shift);
                    ranges_px.iter_mut().for_each(|r| *r = (r.0 + shift, r.1 + shift));
                }
                ranges.extend(ranges_px.into_iter().map(|(a, b)| if a <= b { (a.floor() as i32, b.ceil() as i32) } else { (0, 0) }));
            }
        }
        // 比記號格寬的一行壓成長體放回格子裡：字距與字形本身一起橫向縮放（只縮字距的話，粗體的相鄰兩字縮到約 93% 就黏在一起；
        // 照原寬畫好再整行重新取樣，筆畫會糊）：
        // - 段落照啟動時的格子大小量字寬（metrics.rs）；之後調整視窗或介面縮放，字和格子的比例變了，一行可能比記號格寬一點
        // - 名字、按鍵名比原文寬一點時照原文的格數交過來（Dict::rule_entry 的 SQUEEZE）
        let mut k = 1.0f32;
        if line.runs.iter().any(|&ri| run_at(base, extra, ri).text.is_some()) {
            let width = placed.iter().map(|&(x, c)| x + self.advance(c, dy)).fold(0.0f32, f32::max);
            if width > span as f32 {
                k = span as f32 / width;
                ranges.iter_mut().for_each(|r| *r = ((r.0 as f32 * k).floor() as i32, (r.1 as f32 * k).ceil() as i32));
            }
        }
        let (w, hgt) = (span.max(1) as usize, (dy + margin(dy)) as usize);
        let mut mask = vec![0u8; w * hgt];
        for &(x, c) in &placed {
            if c == ' ' {
                continue;
            }
            // 長體的字只在建這一行的貼圖時畫一次，不進快取
            let condensed;
            let g = if k < 1.0 {
                condensed = self.glyphs[self.size].as_ref().and_then(|(_, glyphs)| glyph_box(glyphs, c, dy, self.size, k));
                condensed.as_ref()
            } else {
                self.glyph_box(c, dy)
            };
            let Some(g) = g else { continue };
            let left = (x * k).round() as i32 - g.org;
            for yy in 0..hgt {
                for xx in 0..g.w {
                    let tx = left + xx as i32;
                    if tx < 0 || tx >= w as i32 {
                        continue;
                    }
                    let d = &mut mask[yy * w + tx as usize];
                    *d = (*d).max(g.mask[yy * g.w + xx]);
                }
            }
        }
        self.chars = chars;
        let pixels: Vec<u32> = mask.iter().map(|&a| ((a as u32) << 24) | 0x00ff_ffff).collect();
        let tex = unsafe { (sdl.create)(rend, PIXELFORMAT_ARGB8888, 0, w as i32, hgt as i32) };
        if tex.is_null() {
            return None;
        }
        unsafe {
            (sdl.update)(tex, std::ptr::null(), pixels.as_ptr().cast(), (w * 4) as i32);
            (sdl.blend)(tex, BLENDMODE_BLEND);
        }
        if self.cache.len() >= CACHE_LIMIT {
            if let Some((&old, _)) = self.cache.iter().min_by_key(|(_, s)| s.last) {
                if let Some(s) = self.cache.remove(&old) {
                    unsafe { (sdl.destroy)(s.tex) };
                }
            }
        }
        self.cache.insert(key, Strip { tex, ranges: ranges.clone(), last: frame });
        TEXTURES.store(self.cache.len(), Ordering::Relaxed);
        Some((tex, ranges))
    }
}

unsafe extern "C" fn hook_update_all(r: *mut u8) {
    let orig: AllFn = unsafe { std::mem::transmute(ORIG[2].load(Ordering::Acquire)) };
    let t0 = std::time::Instant::now();
    if !ON.load(Ordering::Relaxed) {
        unsafe { orig(r) };
        note_time(t0.elapsed().as_secs_f64() * 1e6, 0.0);
        return;
    }
    let ours = STATE.with_borrow_mut(|s| {
        let t = std::time::Instant::now();
        unsafe { s.begin() };
        t.elapsed()
    });
    unsafe { orig(r) };
    let ours = ours
        + STATE.with_borrow_mut(|s| {
            let t = std::time::Instant::now();
            unsafe {
                if !s.flushed {
                    s.flush(r, false);
                }
                if s.top_pass {
                    s.flush(r, true);
                }
            }
            t.elapsed() + std::mem::take(&mut s.spent)
        });
    note_time(t0.elapsed().as_secs_f64() * 1e6, ours.as_secs_f64() * 1e6);
}

/// 算數的格先畫成只有底色：字元換成空白、texpos 清掉，畫完還原（非同步執行緒這時在等，不會同時寫）
unsafe fn blank_call(r: *mut u8, x: i32, y: i32, top: bool, orig: TileFn) {
    let gps = GPS.load(Ordering::Relaxed) as *const u8;
    let dimy = STATE.with_borrow(|s| s.dimy);
    let Some((c, t)) = (unsafe { cell(gps, (x * dimy + y) as usize, top) }) else { return unsafe { orig(r, x, y) } };
    unsafe {
        let (ch, tex) = (*c, *t);
        *c = b' ';
        *t = 0;
        orig(r, x, y);
        *c = ch;
        *t = tex;
    }
}

unsafe extern "C" fn hook_update_tile(r: *mut u8, x: i32, y: i32) {
    let orig: TileFn = unsafe { std::mem::transmute(ORIG[0].load(Ordering::Acquire)) };
    if ON.load(Ordering::Relaxed) && STATE.with_borrow(|s| s.owns(x, y, 0)) {
        unsafe { blank_call(r, x, y, false, orig) }
    } else {
        unsafe { orig(r, x, y) }
    }
}

unsafe extern "C" fn hook_update_top_tile(r: *mut u8, x: i32, y: i32) {
    let orig: TileFn = unsafe { std::mem::transmute(ORIG[1].load(Ordering::Acquire)) };
    if !ON.load(Ordering::Relaxed) {
        return unsafe { orig(r, x, y) };
    }
    let owned = STATE.with_borrow_mut(|s| {
        s.top_pass = true;
        if !s.flushed {
            let t = std::time::Instant::now();
            unsafe { s.flush(r, false) };
            s.spent += t.elapsed();
        }
        s.owns(x, y, 1)
    });
    if owned {
        unsafe { blank_call(r, x, y, true, orig) }
    } else {
        unsafe { orig(r, x, y) }
    }
}

/// 整句貼圖畫的錨定貼圖標籤：遊戲不畫這格的標籤貼圖，字等主層貼圖時一起畫（見 flush）
unsafe extern "C" fn hook_update_anchor_tile(r: *mut u8, x: i32, y: i32) {
    let orig: TileFn = unsafe { std::mem::transmute(ORIG[3].load(Ordering::Acquire)) };
    if ON.load(Ordering::Relaxed) && STATE.with_borrow(|s| s.owns(x, y, 2)) {
        return;
    }
    unsafe { orig(r, x, y) }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn centered_text_lines_up_with_the_box_center() {
        // 人物頁的「想法」：英文 8 格（80 px）從中段起點畫、壓進右邊框一格（左 0、右 −1），「想法」寬 25 px：
        // 對準分頁中段的正中間，比英文的正中間往左半格（5 px）
        let (left, right) = (0, -1);
        assert!(crate::hooks::centered_in((left, right)));
        assert_eq!(centered_start(80, 25, (right - left) * 10), 22);
        assert_eq!(centered_start(80, 25, 0), 27, "不在框裡置中的（說明視窗的標題、畫面正中間的字）照原文的正中間");
        assert_eq!(centered_start(80, 75, -30), 0, "對準框以後超出原文的格子就貼齊");
        assert_eq!(centered_start(80, 90, -10), -5, "字比原文寬時照原文的正中間");
    }

    #[test]
    fn box_edge_rows_keep_default_size_when_grown() {
        let (whole, label) = (Row::Whole, Row::Label);
        // 一列高的按鈕一律小一號
        assert_eq!(size_for(whole, Some((false, false)), false), SMALL);
        assert_eq!(size_for(whole, Some((false, false)), true), SMALL);
        // 框的最上、最下一列與錨定標籤：放大時照預設大小，沒放大時就是一般大小
        for rows in [Some((true, false)), Some((false, true))] {
            assert_eq!(size_for(whole, rows, true), SNUG);
            assert_eq!(size_for(whole, rows, false), NORMAL);
        }
        assert_eq!(size_for(label, None, true), SNUG);
        assert_eq!(size_for(label, None, false), NORMAL);
        // 框的中間列、沒有框、半格位移的分頁標籤照玩家設定
        assert_eq!(size_for(whole, Some((true, true)), true), NORMAL);
        assert_eq!(size_for(whole, None, true), NORMAL);
        assert_eq!(size_for(Row::Top, None, true), NORMAL);
        assert_eq!(per_cell(SNUG), 13.0 / 16.0);
        // 超出 13～16 的設定不算（不改全域的字身）
        assert!(!set_em(12) && !set_em(17));
    }

    /// 翻譯關掉後 DFHack 照畫的字不記、每幀都是空的一幀；開回來照常收
    #[test]
    fn runs_stop_piling_up_while_translation_is_off() {
        let gps = std::ptr::null_mut();
        let plain = || record(gps, false, Row::Whole, 0, 0, b"DFHack 53.16-r2", &[], b'l', 0, 0, None, true);
        ON.store(true, Ordering::Release);
        crate::hooks::set_enabled(true).unwrap();
        frame_begin();
        plain();
        frame_end();
        assert_eq!(run_counts(), (1, 0));
        plain();
        crate::hooks::set_enabled(false).unwrap();
        assert_eq!(run_counts(), (0, 0), "關掉時清掉收到一半的字與最後一幀");
        for _ in 0..1000 {
            plain();
            record_label(gps, 0, 0, &[1, 2], "分頁", 0);
        }
        assert_eq!(run_counts(), (0, 0), "關著時不記");
        frame_end();
        assert_eq!(run_counts(), (0, 0), "關之前就開始的一幀不蓋掉空幀");
        crate::hooks::set_enabled(true).unwrap();
        frame_begin();
        plain();
        frame_end();
        assert_eq!(run_counts(), (1, 0), "開回來照常收");
        crate::hooks::set_enabled(false).unwrap();
        ON.store(false, Ordering::Release);
    }

    /// 一段靠左的字；glyphs 有東西的是譯文（一字兩格），tail 見 Run::tail
    fn run(x0: i32, shadow: &[u8], glyphs: &[(usize, u32)], tail: usize) -> Run {
        Run { top: false, row: Row::Whole, x0, y: 2, shadow: shadow.to_vec(), glyphs: glyphs.to_vec(), align: b'l', pad: 0, tail, text: None, anchor: None, dfhack: false }
    }

    #[test]
    fn neighbours_close_up_except_numbers_set_apart() {
        // 頂部資訊列的心情人數：遊戲把每個數字各自對準一個表情，隔一格：不接，各自留在原格
        assert!(!chainable(&run(38, b"210", &[], 0), &run(42, b"52", &[], 0)));
        // 遊戲接著游標寫的（「v」＋「53.16」）、段落照英文逐字畫的字（傳說頁沒翻的「"The Crazed Conflict"」）：接
        assert!(chainable(&run(0, b"v", &[], 0), &run(1, b"53.16", &[], 0)));
        assert!(chainable(&run(21, b"\"The", &[], 0), &run(26, b"Crazed", &[], 0)));
        // 隔一格空白、有譯文：「標籤：」＋值、傳說的「黑暗」＋「要塞」
        assert!(chainable(&run(0, b"      ", &[(0, 1), (2, 2), (4, 3)], 0), &run(7, b"12", &[], 0)));
        assert!(chainable(&run(0, b"    ", &[(0, 1), (2, 2)], 0), &run(5, b"    ", &[(0, 3), (2, 4)], 0)));
        // 選擇出發地點的金屬清單：「Iron」只寫了「鐵」兩格，「Gold」照英文排在原文結尾隔一格：接
        assert!(chainable(&run(154, b"  ", &[(0, 1)], 2), &run(159, b"  ", &[(0, 2)], 2)));
        // 上方資訊列的庫存標題照英文各自置中在一欄上（「Drink」→「酒水」，下一個「Seeds」離原文結尾兩格）：不接
        assert!(!chainable(&run(81, b"    ", &[(0, 1), (2, 2)], 1), &run(88, b"    ", &[(0, 3), (2, 4)], 1)));
    }
}
