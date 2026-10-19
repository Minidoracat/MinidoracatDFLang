//! 開發用的假游標（`scripts/dev_game.py hover`）：遊戲的游標位置只來自 SDL，主迴圈每圈呼叫 SDL_GetMouseState，
//! 和存著的不同就改寫 gps 的游標欄位；Lua 直接寫那些欄位會被下一圈蓋掉，提示框這類要停留的畫面測不到。
//! 這裡攔下 SDL_GetMouseState，設了位置就回傳它（視窗像素、沒有按鍵），不動真游標。第一次設位置時才掛 hook，玩家用不到。
//! 遊戲還要認為游標在視窗裡才會呼叫它（enabler 的旗標，SDL 的 ENTER／LEAVE 事件設），由呼叫的 Lua 每幀設。

use std::sync::atomic::{AtomicU64, AtomicUsize, Ordering};

use parking_lot::Mutex;
use retour::RawDetour;

type GetMouseStateFn = unsafe extern "C" fn(*mut i32, *mut i32) -> u32;

/// 假游標位置：x 在高 32 位、y 在低 32 位；NONE 是照真游標
const NONE: u64 = u64::MAX;
static FAKE: AtomicU64 = AtomicU64::new(NONE);
static TRAMPOLINE: AtomicUsize = AtomicUsize::new(0);

struct Detour(RawDetour);
// 只在持有鎖時由 Lua 指令建立；遊戲的主執行緒只讀 TRAMPOLINE
unsafe impl Send for Detour {}
static DETOUR: Mutex<Option<Detour>> = Mutex::new(None);

unsafe extern "C" fn hook_get_mouse_state(x: *mut i32, y: *mut i32) -> u32 {
    let fake = FAKE.load(Ordering::Acquire);
    if fake == NONE {
        let orig: GetMouseStateFn = unsafe { std::mem::transmute(TRAMPOLINE.load(Ordering::Acquire)) };
        return unsafe { orig(x, y) };
    }
    unsafe {
        if !x.is_null() {
            *x = (fake >> 32) as u32 as i32;
        }
        if !y.is_null() {
            *y = fake as u32 as i32;
        }
    }
    0
}

/// 假裝游標停在 (x, y)（遊戲視窗的像素）；None 改回真游標
pub fn set(pos: Option<(i32, i32)>) -> Result<(), String> {
    if pos.is_some() {
        let mut detour = DETOUR.lock();
        if detour.is_none() {
            let target = crate::lua::module_export("SDL2.dll", "SDL_GetMouseState").ok_or("SDL2.dll: no SDL_GetMouseState")?;
            let d = unsafe { RawDetour::new(target as *const (), hook_get_mouse_state as *const ()) }.map_err(|e| e.to_string())?;
            TRAMPOLINE.store(d.trampoline() as *const () as usize, Ordering::Release);
            unsafe { d.enable() }.map_err(|e| e.to_string())?;
            *detour = Some(Detour(d));
        }
    }
    FAKE.store(pos.map_or(NONE, |(x, y)| (u64::from(x as u32) << 32) | u64::from(y as u32)), Ordering::Release);
    Ok(())
}
