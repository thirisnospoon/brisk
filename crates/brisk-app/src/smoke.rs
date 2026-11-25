//! What production code needs to know about the smoke tests of `docs/testing.md`. Without the
//! `smoke` feature there are none, and every answer here is no. The smokes themselves are in
//! `workspace::smoke`.

/// Whether the smoke variable `name` (`BRISK_SMOKE_...`) is set.
#[cfg(feature = "smoke")]
pub fn on(name: &str) -> bool {
    std::env::var_os(name).is_some()
}

#[cfg(not(feature = "smoke"))]
#[inline(always)]
pub fn on(_name: &str) -> bool {
    false
}

/// Whether a smoke drives this process: pip stays offline and no browser is opened.
#[cfg(feature = "smoke")]
pub fn active() -> bool {
    std::env::vars_os().any(|(k, _)| k.to_string_lossy().starts_with("BRISK_SMOKE_"))
}

#[cfg(not(feature = "smoke"))]
#[inline(always)]
pub fn active() -> bool {
    false
}

/// Smokes and the `BRISK_OPEN_*` helpers run in the background: the window is on screen but nearly transparent, behind every
/// other window, click-through, without focus or a taskbar button. Windows still composes it,
/// so it draws real frames and `PrintWindow` captures it; off-screen or hidden it would not.
/// `BRISK_SMOKE_VISIBLE=1` opens it normally, to watch a smoke or look at a hold.
#[cfg(feature = "smoke")]
fn background() -> bool {
    let open_helper = std::env::vars_os().any(|(k, _)| k.to_string_lossy().starts_with("BRISK_OPEN_"));
    (active() || open_helper) && !on("BRISK_SMOKE_VISIBLE")
}

/// Opens the window hidden in a background smoke; `window_opened` shows it.
#[cfg(feature = "smoke")]
pub fn window_options(o: gpui::WindowOptions) -> gpui::WindowOptions {
    if cfg!(windows) && background() {
        gpui::WindowOptions { show: false, focus: false, ..o }
    } else {
        o
    }
}

/// Shows the hidden window off-screen, then, a moment later, as the transparent window on
/// screen. An off-screen window gets no `WM_PAINT` and draws nothing; one made layered before
/// it is first shown stays black in `PrintWindow`.
#[cfg(all(feature = "smoke", windows))]
pub fn window_opened() {
    if background() {
        background_window::show_off_screen();
        std::thread::spawn(|| {
            std::thread::sleep(std::time::Duration::from_millis(200));
            background_window::ghost();
        });
    }
}

#[cfg(all(feature = "smoke", not(windows)))]
pub fn window_opened() {}

#[cfg(all(feature = "smoke", windows))]
mod background_window {
    use std::sync::atomic::{AtomicIsize, Ordering};

    #[link(name = "user32")]
    extern "system" {
        fn EnumThreadWindows(thread: u32, f: extern "system" fn(isize, isize) -> i32, l: isize) -> i32;
        fn GetClassNameW(h: isize, buf: *mut u16, len: i32) -> i32;
        fn GetWindowLongPtrW(h: isize, index: i32) -> isize;
        fn SetWindowLongPtrW(h: isize, index: i32, v: isize) -> isize;
        fn SetWindowPos(h: isize, after: isize, x: i32, y: i32, cx: i32, cy: i32, flags: u32) -> i32;
        fn ShowWindow(h: isize, cmd: i32) -> i32;
        fn GetDpiForWindow(h: isize) -> u32;
        fn SetLayeredWindowAttributes(h: isize, key: u32, alpha: u8, flags: u32) -> i32;
    }
    #[link(name = "kernel32")]
    extern "system" {
        fn GetCurrentThreadId() -> u32;
    }
    const GWL_EXSTYLE: i32 = -20;
    const WS_EX_APPWINDOW: isize = 0x0004_0000;
    const WS_EX_TOOLWINDOW: isize = 0x0000_0080;
    const WS_EX_NOACTIVATE: isize = 0x0800_0000;
    const WS_EX_LAYERED: isize = 0x0008_0000;
    const WS_EX_TRANSPARENT: isize = 0x0000_0020;
    const LWA_ALPHA: u32 = 0x2;
    const HWND_BOTTOM: isize = 1;
    const SWP_NOSIZE: u32 = 0x0001;
    const SWP_NOMOVE: u32 = 0x0002;
    const SWP_NOACTIVATE: u32 = 0x0010;
    const SWP_FRAMECHANGED: u32 = 0x0020;
    const SW_SHOWNOACTIVATE: i32 = 4;

    /// GPUI's class for app windows (its dispatcher window is `Zed::PlatformWindow`).
    const CLASS: &str = "Zed::Window";
    /// The size `main` asks for; a window opened hidden is created at the system default.
    const SIZE: (f32, f32) = (1400., 900.);

    static HWND: AtomicIsize = AtomicIsize::new(0);

    /// `SIZE` in physical pixels at the window's DPI.
    fn size(hwnd: isize) -> (i32, i32) {
        let scale = unsafe { GetDpiForWindow(hwnd) }.max(96) as f32 / 96.;
        ((SIZE.0 * scale) as i32, (SIZE.1 * scale) as i32)
    }

    extern "system" fn found(h: isize, out: isize) -> i32 {
        let mut buf = [0u16; 64];
        let n = unsafe { GetClassNameW(h, buf.as_mut_ptr(), buf.len() as i32) };
        if n > 0 && String::from_utf16_lossy(&buf[..n as usize]) == CLASS {
            unsafe { *(out as *mut isize) = h };
            return 0;
        }
        1
    }

    pub(super) fn show_off_screen() {
        let mut hwnd: isize = 0;
        unsafe { EnumThreadWindows(GetCurrentThreadId(), found, &mut hwnd as *mut isize as isize) };
        if hwnd == 0 {
            eprintln!("smoke: no Brisk window to show in the background; it stays hidden");
            return;
        }
        HWND.store(hwnd, Ordering::Relaxed);
        unsafe {
            let ex = GetWindowLongPtrW(hwnd, GWL_EXSTYLE);
            SetWindowLongPtrW(hwnd, GWL_EXSTYLE, (ex & !WS_EX_APPWINDOW) | WS_EX_TOOLWINDOW | WS_EX_NOACTIVATE);
            let (w, h) = size(hwnd);
            SetWindowPos(hwnd, HWND_BOTTOM, -32000, -32000, w, h, SWP_NOACTIVATE | SWP_FRAMECHANGED);
            ShowWindow(hwnd, SW_SHOWNOACTIVATE);
        }
    }

    pub(super) fn ghost() {
        let hwnd = HWND.load(Ordering::Relaxed);
        if hwnd == 0 {
            return;
        }
        unsafe {
            let ex = GetWindowLongPtrW(hwnd, GWL_EXSTYLE);
            SetWindowLongPtrW(hwnd, GWL_EXSTYLE, ex | WS_EX_LAYERED | WS_EX_TRANSPARENT);
            // 1 of 255: not visible, but still composed and drawn.
            SetLayeredWindowAttributes(hwnd, 0, 1, LWA_ALPHA);
            SetWindowPos(hwnd, HWND_BOTTOM, 0, 0, 0, 0, SWP_NOSIZE | SWP_NOACTIVATE);
            // Off-screen the window may have had another DPI than the monitor it is on now.
            let (w, h) = size(hwnd);
            SetWindowPos(hwnd, HWND_BOTTOM, 0, 0, w, h, SWP_NOMOVE | SWP_NOACTIVATE);
        }
    }
}
