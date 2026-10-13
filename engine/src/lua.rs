//! DFHack 內建的 Lua 5.3（`hack/lua53.dll`）C API。執行時從匯出表取函式，不必連結匯入庫，
//! 也就不綁定特定 DFHack 版本：只要行程裡的 lua53.dll 還是 Lua 5.3。
//!
//! DFHack 以 C++ 編譯 Lua，匯出名稱是 MSVC 修飾名（`?lua_gettop@@YAHPEAUlua_State@@@Z`），
//! 所以依「`?函式名@@` 開頭」比對；Lua API 沒有多載，前綴就能唯一對應。
//! 也因為是 C++ 編譯，lua_error 會丟 C++ 例外，不能穿過 Rust 堆疊：本模組不呼叫它，錯誤一律以 nil, 訊息回傳。

use std::ffi::{c_char, c_int, c_void, CStr};
use std::sync::OnceLock;

pub type State = *mut c_void;
pub type CFunction = unsafe extern "C" fn(State) -> c_int;

pub const TSTRING: c_int = 4;
pub const TTABLE: c_int = 5;

#[link(name = "kernel32")]
unsafe extern "system" {
    fn GetModuleHandleW(name: *const u16) -> *mut c_void;
}

/// 在模組匯出表裡找名稱為 `name` 或以 `?name@@` 開頭的匯出。
unsafe fn export(module: *const u8, name: &str) -> Option<*mut c_void> {
    unsafe {
        let pe = module.add(*(module.add(0x3c) as *const u32) as usize);
        let dir_rva = *(pe.add(24 + 112) as *const u32) as usize; // PE32+ 選用標頭的第 0 個資料目錄
        if dir_rva == 0 {
            return None;
        }
        let dir = module.add(dir_rva);
        let count = *(dir.add(0x18) as *const u32) as usize;
        let functions = module.add(*(dir.add(0x1c) as *const u32) as usize) as *const u32;
        let names = module.add(*(dir.add(0x20) as *const u32) as usize) as *const u32;
        let ordinals = module.add(*(dir.add(0x24) as *const u32) as usize) as *const u16;
        let mangled = format!("?{name}@@");
        for i in 0..count {
            let export = CStr::from_ptr(module.add(*names.add(i) as usize).cast()).to_bytes();
            if export == name.as_bytes() || export.starts_with(mangled.as_bytes()) {
                let rva = *functions.add(*ordinals.add(i) as usize) as usize;
                return Some(module.add(rva) as *mut c_void);
            }
        }
        None
    }
}

macro_rules! lua_api {
    ($($name:ident: fn($($arg:ty),*) $(-> $ret:ty)?;)*) => {
        #[allow(non_snake_case)]
        pub struct Api { $(pub $name: unsafe extern "C" fn($($arg),*) $(-> $ret)?,)* }

        impl Api {
            unsafe fn resolve(module: *const u8) -> Option<Api> {
                Some(Api { $($name: {
                    let p = unsafe { export(module, stringify!($name))? };
                    unsafe { std::mem::transmute::<*mut c_void, unsafe extern "C" fn($($arg),*) $(-> $ret)?>(p) }
                },)* })
            }
        }
    };
}

lua_api! {
    lua_type: fn(State, c_int) -> c_int;
    lua_tolstring: fn(State, c_int, *mut usize) -> *const c_char;
    lua_tointegerx: fn(State, c_int, *mut c_int) -> i64;
    lua_rawlen: fn(State, c_int) -> usize;
    lua_rawgeti: fn(State, c_int, i64) -> c_int;
    lua_getfield: fn(State, c_int, *const c_char) -> c_int;
    lua_settop: fn(State, c_int);
    lua_pushnil: fn(State);
    lua_pushinteger: fn(State, i64);
    lua_pushboolean: fn(State, c_int);
    lua_pushlstring: fn(State, *const c_char, usize) -> *const c_char;
    lua_createtable: fn(State, c_int, c_int);
    lua_setfield: fn(State, c_int, *const c_char);
    lua_pushcclosure: fn(State, CFunction, c_int);
}

static API: OnceLock<Api> = OnceLock::new();

/// 取得 lua53.dll 的函式表；找不到模組或任一函式就回傳 false。
pub fn init() -> bool {
    if API.get().is_some() {
        return true;
    }
    let name: Vec<u16> = "lua53.dll\0".encode_utf16().collect();
    let module = unsafe { GetModuleHandleW(name.as_ptr()) } as *const u8;
    if module.is_null() {
        return false;
    }
    match unsafe { Api::resolve(module) } {
        Some(api) => API.set(api).is_ok() || API.get().is_some(),
        None => false,
    }
}

fn api() -> &'static Api {
    API.get().expect("lua::init must succeed before any Lua call")
}

pub unsafe fn arg_bytes<'a>(l: State, idx: c_int) -> Option<&'a [u8]> {
    unsafe {
        if (api().lua_type)(l, idx) != TSTRING {
            return None;
        }
        let mut len = 0usize;
        let p = (api().lua_tolstring)(l, idx, &mut len);
        Some(std::slice::from_raw_parts(p.cast(), len))
    }
}

pub unsafe fn arg_str<'a>(l: State, idx: c_int) -> Option<&'a str> {
    unsafe { arg_bytes(l, idx).and_then(|b| std::str::from_utf8(b).ok()) }
}

pub unsafe fn arg_int(l: State, idx: c_int) -> Option<i64> {
    let mut ok = 0;
    let v = unsafe { (api().lua_tointegerx)(l, idx, &mut ok) };
    (ok != 0).then_some(v)
}

pub unsafe fn arg_type(l: State, idx: c_int) -> c_int {
    unsafe { (api().lua_type)(l, idx) }
}

/// table[name] 的整數值（table 在 idx）。
pub unsafe fn field_int(l: State, idx: c_int, name: &CStr) -> Option<i64> {
    unsafe {
        (api().lua_getfield)(l, idx, name.as_ptr());
        let v = arg_int(l, -1);
        (api().lua_settop)(l, -2);
        v
    }
}

/// table[i] 的字串（複製出來：彈出堆疊後 Lua 可能回收原字串）。
pub unsafe fn index_str(l: State, idx: c_int, i: i64) -> Option<String> {
    unsafe {
        (api().lua_rawgeti)(l, idx, i);
        let v = arg_str(l, -1).map(str::to_owned);
        (api().lua_settop)(l, -2);
        v
    }
}

/// table[i] 的整數值（陣列，table 在 idx，i 從 1 起）。
pub unsafe fn index_int(l: State, idx: c_int, i: i64) -> Option<i64> {
    unsafe {
        (api().lua_rawgeti)(l, idx, i);
        let v = arg_int(l, -1);
        (api().lua_settop)(l, -2);
        v
    }
}

pub unsafe fn len(l: State, idx: c_int) -> usize {
    unsafe { (api().lua_rawlen)(l, idx) }
}

pub unsafe fn push_nil(l: State) {
    unsafe { (api().lua_pushnil)(l) }
}

pub unsafe fn push_int(l: State, v: i64) {
    unsafe { (api().lua_pushinteger)(l, v) }
}

pub unsafe fn push_bool(l: State, v: bool) {
    unsafe { (api().lua_pushboolean)(l, v as c_int) }
}

pub unsafe fn push_str(l: State, s: &str) {
    unsafe { (api().lua_pushlstring)(l, s.as_ptr().cast(), s.len()) };
}

pub unsafe fn new_table(l: State) {
    unsafe { (api().lua_createtable)(l, 0, 0) }
}

/// 把堆疊頂端的值設成 table（在頂端下一格）的 name 欄位。
pub unsafe fn set_field(l: State, name: &CStr) {
    unsafe { (api().lua_setfield)(l, -2, name.as_ptr()) }
}

pub unsafe fn set_fn(l: State, name: &CStr, f: CFunction) {
    unsafe {
        (api().lua_pushcclosure)(l, f, 0);
        set_field(l, name);
    }
}
