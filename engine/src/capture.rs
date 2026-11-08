//! 開發用截圖（`scripts/dev_game.py shot`、`sweep`）：PrintWindow 要螢幕醒著才拍得到 DF 的畫面（GPU 畫的，靠 DWM 合成），
//! 9950X3D 沒人連線時主控台的螢幕是關的，拍到整張黑，送滑鼠輸入也叫不醒（2026-11-08 實測）。這裡攔下 SDL_RenderPresent，
//! 有要求時在這一幀送出前用 SDL_RenderReadPixels 讀回畫面（DF 直接畫在預設的繪圖目標上），存成寬、高（u32 LE）加 BGRA。
//! 第一次要求時才掛 hook，玩家用不到。

use std::ffi::c_void;
use std::sync::atomic::{AtomicUsize, Ordering};

use parking_lot::Mutex;
use retour::RawDetour;

type PresentFn = unsafe extern "C" fn(*mut c_void);
type OutputSizeFn = unsafe extern "C" fn(*mut c_void, *mut i32, *mut i32) -> i32;
type ReadPixelsFn = unsafe extern "C" fn(*mut c_void, *const c_void, u32, *mut c_void, i32) -> i32;

/// SDL_PIXELFORMAT_ARGB8888：記憶體裡（小端序）是 B G R A
const ARGB8888: u32 = 0x1636_2004;
static TRAMPOLINE: AtomicUsize = AtomicUsize::new(0);
static OUTPUT_SIZE: AtomicUsize = AtomicUsize::new(0);
static READ_PIXELS: AtomicUsize = AtomicUsize::new(0);
/// 掛過 hook 了：整個行程只掛一次、不拆（同 mouse.rs）
static INSTALLED: Mutex<bool> = Mutex::new(false);
/// 下一幀要存到的檔名
static PENDING: Mutex<Option<String>> = Mutex::new(None);

unsafe extern "C" fn hook_present(renderer: *mut c_void) {
    if let Some(path) = PENDING.lock().take() {
        if let Err(e) = unsafe { save(renderer, &path) } {
            let _ = std::fs::write(format!("{path}.err"), e);
        }
    }
    let orig: PresentFn = unsafe { std::mem::transmute(TRAMPOLINE.load(Ordering::Acquire)) };
    unsafe { orig(renderer) }
}

unsafe fn save(renderer: *mut c_void, path: &str) -> Result<(), String> {
    let size: OutputSizeFn = unsafe { std::mem::transmute(OUTPUT_SIZE.load(Ordering::Acquire)) };
    let read: ReadPixelsFn = unsafe { std::mem::transmute(READ_PIXELS.load(Ordering::Acquire)) };
    let (mut w, mut h) = (0, 0);
    if unsafe { size(renderer, &mut w, &mut h) } != 0 || w <= 0 || h <= 0 {
        return Err("SDL_GetRendererOutputSize failed".into());
    }
    let mut data = vec![0u8; 8 + w as usize * h as usize * 4];
    data[..4].copy_from_slice(&(w as u32).to_le_bytes());
    data[4..8].copy_from_slice(&(h as u32).to_le_bytes());
    if unsafe { read(renderer, std::ptr::null(), ARGB8888, data[8..].as_mut_ptr().cast(), w * 4) } != 0 {
        return Err("SDL_RenderReadPixels failed".into());
    }
    // 先寫暫存檔再改名：dev_game.py 一看到檔名就讀，不會讀到寫一半的
    let tmp = format!("{path}.tmp");
    std::fs::write(&tmp, &data).and_then(|()| std::fs::rename(&tmp, path)).map_err(|e| e.to_string())
}

/// 下一幀送出前把畫面存到 path；失敗時寫 `<path>.err`
pub fn request(path: &str) -> Result<(), String> {
    let mut installed = INSTALLED.lock();
    if !*installed {
        let export = |name: &str| crate::lua::module_export("SDL2.dll", name).ok_or(format!("SDL2.dll: no {name}"));
        let target = export("SDL_RenderPresent")?;
        OUTPUT_SIZE.store(export("SDL_GetRendererOutputSize")?, Ordering::Release);
        READ_PIXELS.store(export("SDL_RenderReadPixels")?, Ordering::Release);
        let d = unsafe { RawDetour::new(target as *const (), hook_present as *const ()) }.map_err(|e| e.to_string())?;
        TRAMPOLINE.store(d.trampoline() as *const () as usize, Ordering::Release);
        unsafe { d.enable() }.map_err(|e| e.to_string())?;
        std::mem::forget(d);
        *installed = true;
    }
    *PENDING.lock() = Some(path.to_owned());
    Ok(())
}
