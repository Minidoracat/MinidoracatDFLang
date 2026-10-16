//! 實驗（overlay-prototype 分支）：畫面上的字改成一行一張貼圖，用本包的字型畫，
//! 不受「一個中文字佔兩格」與遊戲點陣字限制。
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
//! - 譯文：依字型的字寬緊排，依對齊置中、靠右，或從第一個非空白格起。
//! - 遊戲接著游標連續寫的幾段（「範圍：」＋「114」＋「至」＋「768」）裡有譯文時接成一行緊排，每段用自己的顏色。
//! - 其他沒有譯文的字：每個字畫在原本那一格的正中間，版面完全不動，只是不用遊戲的點陣字。
//! - 方框、箭頭這類圖形字元照遊戲原樣畫（見 is_text）。

use std::cell::RefCell;
use std::collections::HashMap;
use std::ffi::c_void;
use std::hash::{Hash, Hasher};
use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};
use std::sync::{Arc, OnceLock};

use parking_lot::Mutex;

use crate::atlas::{self, Glyphs};
use crate::dict::Row;
use crate::hooks::{DICT, LAYOUT};

pub static ON: AtomicBool = AtomicBool::new(false);
/// render_atlas 用的字型檔，主執行緒第一次畫字時載入
pub static FONT_PATHS: OnceLock<Vec<String>> = OnceLock::new();
static GPS: AtomicUsize = AtomicUsize::new(0);

/// 字身相對格高、往上提的比例，和字圖集相同（見 atlas.rs）
const EM_PER_CELL: f32 = 0.875;
const RAISE_PER_CELL: f32 = 1.0 / 16.0;
/// 自己建的一行貼圖上限，滿了丟掉最久沒用的
const CACHE_LIMIT: usize = 2048;

/// 交給整句貼圖畫的一般字元：可見 ASCII、CP437 的拉丁字母（矮人名字的 ë、ü…）與 ♂♀；
/// 方框、箭頭、☼ 這類圖形字元與 ¢£¥₧ƒ 照遊戲原樣畫。字型子集要有這些字（scripts/build.py）。
pub fn is_text(b: u8) -> bool {
    (0x20..=0x7e).contains(&b) || (0x80..=0x9a).contains(&b) || (0xa0..=0xa5).contains(&b) || b == 0x0b || b == 0x0c
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
    /// 空的是沒有譯文、照原樣畫的字
    glyphs: Vec<(usize, u32)>,
    align: u8,
}

impl Run {
    fn plain(&self) -> bool {
        self.glyphs.is_empty()
    }

    fn end(&self) -> i32 {
        self.x0 + self.shadow.len() as i32
    }

    /// 每個字：(起始格, 字)。句型拼出的譯文字圖不一定依格位排序，先依格位建表
    fn items(&self, chars: &[char]) -> Vec<(usize, char)> {
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

    /// 結尾補位的空白有幾格（字圖佔的兩格不算）
    fn trailing_pad(&self) -> usize {
        let covered = self.glyphs.iter().map(|&(off, _)| off + 2).max().unwrap_or(0);
        self.shadow.iter().rev().take_while(|&&c| c == b' ').count().min(self.shadow.len().saturating_sub(covered))
    }
}

static BUILD: Mutex<Vec<Run>> = Mutex::new(Vec::new());
static FRAME: Mutex<Option<Arc<Vec<Run>>>> = Mutex::new(None);

/// 寫字時呼叫（非同步執行緒）：一段字；top 是頂層，row 是一般列或半格位移的上下列，glyphs 空的是沒有譯文的字。
#[allow(clippy::too_many_arguments)]
pub fn record(gps: *mut u8, top: bool, row: Row, x0: i32, y: i32, shadow: &[u8], glyphs: &[(usize, u32)], align: u8) {
    if !ON.load(Ordering::Relaxed) {
        return;
    }
    if glyphs.is_empty() && !shadow.iter().any(|&b| b != b' ' && is_text(b)) {
        return;
    }
    GPS.store(gps as usize, Ordering::Relaxed);
    BUILD.lock().push(Run { top, row, x0, y, shadow: shadow.to_vec(), glyphs: glyphs.to_vec(), align });
}

/// render_things 開始、結束（非同步執行緒）。沒跑 render_things 的幀畫面緩衝區沒變，沿用上一份。
pub fn frame_begin() {
    BUILD.lock().clear();
}

pub fn frame_end() {
    let runs = std::mem::take(&mut *BUILD.lock());
    *FRAME.lock() = Some(Arc::new(runs));
}

/// 除錯：update_all 每幀的耗時（微秒，指數平均）：[整個 update_all, 其中整句貼圖自己花的（核對＋貼圖）]
static TIMING: Mutex<[f64; 2]> = Mutex::new([0.0; 2]);

fn note_time(total: f64, ours: f64) {
    let mut t = TIMING.lock();
    t[0] = t[0] * 0.95 + total * 0.05;
    t[1] = t[1] * 0.95 + ours * 0.05;
}

/// 除錯：耗時，加上本幀記下的字（頂層, 列別, x0, y, 替身字串, 字圖數）
pub fn dump() -> String {
    let t = *TIMING.lock();
    let head = format!("update_all {:.0} us, overlay {:.0} us\n", t[0], t[1]);
    let Some(runs) = FRAME.lock().clone() else { return head };
    head + &runs
        .iter()
        .map(|r| format!("{}\t{:?}\t{}\t{}\t[{}]\t{}\n", r.top as u8, r.row, r.x0, r.y, crate::cp437::decode(&r.shadow), r.glyphs.len()))
        .collect::<String>()
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
/// renderer_2d 的 vtable 槽：0 update_tile、2 update_top_tile、6 update_all（df-structures；53.16 實測位址相符）
const SLOTS: [usize; 3] = [0, 2, 6];
static ORIG: [AtomicUsize; 3] = [const { AtomicUsize::new(0) }; 3];

/// 換掉 renderer vtable 的三個槽。改指標是一次對齊的 8 位元組寫入，主執行緒正在畫也不會讀到一半。
pub unsafe fn install(renderer: usize) -> Result<(), String> {
    if sdl().is_none() {
        return Err("SDL2.dll 找不到需要的函式".into());
    }
    let hooks = [hook_update_tile as *const () as usize, hook_update_top_tile as *const () as usize, hook_update_all as *const () as usize];
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

/// 遊戲接著游標寫的下一段可以接成同一行：同一層、同一列、b 從 a 結尾那格開始、都靠左、a 結尾沒有兩格以上的補位空白、
/// 兩段都只有一般字元（中間夾著方框、圖形字元的不接，免得緊排後蓋到）
fn chainable(a: &Run, b: &Run) -> bool {
    let all_text = |r: &Run| r.shadow.iter().all(|&c| is_text(c));
    a.top == b.top
        && a.row == b.row
        && a.y == b.y
        && b.x0 == a.end()
        && a.align == b'l'
        && b.align == b'l'
        && a.trailing_pad() < 2
        && all_text(a)
        && all_text(b)
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
    /// [主層, 頂層]：x*dimy+y → 1 + 本幀第幾段（0：不算數）
    owner: [Vec<u32>; 2],
    dimy: i32,
    /// 主層的字貼過了（頂層開始畫之前、或 update_all 結束）
    flushed: bool,
    /// 本幀遊戲有畫頂層（top_in_use 時才會呼叫 update_top_tile）
    top_pass: bool,
    frame: u64,
    cache: HashMap<u64, Strip>,
    glyphs: Option<(i32, Glyphs<'static>)>,
    boxes: HashMap<char, Option<GlyphBox>>,
    chars: Vec<char>,
    /// 從格子讀回來、接在本幀某一行旁邊的字（見 begin）
    extra: Vec<Run>,
    /// 除錯：頂層開始前貼主層花的時間（update_all 結束時併入耗時）
    spent: std::time::Duration,
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

fn fonts() -> Option<&'static [atlas::Font]> {
    static FONTS: OnceLock<Option<&'static [atlas::Font]>> = OnceLock::new();
    *FONTS.get_or_init(|| {
        let paths: Vec<&str> = FONT_PATHS.get()?.iter().map(String::as_str).collect();
        atlas::load_fonts(&paths).ok().map(|f| &*Box::leak(f.into_boxed_slice()))
    })
}

impl State {
    /// update_all 開始：逐格核對本幀每段字的格子還是不是它寫的，再把連續的幾段接成行。
    unsafe fn begin(&mut self) {
        self.frame += 1;
        self.flushed = false;
        self.top_pass = false;
        self.lines.clear();
        self.runs = FRAME.lock().clone();
        let gps = GPS.load(Ordering::Relaxed) as *const u8;
        let (Some(l), Some(runs)) = (LAYOUT.get(), self.runs.clone()) else { return };
        let dict = DICT.load(Ordering::Acquire);
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
        for (r, run) in runs.iter().enumerate() {
            let mut glyph_at = vec![None; run.shadow.len()];
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
                extra.push(Run { top, row: Row::Whole, x0: x, y, shadow: bytes, glyphs: Vec::new(), align: b'l' });
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

    fn owner_of(&self, x: i32, y: i32, top: bool) -> u32 {
        if x < 0 || y < 0 || self.dimy <= 0 || y >= self.dimy {
            return 0;
        }
        self.owner[top as usize].get((x * self.dimy + y) as usize).copied().unwrap_or(0)
    }

    fn owns(&self, x: i32, y: i32, top: bool) -> bool {
        self.owner_of(x, y, top) != 0
    }

    unsafe fn fg(&self, gps: *const u8, x: i32, y: i32, top: bool) -> [u8; 3] {
        let idx = (x * self.dimy + y) as usize;
        unsafe { cell(gps, idx, top).map_or([255; 3], |(c, _)| [*c.add(1), *c.add(2), *c.add(3)]) }
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
            let owned: Vec<bool> = (0..line.len).map(|k| mine(self.owner_of(line.x0 + k, line.y, top))).collect();
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
            let Some((tex, ranges)) = (unsafe { self.strip(sdl, rend, line, &runs, &extra, dx, dy, mono) }) else { continue };
            // 半格位移的文字：貼圖往下移半格，上列畫貼圖的上半段、下列畫下半段（見 atlas::render 的上下列字圖）
            let (sy, h, ty) = match line.row {
                Row::Top => (0, dy - dy / 2, dy / 2),
                Row::Bottom => (dy - dy / 2, dy / 2, 0),
                _ => (0, dy, 0),
            };
            let base_x = ox + line.x0 * dx;
            let base_y = oy + line.y * dy + ty;
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
                        .find(|&x| self.owner_of(x, run.y, top) == ri as u32 + 1 && run.shadow[(x - run.x0) as usize] != b' ')
                        .or_else(|| (0..run.shadow.len() as i32).map(|k| run.x0 + k).find(|&x| self.owner_of(x, run.y, top) == ri as u32 + 1));
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

    /// 字型（依目前格高）與 glyph id → 字 的對照；第一次用時準備
    fn ready(&mut self, dy: i32) -> Option<()> {
        if self.chars.is_empty() {
            let dict = DICT.load(Ordering::Acquire);
            if dict.is_null() {
                return None;
            }
            let dict = unsafe { &*dict };
            self.chars = vec![' '; dict.glyph_of.len()];
            for (&c, &g) in &dict.glyph_of {
                self.chars[g as usize] = c;
            }
        }
        if self.glyphs.as_ref().is_none_or(|(h, _)| *h != dy) {
            let em = (dy as f32 * EM_PER_CELL).round();
            self.glyphs = Some((dy, Glyphs::with_px(fonts()?, em).ok()?));
            self.boxes.clear();
        }
        Some(())
    }

    /// 一個字的點陣（快取）；字型沒有這個字時是 None
    fn glyph_box(&mut self, c: char, dy: i32) -> Option<&GlyphBox> {
        if !self.boxes.contains_key(&c) {
            let glyphs = &self.glyphs.as_ref()?.1;
            let made = glyphs.advance(c).and_then(|adv| {
                let w = adv.ceil() as usize + 4;
                let mut mask = vec![0u8; w * dy as usize];
                let raise = -(dy as f32 * RAISE_PER_CELL).round();
                glyphs.draw(c, w, dy as usize, raise, &mut mask).ok()?.then_some(())?;
                Some(GlyphBox { mask, w, org: ((w as f32 - adv) / 2.0).round() as i32, adv })
            });
            self.boxes.insert(c, made);
        }
        self.boxes.get(&c)?.as_ref()
    }

    /// 字寬（像素）：空白至少三分之一字寬（字型的空白只有約 0.22 字寬，中英數字之間看起來黏在一起）
    fn advance(&mut self, c: char, dy: i32) -> f32 {
        let em = (dy as f32 * EM_PER_CELL).round();
        let adv = self.glyph_box(c, dy).map_or(em * 0.5, |g| g.adv);
        if c == ' ' {
            let space = self.glyphs.as_ref().and_then(|(_, g)| g.advance(' ')).unwrap_or(0.0);
            space.max(em / 3.0)
        } else {
            adv
        }
    }

    /// 一行的貼圖（快取）與每段字在貼圖裡的像素範圍。mono：沒有譯文的字每個留在原格正中間（彩色文字）
    #[allow(clippy::too_many_arguments)]
    unsafe fn strip(&mut self, sdl: &Sdl, rend: *mut c_void, line: &Line, base: &[Run], extra: &[Run], dx: i32, dy: i32, mono: bool) -> Option<(Tex, Vec<(i32, i32)>)> {
        let mut h = std::collections::hash_map::DefaultHasher::new();
        (line.kind as u8, mono, dx, dy).hash(&mut h);
        for &ri in &line.runs {
            let run = run_at(base, extra, ri);
            (&run.shadow, &run.glyphs, run.align, run.x0 - line.x0).hash(&mut h);
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
            // 每組從原本的起始格畫起，只有數字的組（數量、日期）對齊原本的結尾格，欄位的左緣、數字的右緣都不動
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
                    let numeric = group.iter().any(char::is_ascii_digit)
                        && group.iter().all(|&c| c.is_ascii_digit() || ".,-+%/:".contains(c));
                    let x = if numeric { end as f32 * dx as f32 - pen } else { start as f32 * dx as f32 };
                    placed.extend(rel.into_iter().map(|(p, c)| (x + p, c)));
                    k = end.max(start + 1);
                }
                ranges.push((0, span));
            }
            Kind::Single => {
                let run = run_at(base, extra, line.runs[0]);
                let items = run.items(&chars);
                let first = items.iter().position(|&(_, c)| c != ' ');
                let last = items.iter().rposition(|&(_, c)| c != ' ');
                if let (Some(first), Some(last)) = (first, last) {
                    let lead = items[first].0;
                    let end = items.get(last + 1).map_or(run.shadow.len(), |i| i.0);
                    let mut pen = 0f32;
                    let mut rel = Vec::new();
                    for &(_, c) in &items[first..=last] {
                        rel.push((pen, c));
                        pen += self.advance(c, dy);
                    }
                    let width = pen.ceil() as i32;
                    let start = match run.align {
                        b'c' => (span - width) / 2,
                        b'r' => end as i32 * dx - width,
                        _ => lead as i32 * dx,
                    } as f32;
                    placed.extend(rel.into_iter().map(|(x, c)| (start + x, c)));
                }
                ranges.push((0, span));
            }
            // 接成一行：從第一個非空白格起緊排；譯文補到原文寬度的空白不佔位；兩格以上的空白（欄位補位）
            // 跳到下一個字原本的格子（不往回），欄位不跑掉
            Kind::Chain => {
                // (第幾段, 格位, 字, 是譯文結尾的補位空白)
                let all: Vec<(usize, i32, char, bool)> = line
                    .runs
                    .iter()
                    .enumerate()
                    .flat_map(|(i, &ri)| {
                        let run = run_at(base, extra, ri);
                        let items = run.items(&chars);
                        let last = if run.plain() { items.len() } else { items.iter().rposition(|&(_, c)| c != ' ').map_or(0, |p| p + 1) };
                        items.into_iter().enumerate().map(move |(j, (k, c))| (i, run.x0 - line.x0 + k as i32, c, j >= last))
                    })
                    .collect();
                let mut ranges_px: Vec<(f32, f32)> = vec![(f32::MAX, f32::MIN); line.runs.len()];
                let mut pen: Option<f32> = None;
                let mut n = 0;
                while n < all.len() {
                    let (i, cell, c, _) = all[n];
                    if c == ' ' {
                        let gap = all[n..].iter().take_while(|&&(_, _, c, _)| c == ' ').count();
                        // 只剩譯文補位的空白（後面還有字）：不佔位
                        let pad_only = all[n..n + gap].iter().all(|&(_, _, _, pad)| pad) && n + gap < all.len();
                        if let (Some(p), false) = (pen.as_mut(), pad_only) {
                            let real = all[n..n + gap].iter().filter(|&&(_, _, _, pad)| !pad).count();
                            if real >= 2 {
                                if let Some(&(_, next, _, _)) = all.get(n + gap) {
                                    *p = p.max(next as f32 * dx as f32);
                                }
                            } else if real == 1 {
                                let r = &mut ranges_px[i];
                                r.0 = r.0.min(*p);
                                *p += self.advance(' ', dy);
                                r.1 = r.1.max(*p);
                            }
                        }
                        n += gap;
                        continue;
                    }
                    let p = pen.get_or_insert(cell as f32 * dx as f32);
                    let adv = self.advance(c, dy);
                    placed.push((*p, c));
                    let r = &mut ranges_px[i];
                    r.0 = r.0.min(*p);
                    *p += adv;
                    r.1 = r.1.max(*p);
                    n += 1;
                }
                ranges.extend(ranges_px.into_iter().map(|(a, b)| if a <= b { (a.floor() as i32, b.ceil() as i32) } else { (0, 0) }));
            }
        }
        let (w, hgt) = (span.max(1) as usize, dy as usize);
        let mut mask = vec![0u8; w * hgt];
        for &(x, c) in &placed {
            if c == ' ' {
                continue;
            }
            let Some(g) = self.glyph_box(c, dy) else { continue };
            let left = x.round() as i32 - g.org;
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
    if ON.load(Ordering::Relaxed) && STATE.with_borrow(|s| s.owns(x, y, false)) {
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
        s.owns(x, y, true)
    });
    if owned {
        unsafe { blank_call(r, x, y, true, orig) }
    } else {
        unsafe { orig(r, x, y) }
    }
}
