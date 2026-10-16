//! 實驗（overlay-prototype 分支）：主層的譯文改成整句一張貼圖，字距照字型，不受「一個中文字佔兩格」限制。
//!
//! DF 53.16 每幀由非同步執行緒在 render_things 裡把字寫進畫面緩衝區，主執行緒再用 renderer 的 vtable 逐格畫
//! （update_all → 每格 update_tile → 頂層 update_top_tile），兩邊交棒、不會同時跑；畫面每幀整個重畫，沒有只畫變動格。
//! 做法：
//! - 寫字時（hooks::paint）照舊貼字圖，另外記下這一幀每句譯文的位置與替身字串（record）；render_things 結束時交給主執行緒。
//! - 主執行緒 update_all 開始時逐格核對：格子還是這句寫的內容（字圖的 texpos、替身字串的字元）才算本句的格；
//!   畫這些格時先把字元換成空白、texpos 清掉，遊戲只畫底色，畫完再還原。
//! - 主層畫完（頂層開始前，或 update_all 結束）把整句的貼圖照「格子還是本句的」切段貼上，頂層照樣蓋在上面。
//!
//! 只處理主層、一般列；頂層與半格位移的分頁標籤照舊用字圖。

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
/// render_atlas 用的字型檔，主執行緒第一次畫句子時載入
pub static FONT_PATHS: OnceLock<Vec<String>> = OnceLock::new();
static GPS: AtomicUsize = AtomicUsize::new(0);

/// 字身相對格高、往上提的比例，和字圖集相同（見 atlas.rs）
const EM_PER_CELL: f32 = 0.875;
const RAISE_PER_CELL: f32 = 1.0 / 16.0;
/// 自己建的句子貼圖上限，滿了丟掉最久沒用的
const CACHE_LIMIT: usize = 2048;

struct Run {
    top: bool,
    row: Row,
    x0: i32,
    y: i32,
    shadow: Vec<u8>,
    glyphs: Vec<(usize, u32)>,
    align: u8,
}

static BUILD: Mutex<Vec<Run>> = Mutex::new(Vec::new());
static FRAME: Mutex<Option<Arc<Vec<Run>>>> = Mutex::new(None);

/// 寫字時呼叫（非同步執行緒）：一句譯文；top 是頂層，row 是一般列或半格位移的上下列。
#[allow(clippy::too_many_arguments)]
pub fn record(gps: *mut u8, top: bool, row: Row, x0: i32, y: i32, shadow: &[u8], glyphs: &[(usize, u32)], align: u8) {
    if !ON.load(Ordering::Relaxed) || glyphs.is_empty() {
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

/// 除錯：耗時，加上本幀記下的句子（頂層, x0, y, 替身字串, 字圖數）
pub fn dump() -> String {
    let t = *TIMING.lock();
    let head = format!("update_all {:.0} us, overlay {:.0} us\n", t[0], t[1]);
    let Some(runs) = FRAME.lock().clone() else { return head };
    head + &runs
        .iter()
        .map(|r| format!("{}\t{}\t{}\t[{}]\t{}\n", r.top as u8, r.x0, r.y, crate::cp437::decode(&r.shadow), r.glyphs.len()))
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

struct Strip {
    tex: Tex,
    last: u64,
}

#[derive(Default)]
struct State {
    runs: Option<Arc<Vec<Run>>>,
    /// [主層, 頂層]：x*dimy+y → 1 + 本幀第幾句（0：不是本句的格）
    owner: [Vec<u32>; 2],
    dimy: i32,
    /// 主層的句子貼過了（頂層開始畫之前、或 update_all 結束）
    flushed: bool,
    /// 本幀遊戲有畫頂層（top_in_use 時才會呼叫 update_top_tile）
    top_pass: bool,
    frame: u64,
    cache: HashMap<u64, Strip>,
    glyphs: Option<(i32, Glyphs<'static>)>,
    chars: Vec<char>,
    /// 除錯：頂層開始前貼主層句子花的時間（update_all 結束時併入耗時）
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

impl State {
    /// update_all 開始：逐格核對本幀每句譯文的格子還是不是它寫的。
    unsafe fn begin(&mut self) {
        self.frame += 1;
        self.flushed = false;
        self.top_pass = false;
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
                if x < 0 || x >= dimx || run.y < 0 || run.y >= dimy {
                    continue;
                }
                let idx = (x * dimy + run.y) as usize;
                let Some((c, t)) = (unsafe { cell(gps, idx, run.top) }) else { return };
                // 替身字串的一般字元（數字、英文、補位的空白）只核對字元：遊戲寫一般字時 texpos 會不會清成 0 沒有實測
                let mine = unsafe {
                    match glyph_at[k] {
                        Some(want) => *t == want,
                        None => *c == ch,
                    }
                };
                if mine {
                    self.owner[run.top as usize][idx] = r as u32 + 1;
                }
            }
        }
    }

    fn owns(&self, x: i32, y: i32, top: bool) -> bool {
        let idx = (x * self.dimy + y) as usize;
        self.owner[top as usize].get(idx).is_some_and(|&o| o != 0)
    }

    /// 主層畫完：每句的貼圖照「還是本句的格」切段貼上，顏色用該格的前景色。
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
        for (ri, run) in runs.iter().enumerate().filter(|(_, run)| run.top == top) {
            let me = ri as u32 + 1;
            let n = run.shadow.len() as i32;
            let mut k = 0;
            let mut segs = Vec::new();
            while k < n {
                let x = run.x0 + k;
                if !self.owns_is(x, run.y, me, top) {
                    k += 1;
                    continue;
                }
                let fg = unsafe { self.fg(gps, x, run.y, top) };
                let start = k;
                while k < n && self.owns_is(run.x0 + k, run.y, me, top) && unsafe { self.fg(gps, run.x0 + k, run.y, top) } == fg {
                    k += 1;
                }
                segs.push((start, k, fg));
            }
            if segs.is_empty() {
                continue;
            }
            let Some(tex) = (unsafe { self.strip(sdl, rend, run, dx, dy) }) else { continue };
            // 半格位移的文字：整句貼圖往下移半格，上列畫貼圖的上半段、下列畫下半段（見 atlas::render 的上下列字圖）
            let (sy, h, ty) = match run.row {
                Row::Top => (0, dy - dy / 2, dy / 2),
                Row::Bottom => (dy - dy / 2, dy / 2, 0),
                _ => (0, dy, 0),
            };
            for (a, b, [cr, cg, cb]) in segs {
                let src = Rect { x: a * dx, y: sy, w: (b - a) * dx, h };
                let dst = Rect { x: ox + (run.x0 + a) * dx, y: oy + run.y * dy + ty, w: (b - a) * dx, h };
                unsafe {
                    (sdl.color)(tex, cr, cg, cb);
                    (sdl.copy)(rend, tex, &src, &dst);
                }
            }
        }
    }

    fn owns_is(&self, x: i32, y: i32, me: u32, top: bool) -> bool {
        if x < 0 || y < 0 || self.dimy <= 0 || y >= self.dimy {
            return false;
        }
        self.owner[top as usize].get((x * self.dimy + y) as usize).is_some_and(|&o| o == me)
    }

    unsafe fn fg(&self, gps: *const u8, x: i32, y: i32, top: bool) -> [u8; 3] {
        let idx = (x * self.dimy + y) as usize;
        unsafe { cell(gps, idx, top).map_or([255; 3], |(c, _)| [*c.add(1), *c.add(2), *c.add(3)]) }
    }

    /// 一句的貼圖：寬度是替身字串的格數，字照字型的字寬緊排；依譯文的對齊置中、靠右，其他從第一個非空白格起。
    unsafe fn strip(&mut self, sdl: &Sdl, rend: *mut c_void, run: &Run, dx: i32, dy: i32) -> Option<Tex> {
        let mut h = std::collections::hash_map::DefaultHasher::new();
        (&run.shadow, &run.glyphs, run.align, dx, dy).hash(&mut h);
        let key = h.finish();
        let frame = self.frame;
        if let Some(s) = self.cache.get_mut(&key) {
            s.last = frame;
            return Some(s.tex);
        }
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
            let fonts: &'static [atlas::Font] = {
                static FONTS: OnceLock<Option<&'static [atlas::Font]>> = OnceLock::new();
                (*FONTS.get_or_init(|| {
                    let paths: Vec<&str> = FONT_PATHS.get()?.iter().map(String::as_str).collect();
                    atlas::load_fonts(&paths).ok().map(|f| &*Box::leak(f.into_boxed_slice()))
                }))?
            };
            let em = (dy as f32 * EM_PER_CELL).round();
            self.glyphs = Some((dy, Glyphs::with_px(fonts, em).ok()?));
        }
        let glyphs = &self.glyphs.as_ref()?.1;

        // 每格的字：(起始格, 字)。句型拼出的譯文字圖不一定依格位排序，先依格位建表
        let mut at = vec![None; run.shadow.len()];
        for &(off, g) in &run.glyphs {
            if let Some(slot) = at.get_mut(off) {
                *slot = Some(g);
            }
        }
        let mut items: Vec<(usize, char)> = Vec::new();
        let mut k = 0;
        while k < run.shadow.len() {
            if let Some(g) = at[k] {
                items.push((k, self.chars.get(g as usize).copied().unwrap_or('?')));
                k += 2;
            } else {
                items.push((k, crate::cp437::TABLE[run.shadow[k] as usize]));
                k += 1;
            }
        }
        let first = items.iter().position(|&(_, c)| c != ' ')?;
        let last = items.iter().rposition(|&(_, c)| c != ' ')?;
        let lead = items[first].0;
        let end = items.get(last + 1).map_or(run.shadow.len(), |i| i.0);
        let em = (dy as f32 * EM_PER_CELL).round();
        let mut pen = 0f32;
        let mut placed = Vec::new();
        for &(_, c) in &items[first..=last] {
            // 字型的空白只有約 0.22 字寬，中英數字之間看起來黏在一起（「高度1」），至少給三分之一字寬
            let adv = glyphs.advance(c).unwrap_or(em * 0.5);
            let adv = if c == ' ' { adv.max(em / 3.0) } else { adv };
            placed.push((pen, c, adv));
            pen += adv;
        }
        let span = run.shadow.len() as i32 * dx;
        let width = pen.ceil() as i32;
        let start = match run.align {
            b'c' => (span - width) / 2,
            b'r' => end as i32 * dx - width,
            _ => lead as i32 * dx,
        };
        let (w, hgt) = (span as usize, dy as usize);
        let mut mask = vec![0u8; w * hgt];
        let raise = -(dy as f32 * RAISE_PER_CELL).round();
        for (x, c, adv) in placed {
            if c == ' ' {
                continue;
            }
            let box_w = adv.ceil() as usize + 4;
            let mut tmp = vec![0u8; box_w * hgt];
            if !glyphs.draw(c, box_w, hgt, raise, &mut tmp).ok()? {
                continue;
            }
            let left = (start as f32 + x - ((box_w as f32 - adv) / 2.0).round()).round() as i32;
            for yy in 0..hgt {
                for xx in 0..box_w {
                    let tx = left + xx as i32;
                    if tx < 0 || tx >= w as i32 {
                        continue;
                    }
                    let d = &mut mask[yy * w + tx as usize];
                    *d = (*d).max(tmp[yy * box_w + xx]);
                }
            }
        }
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
        self.cache.insert(key, Strip { tex, last: frame });
        Some(tex)
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
    let ours = ours + STATE.with_borrow_mut(|s| {
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

/// 本句的格先畫成只有底色：字元換成空白、texpos 清掉，畫完還原（非同步執行緒這時在等，不會同時寫）
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
