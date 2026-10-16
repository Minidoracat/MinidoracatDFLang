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
mod metrics;
mod mtb;
pub mod names;
mod overlay;
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
        lua::set_fn(l, c"set_name_layout", set_name_layout);
        lua::set_fn(l, c"load_names", load_names);
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
        lua::set_fn(l, c"dh_translate", dh_translate);
        lua::set_fn(l, c"dh_stamp", dh_stamp);
        lua::set_fn(l, c"dh_wrap", dh_wrap);
        lua::set_fn(l, c"overlay_install", overlay_install);
        lua::set_fn(l, c"set_overlay", set_overlay);
        lua::set_fn(l, c"overlay_dump", overlay_dump);
        lua::set_fn(l, c"dh_plain", dh_plain);
        lua::set_fn(l, c"english_at", english_at);
    }
    1
}
/// overlay_dump()：除錯，整句貼圖的耗時與本幀記下的字（見 overlay::dump）
unsafe extern "C" fn overlay_dump(l: lua::State) -> c_int {
    unsafe { lua::push_str(l, &overlay::dump()) };
    1
}

/// dh_plain(gps 位址, x, y, 頂層 0/1, 字)：DFHack 照原樣畫的一段字，交給整句貼圖（見 hooks::dfhack_plain）
unsafe extern "C" fn dh_plain(l: lua::State) -> c_int {
    unsafe {
        if let (Some(gps), Some(x), Some(y), Some(top), Some(text)) =
            (lua::arg_int(l, 1), lua::arg_int(l, 2), lua::arg_int(l, 3), lua::arg_int(l, 4), lua::arg_bytes(l, 5))
        {
            hooks::dfhack_plain(gps as usize, x as i32, y as i32, top != 0, text);
        }
    }
    0
}

/// english_at(x, y)：上一幀 (x, y) 翻譯前的英文字元（CP437 位元組），沒翻譯是 nil（見 hooks::English）
unsafe extern "C" fn english_at(l: lua::State) -> c_int {
    unsafe {
        let ch = match (lua::arg_int(l, 1), lua::arg_int(l, 2)) {
            (Some(x), Some(y)) => hooks::ENGLISH.lock().at(x as i32, y as i32),
            _ => None,
        };
        match ch {
            Some(b) => lua::push_int(l, b as i64),
            None => lua::push_nil(l),
        }
    }
    1
}

/// overlay_install(renderer 位址)：整句貼圖，換掉 renderer vtable 的四個槽（見 overlay.rs）
unsafe extern "C" fn overlay_install(l: lua::State) -> c_int {
    unsafe {
        let Some(addr) = lua::arg_int(l, 1) else { return fail(l, "overlay_install: expects the renderer address") };
        match overlay::install(addr as usize) {
            Ok(()) => ok(l),
            Err(e) => fail(l, &e),
        }
    }
}

/// set_overlay(1 或 0)：整句貼圖開關，遊戲啟動時（啟用引擎前）呼叫一次：段落換行與替身字串也跟著改用字型的實際字寬（見 metrics.rs）
unsafe extern "C" fn set_overlay(l: lua::State) -> c_int {
    unsafe {
        let on = lua::arg_int(l, 1).is_some_and(|v| v != 0);
        overlay::ON.store(on, Ordering::Release);
        metrics::set_compact(on);
        ok(l)
    }
}

/// Lua 慣例的失敗回傳：nil, 訊息。
unsafe fn fail(l: lua::State, msg: &str) -> c_int {
    unsafe {
        lua::push_nil(l);
        lua::push_str(l, msg);
    }
    2
}

/// dh_translate(text, y[, 置中 1])：DFHack 要畫的一段字（CP437）有譯文就回傳影子字串，否則 nil（見 hooks::dfhack_translate）。
unsafe extern "C" fn dh_translate(l: lua::State) -> c_int {
    unsafe {
        let y = lua::arg_int(l, 2).unwrap_or(-1) as i32;
        let center = lua::arg_int(l, 3).is_some_and(|c| c != 0);
        match lua::arg_bytes(l, 1).and_then(|text| hooks::dfhack_translate(text, y, center)) {
            Some(shadow) => lua::push_bytes(l, &shadow),
            None => lua::push_nil(l),
        }
        1
    }
}

/// dh_stamp(gps 位址, x, y, 頂層 0/1)：貼上 dh_translate 查到的字圖（見 hooks::dfhack_stamp）。
unsafe extern "C" fn dh_stamp(l: lua::State) -> c_int {
    unsafe {
        if let (Some(gps), Some(x), Some(y), Some(top)) = (lua::arg_int(l, 1), lua::arg_int(l, 2), lua::arg_int(l, 3), lua::arg_int(l, 4)) {
            hooks::dfhack_stamp(gps as usize, x as i32, y as i32, top != 0);
        }
        0
    }
}

/// dh_wrap(整段英文, 寬幾格, {英文行...})：DFHack WrappedLabel 的段落有譯文就登記每一行要畫的中文（見 hooks::dfhack_wrap）。
unsafe extern "C" fn dh_wrap(l: lua::State) -> c_int {
    unsafe {
        let ok = match (lua::arg_bytes(l, 1).map(<[u8]>::to_vec), lua::arg_int(l, 2), lua::arg_type(l, 3) == lua::TTABLE) {
            (Some(text), Some(width), true) => {
                let lines: Option<Vec<Vec<u8>>> = (1..=lua::len(l, 3) as i64).map(|i| lua::index_bytes(l, 3, i)).collect();
                lines.is_some_and(|lines| hooks::dfhack_wrap(&text, width.max(0) as usize, &lines))
            }
            _ => false,
        };
        lua::push_bool(l, ok);
        1
    }
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
                    lower: f(c"screentexpos_lower")?,
                },
                top: Layer {
                    screen: f(c"screen_top")?,
                    limit: f(c"screen_top_limit")?,
                    texpos: f(c"screentexpos_top")?,
                    flag: f(c"screentexpos_top_flag")?,
                    lower: f(c"screentexpos_top_lower")?,
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

/// set_name_layout{words, parts = language_name 欄位位移, word_vec = world.raws.language.words 的位址,
/// word_token, word_forms = language_word 欄位位移}（世界生成名稱，見 names.rs）
unsafe extern "C" fn set_name_layout(l: lua::State) -> c_int {
    unsafe {
        if lua::arg_type(l, 1) != lua::TTABLE {
            return fail(l, "set_name_layout expects a table");
        }
        let f = |name: &CStr| lua::field_int(l, 1, name).map(|v| v as usize);
        match (f(c"words"), f(c"parts"), f(c"word_vec"), f(c"word_token"), f(c"word_forms")) {
            (Some(words), Some(parts), Some(word_vec), Some(word_token), Some(word_forms)) if word_vec != 0 => {
                let _ = names::NAME_LAYOUT.set(names::NameLayout { words, parts, word_vec, word_token, word_forms });
                ok(l)
            }
            _ => fail(l, "set_name_layout: missing or non-integer field"),
        }
    }
}

/// load_names(路徑) → 載入條數（世界生成名稱的字表：代號<TAB>詞性<TAB>中文）
unsafe extern "C" fn load_names(l: lua::State) -> c_int {
    unsafe {
        let Some(path) = lua::arg_str(l, 1) else { return fail(l, "load_names: expects a path") };
        match std::fs::read_to_string(path) {
            Ok(content) => {
                lua::push_int(l, names::load_words(&content) as i64);
                1
            }
            Err(e) => fail(l, &format!("load_names: {e}")),
        }
    }
}

/// render_atlas(輸出 PNG, 格寬, 格高, 字串, {字型路徑...}, 整句貼圖字身?) → 貼圖數, 沒有字型能畫的字
/// 字身是十六分之幾格高（13～16，dflang overlay size），省略或超出範圍照預設（見 overlay::set_em）
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
        let _ = overlay::FONT_PATHS.set(paths.iter().map(|p| p.to_string()).collect());
        if let Some(em) = lua::arg_int(l, 6).and_then(|v| u8::try_from(v).ok()) {
            overlay::set_em(em);
        }
        let result = atlas::load_fonts(&paths).and_then(|fonts| {
            let chars: Vec<char> = chars.chars().collect();
            let report = atlas::render(&fonts, &chars, cell_w as usize, cell_h as usize, std::path::Path::new(out))?;
            // 段落照字型實際字寬換行用（整句貼圖開著時，見 metrics.rs）：字圖集的字、英數與 CP437 的拉丁字母
            let latin = (0x20u8..=0xff).filter(|&b| overlay::is_text(b)).map(|b| cp437::TABLE[b as usize]);
            metrics::set_advances(atlas::advances(&fonts, chars.iter().copied().chain(latin), cell_w as usize, cell_h as usize, overlay::em_per_cell())?);
            Ok(report)
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

/// set_glyphs(字串, {texpos...})：字圖集裡依序的字，與每字八張貼圖的 texpos（左右兩格、半格位移的上下列、錨定標籤用；見 atlas::render）。
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
            return fail(l, "set_glyphs: texpos count must be 8 times the glyph count");
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

/// status() → {enabled, hooks, entries, rules, glyphs, wraps, hits, misses,
///   misses_logging, misses_logged, misses_full（未翻譯紀錄：在記嗎、檔案裡幾筆、到上限了沒）,
///   cache_rules, cache_names, cache_mtb_words, cache_mtb_paragraphs, cache_wrap_lines, cache_wrap_plans,
///   cache_ctb_lines, cache_ctb_plans, cache_dfhack_lines（各快取目前筆數）,
///   textures（整句貼圖快取的貼圖張數）, overlay_runs（最後一幀收到的字段）, overlay_pending（還沒收完的字段，翻譯關著時應為 0）}
unsafe extern "C" fn status(l: lua::State) -> c_int {
    unsafe {
        let dict = hooks::DICT.load(Ordering::Acquire);
        let (entries, rules, glyphs, cache_rules) = if dict.is_null() {
            (0, 0, 0, 0)
        } else {
            ((*dict).entries.len(), (*dict).rules.len(), (*dict).glyph_of.len(), (*dict).cache_len())
        };
        let (logging, logged, full) = hooks::miss_log_state();
        let (mtb_words, mtb_paragraphs) = mtb::counts();
        let (wrap_lines, wrap_plans) = lines::counts();
        let (ctb_lines, ctb_plans) = ctb::counts();
        let (overlay_runs, overlay_pending) = overlay::run_counts();
        lua::new_table(l);
        for (name, v) in [(c"enabled", hooks::ENABLED.load(Ordering::Acquire)), (c"misses_logging", logging), (c"misses_full", full)] {
            lua::push_bool(l, v);
            lua::set_field(l, name);
        }
        for (name, v) in [
            (c"hooks", hooks::hook_count()),
            (c"entries", entries),
            (c"rules", rules),
            (c"glyphs", glyphs),
            (c"wraps", STATIC_WRAPS.load(Ordering::Relaxed)),
            (c"hits", hooks::HITS.load(Ordering::Relaxed) as usize),
            (c"misses", hooks::MISSES.load(Ordering::Relaxed) as usize),
            (c"misses_logged", logged),
            (c"cache_rules", cache_rules),
            (c"cache_names", names::count()),
            (c"cache_mtb_words", mtb_words),
            (c"cache_mtb_paragraphs", mtb_paragraphs),
            (c"cache_wrap_lines", wrap_lines),
            (c"cache_wrap_plans", wrap_plans),
            (c"cache_ctb_lines", ctb_lines),
            (c"cache_ctb_plans", ctb_plans),
            (c"cache_dfhack_lines", hooks::dh_lines_len()),
            (c"textures", overlay::texture_count()),
            (c"overlay_runs", overlay_runs),
            (c"overlay_pending", overlay_pending),
        ] {
            lua::push_int(l, v as i64);
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
