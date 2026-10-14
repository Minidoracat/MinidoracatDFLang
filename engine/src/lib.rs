//! DFLang：矮人要塞中文化引擎。
//!
//! DFHack 的 Lua 以 `package.loadlib(dll, "luaopen_dflang")` 載入，回傳一張函式表；
//! 不連結 dfhack.dll，所以不綁 DFHack 版本，也不依賴其他 MOD。載入順序（見 lua/loader.lua）：
//! set_layout／set_mtb_layout／set_ctb_layout／set_fonts → render_atlas → set_glyphs → load／load_rules／load_screens
//! → scan/attach → enable（enable 時補登記遊戲啟動時就切好的提示文字，見 lines.rs）；之後每幀 set_focus（依畫面套用的譯文用）。
//! dict、ctb 也給 examples/t.rs（離線查譯文）用。

mod atlas;
pub mod cp437;
pub mod ctb;
pub mod dict;
mod hooks;
mod image;
mod lines;
mod lua;
mod mtb;
mod scan;

use std::ffi::{c_int, CStr};
use std::sync::atomic::{AtomicUsize, Ordering};

use parking_lot::Mutex;

use dict::Dict;
use hooks::{Layer, Layout};

/// 載入中、尚未啟用的字典；enable 時才交給 hook。
static STAGING: Mutex<Option<Dict>> = Mutex::new(None);
/// 遊戲啟動時就切好、而且有譯文的提示句數（status 顯示）
static STATIC_WRAPS: AtomicUsize = AtomicUsize::new(0);

#[unsafe(no_mangle)]
pub unsafe extern "C" fn luaopen_dflang(l: lua::State) -> c_int {
    if !lua::init() {
        return 0;
    }
    unsafe {
        lua::new_table(l);
        lua::set_fn(l, c"set_layout", set_layout);
        lua::set_fn(l, c"set_mtb_layout", set_mtb_layout);
        lua::set_fn(l, c"set_ctb_layout", set_ctb_layout);
        lua::set_fn(l, c"render_atlas", render_atlas);
        lua::set_fn(l, c"render_mascot", render_mascot);
        lua::set_fn(l, c"set_glyphs", set_glyphs);
        lua::set_fn(l, c"set_fonts", set_fonts);
        lua::set_fn(l, c"set_focus", set_focus);
        lua::set_fn(l, c"load", load);
        lua::set_fn(l, c"load_rules", load_rules);
        lua::set_fn(l, c"load_screens", load_screens);
        lua::set_fn(l, c"scan", scan);
        lua::set_fn(l, c"attach", attach);
        lua::set_fn(l, c"enable", enable);
        lua::set_fn(l, c"disable", disable);
        lua::set_fn(l, c"status", status);
        lua::set_fn(l, c"translate", translate);
        lua::set_fn(l, c"log_misses", log_misses);
        lua::set_fn(l, c"update_texpos", update_texpos);
    }
    1
}

/// Lua 慣例的失敗回傳：nil, 訊息。
unsafe fn fail(l: lua::State, msg: &str) -> c_int {
    unsafe {
        lua::push_nil(l);
        lua::push_str(l, msg);
    }
    2
}

unsafe fn ok(l: lua::State) -> c_int {
    unsafe { lua::push_bool(l, true) };
    1
}

/// set_layout{gps 欄位名 = 相對 gps 的位移, ...}
unsafe extern "C" fn set_layout(l: lua::State) -> c_int {
    unsafe {
        if lua::arg_type(l, 1) != lua::TTABLE {
            return fail(l, "set_layout expects a table");
        }
        let f = |name: &CStr| lua::field_int(l, 1, name).map(|v| v as usize);
        let layout = (|| {
            Some(Layout {
                screenx: f(c"screenx")?,
                screeny: f(c"screeny")?,
                dimx: f(c"dimx")?,
                dimy: f(c"dimy")?,
                clipx: f(c"clipx")?,
                clipy: f(c"clipy")?,
                main: Layer {
                    screen: f(c"screen")?,
                    limit: f(c"screen_limit")?,
                    texpos: f(c"screentexpos")?,
                    flag: f(c"screentexpos_flag")?,
                },
                top: Layer {
                    screen: f(c"screen_top")?,
                    limit: f(c"screen_top_limit")?,
                    texpos: f(c"screentexpos_top")?,
                    flag: f(c"screentexpos_top_flag")?,
                },
                anchored: f(c"screentexpos_anchored")?,
                color: [f(c"screen_color_r")?, f(c"screen_color_g")?, f(c"screen_color_b")?],
            })
        })();
        match layout {
            Some(layout) => {
                let _ = hooks::LAYOUT.set(layout);
                ok(l)
            }
            None => fail(l, "set_layout: missing or non-integer field"),
        }
    }
}

/// set_mtb_layout{markup_text_boxst／markup_text_wordst 欄位名 = 位移, ...}（段落翻譯用，見 mtb.rs）
unsafe extern "C" fn set_mtb_layout(l: lua::State) -> c_int {
    unsafe {
        if lua::arg_type(l, 1) != lua::TTABLE {
            return fail(l, "set_mtb_layout expects a table");
        }
        let f = |name: &CStr| lua::field_int(l, 1, name).map(|v| v as usize);
        let layout = (|| {
            Some(mtb::MtbLayout {
                palette: f(c"palette")?,
                words: f(c"word")?,
                current_width: f(c"current_width")?,
                word_str: f(c"str")?,
                word_red: f(c"red")?,
                word_link: f(c"link_index")?,
                word_px: f(c"px")?,
                word_py: f(c"py")?,
                word_flags: f(c"flags")?,
            })
        })();
        match layout {
            Some(layout) => {
                let _ = mtb::MTB_LAYOUT.set(layout);
                ok(l)
            }
            None => fail(l, "set_mtb_layout: missing or non-integer field"),
        }
    }
}

/// set_ctb_layout{width = color_text_boxst.width 位移, color = color_text_linest.color 位移}（彩色文字框，見 ctb.rs）
unsafe extern "C" fn set_ctb_layout(l: lua::State) -> c_int {
    unsafe {
        if lua::arg_type(l, 1) != lua::TTABLE {
            return fail(l, "set_ctb_layout expects a table");
        }
        let f = |name: &CStr| lua::field_int(l, 1, name).map(|v| v as usize);
        match (f(c"width"), f(c"color")) {
            (Some(width), Some(color)) => {
                let _ = ctb::CTB_LAYOUT.set(ctb::CtbLayout { width, color });
                ok(l)
            }
            _ => fail(l, "set_ctb_layout: missing or non-integer field"),
        }
    }
}

/// render_atlas(輸出 PNG, 格寬, 格高, 字串, {字型路徑...}) → 貼圖數, 沒有字型能畫的字
unsafe extern "C" fn render_atlas(l: lua::State) -> c_int {
    unsafe {
        let (Some(out), Some(cell_w), Some(cell_h), Some(chars)) =
            (lua::arg_str(l, 1), lua::arg_int(l, 2), lua::arg_int(l, 3), lua::arg_str(l, 4))
        else {
            return fail(l, "render_atlas: expects (png, cell_w, cell_h, chars, fonts)");
        };
        if lua::arg_type(l, 5) != lua::TTABLE || cell_w < 1 || cell_h < 1 {
            return fail(l, "render_atlas: bad cell size or font list");
        }
        let paths: Option<Vec<String>> = (1..=lua::len(l, 5) as i64).map(|i| lua::index_str(l, 5, i)).collect();
        let Some(paths) = paths else { return fail(l, "render_atlas: font paths must be strings") };
        let paths: Vec<&str> = paths.iter().map(String::as_str).collect();
        let result = atlas::load_fonts(&paths).and_then(|fonts| {
            let chars: Vec<char> = chars.chars().collect();
            atlas::render(&fonts, &chars, cell_w as usize, cell_h as usize, std::path::Path::new(out))
        });
        match result {
            Ok(report) => {
                lua::push_int(l, report.tiles as i64);
                lua::push_str(l, &report.missing.iter().collect::<String>());
                2
            }
            Err(e) => fail(l, &e),
        }
    }
}

/// render_mascot(來源 PNG, 輸出 PNG, 格寬, 格高, 高幾格, 語言標籤, 作者名, {字型路徑...}) → 寬幾格
/// 標題畫面吉祥物：縮成整格大小，底部疊上名牌（名牌文字用和字圖集相同的 hinting 點陣化）。
unsafe extern "C" fn render_mascot(l: lua::State) -> c_int {
    unsafe {
        let (Some(src), Some(out), Some(cell_w), Some(cell_h), Some(rows), Some(label), Some(credit)) = (
            lua::arg_str(l, 1),
            lua::arg_str(l, 2),
            lua::arg_int(l, 3),
            lua::arg_int(l, 4),
            lua::arg_int(l, 5),
            lua::arg_str(l, 6),
            lua::arg_str(l, 7),
        ) else {
            return fail(l, "render_mascot: expects (src, out, cell_w, cell_h, rows, label, credit, fonts)");
        };
        if lua::arg_type(l, 8) != lua::TTABLE || cell_w < 1 || cell_h < 1 || rows < 2 {
            return fail(l, "render_mascot: bad cell size, rows or font list");
        }
        let paths: Option<Vec<String>> = (1..=lua::len(l, 8) as i64).map(|i| lua::index_str(l, 8, i)).collect();
        let Some(paths) = paths else { return fail(l, "render_mascot: font paths must be strings") };
        let paths: Vec<&str> = paths.iter().map(String::as_str).collect();
        let (src, out) = (std::path::Path::new(src), std::path::Path::new(out));
        let result = atlas::load_fonts(&paths).and_then(|fonts| {
            image::mascot(src, out, cell_w as usize, cell_h as usize, rows as usize, label, credit, &fonts)
        });
        match result {
            Ok(cols) => {
                lua::push_int(l, cols as i64);
                1
            }
            Err(e) => fail(l, &e),
        }
    }
}

/// set_glyphs(字串, {texpos...})：字圖集裡依序的字，與每字左右兩格的 texpos。
unsafe extern "C" fn set_glyphs(l: lua::State) -> c_int {
    unsafe {
        let Some(chars) = lua::arg_str(l, 1) else { return fail(l, "set_glyphs: arg 1 must be a UTF-8 string") };
        if lua::arg_type(l, 2) != lua::TTABLE {
            return fail(l, "set_glyphs: arg 2 must be a table");
        }
        let n = lua::len(l, 2);
        let mut texpos = Vec::with_capacity(n);
        for i in 1..=n as i64 {
            match lua::index_int(l, 2, i) {
                Some(v) => texpos.push(v as i32),
                None => return fail(l, "set_glyphs: texpos must be integers"),
            }
        }
        let mut staging = STAGING.lock();
        let dict = staging.get_or_insert_with(Dict::default);
        if !dict.set_glyphs(chars, texpos) {
            return fail(l, "set_glyphs: texpos count must be twice the glyph count");
        }
        lua::push_int(l, dict.glyph_of.len() as i64);
        1
    }
}

/// load(路徑) → 載入條數, 原文無法用 CP437 表示的條數, 缺字圖的字數
unsafe extern "C" fn load(l: lua::State) -> c_int {
    unsafe { load_with(l, "load", Dict::load_tsv) }
}

/// load_rules(路徑) → 載入條數, 寫法不合規定的條數, 缺字圖的字數（句型規則）
unsafe extern "C" fn load_rules(l: lua::State) -> c_int {
    unsafe { load_with(l, "load_rules", Dict::load_rules_tsv) }
}

/// load_screens(路徑) → 載入條數, 原文無法用 CP437 表示的條數, 缺字圖的字數（依畫面套用的整句）
unsafe extern "C" fn load_screens(l: lua::State) -> c_int {
    unsafe { load_with(l, "load_screens", Dict::load_screens_tsv) }
}

/// set_fonts(small_font_texpos 位址, large_font_texpos 位址)：遊戲字型表，翻譯逐字畫成貼圖的分頁標籤用
unsafe extern "C" fn set_fonts(l: lua::State) -> c_int {
    unsafe {
        let (Some(small), Some(large)) = (lua::arg_int(l, 1), lua::arg_int(l, 2)) else {
            return fail(l, "set_fonts: expects two addresses");
        };
        if small == 0 || large == 0 {
            return fail(l, "set_fonts: null address");
        }
        let _ = hooks::FONTS.set([small as usize, large as usize]);
        ok(l)
    }
}

/// set_focus(焦點字串)：DFHack 目前的焦點（多個以換行分隔），Lua 每幀呼叫
unsafe extern "C" fn set_focus(l: lua::State) -> c_int {
    unsafe {
        let Some(focus) = lua::arg_str(l, 1) else { return fail(l, "set_focus: expects a string") };
        let mut current = hooks::FOCUS.lock();
        if *current != focus {
            current.clear();
            current.push_str(focus);
        }
        ok(l)
    }
}

unsafe fn load_with(l: lua::State, name: &str, parse: fn(&mut Dict, &str) -> dict::LoadReport) -> c_int {
    unsafe {
        let Some(path) = lua::arg_str(l, 1) else { return fail(l, &format!("{name}: path must be a string")) };
        let content = match std::fs::read_to_string(path) {
            Ok(c) => c,
            Err(e) => return fail(l, &format!("{name} {path}: {e}")),
        };
        let mut staging = STAGING.lock();
        let Some(dict) = staging.as_mut().filter(|d| !d.glyph_of.is_empty()) else {
            return fail(l, &format!("{name}: call set_glyphs first"));
        };
        let report = parse(dict, &content);
        lua::push_int(l, report.loaded as i64);
        lua::push_int(l, report.bad_source as i64);
        lua::push_int(l, report.missing_glyphs as i64);
        3
    }
}

/// scan(特徵碼) → 位址或 nil, 命中數（最多 2）
unsafe extern "C" fn scan(l: lua::State) -> c_int {
    unsafe {
        let Some(pattern) = lua::arg_str(l, 1).and_then(scan::parse) else { return fail(l, "scan: bad pattern") };
        let (addr, hits) = scan::find(&pattern);
        match addr {
            Some(a) => lua::push_int(l, a as i64),
            None => lua::push_nil(l),
        }
        lua::push_int(l, hits as i64);
        2
    }
}

/// attach(hook 名稱, 位址)
unsafe extern "C" fn attach(l: lua::State) -> c_int {
    unsafe {
        let (Some(name), Some(addr)) = (lua::arg_str(l, 1), lua::arg_int(l, 2)) else {
            return fail(l, "attach: expects (name, address)");
        };
        match hooks::attach(name, addr as usize) {
            Ok(()) => ok(l),
            Err(e) => fail(l, &format!("attach {name}: {e}")),
        }
    }
}

unsafe extern "C" fn enable(l: lua::State) -> c_int {
    unsafe {
        if hooks::LAYOUT.get().is_none() {
            return fail(l, "enable: call set_layout first");
        }
        if let Some(dict) = STAGING.lock().take() {
            // 舊字典刻意不釋放：遊戲執行緒可能正在讀它；重新載入是少見的開發操作
            hooks::DICT.swap(Box::into_raw(Box::new(dict)), Ordering::AcqRel);
        }
        let dict = hooks::DICT.load(Ordering::Acquire);
        if dict.is_null() {
            return fail(l, "enable: no dictionary loaded");
        }
        if let Some(&wrap_text) = hooks::WRAP_TEXT.get() {
            STATIC_WRAPS.store(lines::plan_static(wrap_text, &*dict), Ordering::Relaxed);
        }
        match hooks::set_enabled(true) {
            Ok(()) => ok(l),
            Err(e) => fail(l, &e),
        }
    }
}

unsafe extern "C" fn disable(l: lua::State) -> c_int {
    unsafe {
        match hooks::set_enabled(false) {
            Ok(()) => ok(l),
            Err(e) => fail(l, &e),
        }
    }
}

/// status() → {enabled, hooks, entries, rules, glyphs, wraps, hits, misses}
unsafe extern "C" fn status(l: lua::State) -> c_int {
    unsafe {
        let dict = hooks::DICT.load(Ordering::Acquire);
        let (entries, rules, glyphs) =
            if dict.is_null() { (0, 0, 0) } else { ((*dict).entries.len(), (*dict).rules.len(), (*dict).glyph_of.len()) };
        lua::new_table(l);
        lua::push_bool(l, hooks::ENABLED.load(Ordering::Acquire));
        lua::set_field(l, c"enabled");
        for (name, v) in [
            (c"hooks", hooks::hook_count() as i64),
            (c"entries", entries as i64),
            (c"rules", rules as i64),
            (c"glyphs", glyphs as i64),
            (c"wraps", STATIC_WRAPS.load(Ordering::Relaxed) as i64),
            (c"hits", hooks::HITS.load(Ordering::Relaxed) as i64),
            (c"misses", hooks::MISSES.load(Ordering::Relaxed) as i64),
        ] {
            lua::push_int(l, v);
            lua::set_field(l, name);
        }
        1
    }
}

/// translate(英文原文) → 譯文或 nil（查已啟用的字典：整句、句型，再逐句；可含換色標記，照彩色文字框處理）
unsafe extern "C" fn translate(l: lua::State) -> c_int {
    unsafe {
        let dict = hooks::DICT.load(Ordering::Acquire);
        let text = lua::arg_str(l, 1)
            .and_then(|s| Some((s.contains("[C:"), cp437::encode(s)?)))
            .filter(|_| !dict.is_null())
            .and_then(|(colored, key)| {
                let zh = ctb::translate(&key, 7, &*dict).ok()?;
                Some(if colored { zh } else { ctb::strip_colors(&zh) })
            });
        match text {
            Some(t) => lua::push_str(l, &t),
            None => lua::push_nil(l),
        }
        1
    }
}

/// log_misses(路徑或 nil)：把遊戲畫過、字典沒有的字串（去重）追加到檔案
unsafe extern "C" fn log_misses(l: lua::State) -> c_int {
    unsafe {
        let path = lua::arg_str(l, 1);
        match hooks::log_misses(path) {
            Ok(()) => ok(l),
            Err(e) => fail(l, &e.to_string()),
        }
    }
}

/// update_texpos({texpos...})：DFHack 重新註冊動態範圍的貼圖後，把新 texpos 寫回已啟用的字典。
/// Lua 端在遊戲狀態切換時呼叫，那時遊戲執行緒停在 DFHack 的更新點，不會同時畫字。
unsafe extern "C" fn update_texpos(l: lua::State) -> c_int {
    unsafe {
        let dict = hooks::DICT.load(Ordering::Acquire);
        if dict.is_null() || lua::arg_type(l, 1) != lua::TTABLE {
            return fail(l, "update_texpos: engine not enabled or arg is not a table");
        }
        let texpos = &mut (&mut *dict).texpos;
        if lua::len(l, 1) != texpos.len() {
            return fail(l, "update_texpos: texpos count changed");
        }
        for (i, slot) in texpos.iter_mut().enumerate() {
            match lua::index_int(l, 1, i as i64 + 1) {
                Some(v) => *slot = v as i32,
                None => return fail(l, "update_texpos: texpos must be integers"),
            }
        }
        ok(l)
    }
}
