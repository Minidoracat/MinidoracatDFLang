//! 玩家還沒選過語言時照系統判斷用繁中還是簡中（見 lua/loader.lua 的 chosen_lang）：依序看 Steam 用戶端的介面語言、
//! Windows 的顯示語言（偏好清單裡第一個中文）、Windows 的地區格式，第一個是中文的就照它；都不是中文回傳 None，載入器用繁中。
//! 遊戲的語言（ISteamApps::GetCurrentGameLanguage）沒有用：DF 在 Steam 只支援英文。系統字碼頁也不看：
//! 開了「使用 Unicode UTF-8」時是 65001，看不出繁簡。

use std::ffi::{CStr, c_char, c_void};

#[link(name = "kernel32")]
unsafe extern "system" {
    fn GetModuleHandleW(name: *const u16) -> *mut c_void;
    fn GetProcAddress(module: *mut c_void, name: *const c_char) -> *mut c_void;
    fn GetUserPreferredUILanguages(flags: u32, count: *mut u32, buf: *mut u16, len: *mut u32) -> i32;
    fn GetUserDefaultLocaleName(buf: *mut u16, len: i32) -> i32;
}

const MUI_LANGUAGE_NAME: u32 = 0x8;
const LOCALE_NAME_MAX_LENGTH: usize = 85;

pub fn system_lang() -> Option<&'static str> {
    pick(steam_ui_language().as_deref(), &ui_languages(), user_locale().as_deref())
}

/// Steam 的 tchinese、schinese 優先；Steam 是其他語言時看 Windows 的顯示語言清單，再看地區格式
fn pick(steam: Option<&str>, ui: &[String], locale: Option<&str>) -> Option<&'static str> {
    match steam {
        Some("tchinese") => Some("zh-Hant"),
        Some("schinese") => Some("zh-Hans"),
        _ => ui.iter().map(String::as_str).chain(locale).find_map(chinese),
    }
}

/// 語言名稱（zh-TW、zh-Hant-HK、zh-CN…）是哪種中文：寫了文字（Hant、Hans）照文字，否則台灣、香港、澳門是繁中，
/// 其他地區是簡中；不是中文回傳 None
fn chinese(tag: &str) -> Option<&'static str> {
    let mut parts = tag.split(['-', '_']);
    if !parts.next()?.eq_ignore_ascii_case("zh") {
        return None;
    }
    for part in parts {
        if part.eq_ignore_ascii_case("hant") || ["tw", "hk", "mo"].iter().any(|r| part.eq_ignore_ascii_case(r)) {
            return Some("zh-Hant");
        }
        if part.eq_ignore_ascii_case("hans") {
            return Some("zh-Hans");
        }
    }
    Some("zh-Hans")
}

/// Steam 用戶端的介面語言（API 語言代碼：tchinese、schinese、english…）。遊戲已經載入 steam_api64.dll、初始化過 Steam 才拿得到；
/// 介面存取函式的名稱帶版本號（DF 53.16 附的是 v010），遊戲換了版本找不到就略過，改看 Windows
fn steam_ui_language() -> Option<String> {
    type Utils = unsafe extern "C" fn() -> *mut c_void;
    type UiLanguage = unsafe extern "C" fn(*mut c_void) -> *const c_char;
    let name: Vec<u16> = "steam_api64.dll\0".encode_utf16().collect();
    unsafe {
        let module = GetModuleHandleW(name.as_ptr());
        if module.is_null() {
            return None;
        }
        let utils = GetProcAddress(module, c"SteamAPI_SteamUtils_v010".as_ptr());
        let language = GetProcAddress(module, c"SteamAPI_ISteamUtils_GetSteamUILanguage".as_ptr());
        if utils.is_null() || language.is_null() {
            return None;
        }
        let utils = std::mem::transmute::<*mut c_void, Utils>(utils)();
        if utils.is_null() {
            return None;
        }
        let s = std::mem::transmute::<*mut c_void, UiLanguage>(language)(utils);
        (!s.is_null()).then(|| CStr::from_ptr(s).to_string_lossy().into_owned())
    }
}

/// Windows 的顯示語言，照使用者設定的偏好順序
fn ui_languages() -> Vec<String> {
    let (mut count, mut len) = (0u32, 0u32);
    unsafe {
        if GetUserPreferredUILanguages(MUI_LANGUAGE_NAME, &mut count, std::ptr::null_mut(), &mut len) == 0 {
            return Vec::new();
        }
        let mut buf = vec![0u16; len as usize];
        if GetUserPreferredUILanguages(MUI_LANGUAGE_NAME, &mut count, buf.as_mut_ptr(), &mut len) == 0 {
            return Vec::new();
        }
        buf.split(|&c| c == 0).filter(|s| !s.is_empty()).map(String::from_utf16_lossy).collect()
    }
}

/// Windows 的地區格式（zh-TW 這類名稱）
fn user_locale() -> Option<String> {
    let mut buf = [0u16; LOCALE_NAME_MAX_LENGTH];
    let n = unsafe { GetUserDefaultLocaleName(buf.as_mut_ptr(), buf.len() as i32) };
    (n > 1).then(|| String::from_utf16_lossy(&buf[..n as usize - 1]))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn tags(list: &[&str]) -> Vec<String> {
        list.iter().map(|s| s.to_string()).collect()
    }

    #[test]
    fn steam_language_comes_first_then_the_first_chinese_windows_setting() {
        assert_eq!(pick(Some("schinese"), &tags(&["zh-TW"]), Some("zh-TW")), Some("zh-Hans"));
        assert_eq!(pick(Some("english"), &tags(&["en-US", "zh-HK"]), Some("zh-CN")), Some("zh-Hant"));
        assert_eq!(pick(None, &tags(&["en-US"]), Some("zh-SG")), Some("zh-Hans"));
        assert_eq!(pick(None, &tags(&["zh-Hant-MO"]), None), Some("zh-Hant"));
        assert_eq!(pick(None, &tags(&["zh-Hans-HK"]), None), Some("zh-Hans"));
        assert_eq!(pick(Some("english"), &tags(&["en-US", "ja-JP"]), Some("en-US")), None);
    }
}
