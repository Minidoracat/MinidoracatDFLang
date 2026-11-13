//! 遊戲自己的輸入框（傳說清單的篩選、庫存搜尋、命名、數量）裡玩家打的字照原樣畫：打出字典裡有的字（Gallows、Gems）時，
//! 輸入框不能變成中文。整段和輸入框的字相同才照原文，所以清單裡剛好和打的字同字的那一列也照英文。
//!
//! 輸入法打的中文字：遊戲把 SDL 送來的 UTF-8 一個位元組一個位元組放進輸入框（見 hooks.rs 的 hook_text_input）。
//! 搜尋／篩選框收整個字（放不下的字整個不收、退格一次刪一個字），畫的時候照 UTF-8 畫成中文，比對清單時用譯文比（見 pinyin.rs）；
//! 命名框（倉儲區、小隊、暱稱、存檔名，字會進存檔）只收 ASCII，不讓中文位元組寫進存檔。

use parking_lot::Mutex;
use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};

/// 遊戲全域物件 main_interface 裡輸入框的 std::string 位址：Lua 啟動時從 DFHack 的結構定義找好交來（set_fields），整局不變
static FIELDS: Mutex<Vec<usize>> = Mutex::new(Vec::new());
/// Lua 每幀送來的字：畫面物件裡的輸入框（傳說清單的篩選）畫面關掉後位址就無效，所以送內容不送位址
static EXTRA: Mutex<Vec<u8>> = Mutex::new(Vec::new());
/// 這一幀輸入框裡的字（去掉頭尾空白，不含空的）
static CURRENT: Mutex<Vec<Vec<u8>>> = Mutex::new(Vec::new());
/// CURRENT 有沒有東西：平常輸入框都是空的，畫字時不必上鎖
static ANY: AtomicBool = AtomicBool::new(false);
/// 最後一次打字的輸入框裡的字（遊戲的打字函式改完就記，見 hooks.rs 的 hook_text_input；原樣，不轉小寫）：搜尋框打字後馬上重新比對清單，
/// 那時還沒到下一幀，CURRENT 還是舊的
static LIVE: Mutex<Vec<u8>> = Mutex::new(Vec::new());
/// LIVE 有輸入法打的中文：畫字時也拿 LIVE 比（遊戲的 widgets::textbox 不在 FIELDS 裡；英文照舊只比 FIELDS 與 EXTRA，
/// 關掉的框留下的英文字不會讓別處同字的字串照英文畫）
static LIVE_ZH: AtomicBool = AtomicBool::new(false);

/// 搜尋／篩選框的 std::string 位址：main_interface 裡名字含 filter、search 的欄位（Lua 啟動時交來，整局不變）
static SEARCH_FIELDS: Mutex<Vec<usize>> = Mutex::new(Vec::new());
/// 傳說清單篩選框的 std::string 位址：Lua 在傳說畫面時每幀送來，離開時送 0（只拿來比位址，不讀）
static LEGENDS_FIELD: AtomicUsize = AtomicUsize::new(0);
/// 遊戲的 widgets::textbox：interfacest.cur_textbox（有鍵盤焦點的那個框的指標）的位址，與框裡 str、textbox_type 欄位的位移。
/// textbox_type 是 FILTER（0）的是篩選框，NAME、NONE 不是（df-structures 的 widget_textbox）。Lua 啟動時交來，0 是沒有
static TEXTBOX: [AtomicUsize; 3] = [const { AtomicUsize::new(0) }; 3];
const TEXTBOX_FILTER: i32 = 0;

pub fn set_fields(addrs: Vec<usize>) {
    *FIELDS.lock() = addrs;
}

pub fn set_extra(text: &[u8]) {
    let mut extra = EXTRA.lock();
    extra.clear();
    extra.extend_from_slice(text);
}

/// 搜尋框的位址：main_interface 的篩選欄位，與 cur_textbox 指標的位址、textbox 裡 str 與 textbox_type 的位移
pub fn set_search_fields(addrs: Vec<usize>, textbox: [usize; 3]) {
    *SEARCH_FIELDS.lock() = addrs;
    for (slot, v) in TEXTBOX.iter().zip(textbox) {
        slot.store(v, Ordering::Relaxed);
    }
}

pub fn set_legends_field(addr: usize) {
    LEGENDS_FIELD.store(addr, Ordering::Relaxed);
}

/// 遊戲打字函式收到的字串 s 是不是搜尋／篩選框（是才收中文）
pub fn is_search_box(s: usize) -> bool {
    if s == 0 {
        return false;
    }
    if LEGENDS_FIELD.load(Ordering::Relaxed) == s || SEARCH_FIELDS.lock().contains(&s) {
        return true;
    }
    let [cur, str_at, type_at] = TEXTBOX.each_ref().map(|a| a.load(Ordering::Relaxed));
    if cur == 0 || str_at == 0 || type_at == 0 {
        return false;
    }
    // cur_textbox 是遊戲的全域指標，textbox 解構時遊戲會把指著它的 cur_textbox 清成 0
    let tb = unsafe { *(cur as *const usize) };
    tb != 0 && s == tb + str_at && unsafe { *((tb + type_at) as *const i32) } == TEXTBOX_FILTER
}

/// 每幀畫字前（render_things 開頭）讀一次輸入框現在的字
pub fn refresh() {
    let fields = FIELDS.lock();
    let extra = EXTRA.lock();
    let mut current = CURRENT.lock();
    current.clear();
    let texts = fields.iter().map(|&a| unsafe { crate::lines::str_bytes(a) }).chain(std::iter::once(extra.as_slice()));
    for text in texts.map(<[u8]>::trim_ascii).filter(|t| !t.is_empty() && t.len() < 256) {
        current.push(text.to_vec());
    }
    ANY.store(!current.is_empty(), Ordering::Relaxed);
}

/// 整段就是輸入框裡的字（遊戲畫輸入中的字時後面接著游標「_」也算）
pub fn is_typed(text: &[u8]) -> bool {
    let (any, zh) = (ANY.load(Ordering::Relaxed), LIVE_ZH.load(Ordering::Relaxed));
    if !any && !zh {
        return false;
    }
    let text = text.trim_ascii();
    let bare = text.strip_suffix(b"_").unwrap_or(text).trim_ascii_end();
    let same = |t: &[u8]| t == text || t == bare;
    if any && CURRENT.lock().iter().any(|t| same(t.as_slice())) {
        return true;
    }
    zh && same(LIVE.lock().trim_ascii())
}

/// 輸入框裡的字有輸入法打的中文（合法的 UTF-8、有非 ASCII 的字）時解成文字，照中文畫；否則 None，照 CP437 畫
pub fn chinese(text: &[u8]) -> Option<&str> {
    if text.is_ascii() {
        return None;
    }
    std::str::from_utf8(text).ok()
}

pub fn set_live(text: &[u8]) {
    let mut live = LIVE.lock();
    live.clear();
    live.extend_from_slice(&text[..text.len().min(256)]);
    LIVE_ZH.store(chinese(&live).is_some(), Ordering::Relaxed);
}

/// 遊戲比對清單前把打的字轉小寫、去掉 CP437 的重音（FUN_14052cee0，逐位元組、長度不變）：UTF-8 的後續位元組
/// 0x80～0xA5 也會被換成英文字母，所以要從打的原字找回來
fn fold(c: u8) -> u8 {
    match c {
        b'A'..=b'Z' => c + 32,
        0x80 | 0x87 => b'c',
        0x81 | 0x96 | 0x97 | 0x9a | 0xa3 => b'u',
        0x82 | 0x88..=0x8a | 0x90 => b'e',
        0x83..=0x86 | 0x8e | 0x8f | 0x91 | 0x92 | 0xa0 => b'a',
        0x8b..=0x8d | 0xa1 => b'i',
        0x93..=0x95 | 0x99 | 0xa2 => b'o',
        0x98 => b'y',
        0xa4 | 0xa5 => b'n',
        _ => c,
    }
}

/// text 裡和 needle（遊戲轉過小寫、去過重音，或只轉小寫）對得上的那一段原字；中文要整個字對上（合法 UTF-8）
fn find_typed(text: &[u8], needle: &[u8]) -> Option<Vec<u8>> {
    let same = |w: &[u8]| w.iter().zip(needle).all(|(&t, &n)| fold(t) == n || t.to_ascii_lowercase() == n);
    text.windows(needle.len()).find(|w| same(*w) && std::str::from_utf8(w).is_ok()).map(<[u8]>::to_vec)
}

/// 搜尋框比對清單時要找的字 needle 是玩家在輸入框打的字（或其中以空白分開的一段）時，回傳那一段原本打的字
/// （輸入法打的中文是 UTF-8 原字）：拼音、中文比對只對這種（見 pinyin.rs）
pub fn typed_part(needle: &[u8]) -> Option<Vec<u8>> {
    if needle.is_empty() {
        return None;
    }
    // LIVE 與 CURRENT 不同時鎖（is_typed 在畫面執行緒反過來鎖）
    let live = find_typed(&LIVE.lock(), needle);
    live.or_else(|| if ANY.load(Ordering::Relaxed) { CURRENT.lock().iter().find_map(|t| find_typed(t, needle)) } else { None })
}

/// 輸入法一次送來的字（SDL_TEXTINPUT 的 UTF-8，最多 32 位元組）裡交給遊戲打字函式的部分：只有 ASCII。
/// 遊戲的打字函式只在旗標有 0x10 時收 0x80 以上的位元組（FUN_140c39700），庫存這類搜尋框（旗標 0xb）一個都不收，
/// 中文字由 hook 自己接到字串後面（見 wide_part）；其他輸入框（命名，會進存檔）也就只收到 ASCII
pub fn ascii_part(text: &[u8]) -> Vec<u8> {
    text.iter().copied().filter(u8::is_ascii).collect()
}

/// 搜尋框要接在字串後面的中文：text 裡合法 UTF-8 的非 ASCII 字，照順序收到放不下 room 個位元組為止（字不切半）
pub fn wide_part(text: &[u8], room: usize) -> Vec<u8> {
    let valid = match std::str::from_utf8(text) {
        Ok(s) => s,
        Err(e) => std::str::from_utf8(&text[..e.valid_up_to()]).unwrap_or_default(),
    };
    let mut out = Vec::new();
    for c in valid.chars().filter(|c| !c.is_ascii()) {
        let mut b = [0; 4];
        let b = c.encode_utf8(&mut b).as_bytes();
        if out.len() + b.len() > room {
            break;
        }
        out.extend_from_slice(b);
    }
    out
}

/// 最後一次收到輸入法的字時的情形（除錯用，Lua 的 engine.ime_dump()）
static LAST_IME: Mutex<String> = Mutex::new(String::new());

#[allow(clippy::too_many_arguments)]
pub fn note_ime(s: usize, max: i32, flags: u64, len: usize, search: bool, room: usize, wide: usize, appended: bool) {
    *LAST_IME.lock() =
        format!("string=0x{s:x} max={max} flags=0x{flags:x} bytes={len} search={search} room={room} wide={wide} appended={appended}");
}

pub fn ime_dump() -> String {
    let search = SEARCH_FIELDS.lock().iter().map(|a| format!("0x{a:x}")).collect::<Vec<_>>().join(" ");
    let textbox = TEXTBOX.each_ref().map(|a| format!("0x{:x}", a.load(Ordering::Relaxed))).join(" ");
    format!("{}\nsearch_fields: {search}\ntextbox: {textbox}\nlegends: 0x{:x}", LAST_IME.lock(), LEGENDS_FIELD.load(Ordering::Relaxed))
}

/// 字串結尾缺了後半的 UTF-8 字有幾個位元組（遊戲的退格一次只刪一個位元組，剩下的要一起刪）；結尾是完整的字時 0
pub fn broken_tail(text: &[u8]) -> usize {
    let cont = text.iter().rev().take(3).take_while(|&&c| c & 0xc0 == 0x80).count();
    let Some(&lead) = text.len().checked_sub(cont + 1).map(|i| &text[i]) else { return 0 };
    let need = match lead {
        0xc2..=0xdf => 2,
        0xe0..=0xef => 3,
        0xf0..=0xf4 => 4,
        _ => return 0,
    };
    if cont + 1 < need { cont + 1 } else { 0 }
}

/// 讓 SDL 顯示輸入法自己的組字與選字視窗：SDL2 在 Windows 預設用 TSF 的 UILess 模式把它們藏起來，要程式自己畫（SDL_HINT_IME_SHOW_UI，
/// 預設 0），DF 沒畫，玩家看不到注音、拼音與候選字。SDL 第一次 SDL_StartTextInput（遊戲第一次用輸入框，見 hook_text_input）時才讀這個設定
pub fn show_ime_ui() {
    use std::ffi::{c_char, c_void};
    #[link(name = "kernel32")]
    unsafe extern "system" {
        fn GetModuleHandleW(name: *const u16) -> *mut c_void;
        fn GetProcAddress(module: *mut c_void, name: *const c_char) -> *mut c_void;
    }
    type SetHint = unsafe extern "C" fn(*const c_char, *const c_char) -> i32;
    let name: Vec<u16> = "SDL2.dll\0".encode_utf16().collect();
    unsafe {
        let module = GetModuleHandleW(name.as_ptr());
        if module.is_null() {
            return;
        }
        let f = GetProcAddress(module, c"SDL_SetHint".as_ptr());
        if !f.is_null() {
            std::mem::transmute::<*mut c_void, SetHint>(f)(c"SDL_IME_SHOW_UI".as_ptr(), c"1".as_ptr());
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn text_typed_into_a_game_input_box_is_matched_whole() {
        set_extra(b"Gallows");
        refresh();
        assert!(is_typed(b"Gallows"));
        assert!(is_typed(b" Gallows_"), "輸入中的游標、頭尾空白不論");
        assert!(!is_typed(b"Gallows of Doom"), "整段相同才算");
        set_extra(b"");
        refresh();
        assert!(!is_typed(b"Gallows"), "清空後照常翻");
    }

    #[test]
    fn ime_text_splits_into_ascii_for_the_game_and_whole_chars_for_search_boxes() {
        let tie = "鐵礦".as_bytes();
        assert_eq!(wide_part(tie, 32), tie, "搜尋框整串收");
        assert_eq!(wide_part(tie, 5), "鐵".as_bytes(), "放不下的字整個不收");
        assert_eq!(wide_part(tie, 2), b"", "一個字都放不下");
        assert_eq!(wide_part(&tie[..4], 32), "鐵".as_bytes(), "SDL 切壞的尾巴不收");
        assert_eq!(wide_part("a鐵b".as_bytes(), 32), "鐵".as_bytes(), "ASCII 交給遊戲的打字函式");
        assert_eq!(ascii_part("a鐵b".as_bytes()), b"ab", "遊戲的打字函式（與命名框）只收到 ASCII");
        assert_eq!(chinese("鐵礦_".as_bytes()), Some("鐵礦_"));
        assert_eq!(chinese(b"Gems"), None, "英文照 CP437");
        assert_eq!(chinese(&[0x9a, b'r']), None, "CP437 的重音字不是 UTF-8");
    }

    #[test]
    fn backspace_removes_the_rest_of_a_chinese_char() {
        let mut s = "a鐵礦".as_bytes().to_vec();
        s.pop(); // 遊戲的退格刪一個位元組
        let n = broken_tail(&s);
        assert_eq!(n, 2);
        s.truncate(s.len() - n);
        assert_eq!(s, "a鐵".as_bytes());
        assert_eq!(broken_tail(&s), 0, "完整的字不刪");
        assert_eq!(broken_tail(b"abc"), 0);
        assert_eq!(broken_tail(&[0xe9]), 1);
        assert_eq!(broken_tail(&[b'x', 0x9a]), 0, "單獨的 CP437 字元（不是 UTF-8 開頭）不動");
    }

    #[test]
    fn folded_needle_finds_the_typed_chinese() {
        // 鐵 = E9 90 B5、礦 = E7 A4 A6：0x90 被遊戲換成 e、0xA4 換成 n
        let typed = "x 鐵礦".as_bytes();
        let folded: Vec<u8> = "鐵礦".bytes().map(fold).collect();
        assert_ne!(folded, "鐵礦".as_bytes());
        assert_eq!(find_typed(typed, &folded), Some("鐵礦".as_bytes().to_vec()));
        assert_eq!(find_typed(b"Tie Kuang", b"kuang"), Some(b"Kuang".to_vec()), "英文照舊（原字大小寫不論）");
        assert_eq!(find_typed(typed, b"zz"), None);
    }
}
