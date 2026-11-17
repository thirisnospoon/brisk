//! Smoke checks only: real Windows mouse messages posted to our own window, so clicks and drags
//! travel the same path as the user's (hit testing, occlusion, focus).

#[derive(Clone, Copy)]
pub(super) enum Mouse {
    Move,
    Down,
    Up,
    RightDown,
    RightUp,
}

/// Post one mouse message at window coordinates in physical pixels; `held`: left button down.
#[cfg(windows)]
pub(super) fn post(kind: Mouse, x: f32, y: f32, held: bool) {
    #[link(name = "user32")]
    extern "system" {
        fn EnumThreadWindows(thread: u32, f: extern "system" fn(isize, isize) -> i32, l: isize) -> i32;
        fn IsWindowVisible(h: isize) -> i32;
        fn PostMessageW(h: isize, msg: u32, w: usize, l: isize) -> i32;
    }
    #[link(name = "kernel32")]
    extern "system" {
        fn GetCurrentThreadId() -> u32;
    }
    extern "system" fn found(h: isize, out: isize) -> i32 {
        unsafe {
            if IsWindowVisible(h) != 0 {
                *(out as *mut isize) = h;
                return 0;
            }
        }
        1
    }
    let mut hwnd: isize = 0;
    unsafe {
        EnumThreadWindows(GetCurrentThreadId(), found, &mut hwnd as *mut isize as isize);
        if hwnd == 0 {
            return;
        }
        let l = ((y as i32 as isize & 0xffff) << 16) | (x as i32 as isize & 0xffff);
        let msg = match kind {
            Mouse::Move => 0x0200, // WM_MOUSEMOVE
            Mouse::Down => 0x0201, // WM_LBUTTONDOWN
            Mouse::Up => 0x0202,   // WM_LBUTTONUP
            Mouse::RightDown => 0x0204,
            Mouse::RightUp => 0x0205,
        };
        let buttons = match kind {
            Mouse::RightDown => 2, // MK_RBUTTON
            _ if held => 1,        // MK_LBUTTON
            _ => 0,
        };
        PostMessageW(hwnd, msg, buttons, l);
    }
}

#[cfg(not(windows))]
pub(super) fn post(_: Mouse, _: f32, _: f32, _: bool) {}

/// A left click: move, press, release.
pub(super) fn click(x: f32, y: f32) {
    post(Mouse::Move, x, y, false);
    post(Mouse::Down, x, y, true);
    post(Mouse::Up, x, y, false);
}

pub(super) fn right_click(x: f32, y: f32) {
    post(Mouse::Move, x, y, false);
    post(Mouse::RightDown, x, y, false);
    post(Mouse::RightUp, x, y, false);
}
