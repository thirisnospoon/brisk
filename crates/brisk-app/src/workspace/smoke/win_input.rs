//! Smoke checks only: real Windows mouse messages posted to our own window, so clicks and drags
//! travel the same path as the user's (hit testing, occlusion, focus).

#[derive(Clone, Copy)]
pub(in crate::workspace) enum Mouse {
    Move,
    Down,
    Up,
    RightDown,
    RightUp,
    MiddleDown,
    MiddleUp,
    /// WM_MOUSEMOVE with the middle button held.
    MiddleMove,
}

/// Post one mouse message at window coordinates in physical pixels; `held`: left button down.
#[cfg(windows)]
pub(in crate::workspace) fn post(kind: Mouse, x: f32, y: f32, held: bool) {
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
            Mouse::Move | Mouse::MiddleMove => 0x0200, // WM_MOUSEMOVE
            Mouse::Down => 0x0201,                     // WM_LBUTTONDOWN
            Mouse::Up => 0x0202,                       // WM_LBUTTONUP
            Mouse::RightDown => 0x0204,
            Mouse::RightUp => 0x0205,
            Mouse::MiddleDown => 0x0207, // WM_MBUTTONDOWN
            Mouse::MiddleUp => 0x0208,
        };
        let buttons = match kind {
            Mouse::RightDown => 2,                         // MK_RBUTTON
            Mouse::MiddleDown | Mouse::MiddleMove => 0x10, // MK_MBUTTON
            _ if held => 1,                                // MK_LBUTTON
            _ => 0,
        };
        PostMessageW(hwnd, msg, buttons, l);
    }
}

#[cfg(not(windows))]
pub(in crate::workspace) fn post(_: Mouse, _: f32, _: f32, _: bool) {}

/// One WM_MOUSEWHEEL notch (`delta` 120 = up, -120 = down) at window coordinates in physical
/// pixels (the message carries screen coordinates).
#[cfg(windows)]
pub(in crate::workspace) fn wheel(x: f32, y: f32, delta: i16) {
    #[repr(C)]
    struct Pt {
        x: i32,
        y: i32,
    }
    #[link(name = "user32")]
    extern "system" {
        fn EnumThreadWindows(thread: u32, f: extern "system" fn(isize, isize) -> i32, l: isize) -> i32;
        fn IsWindowVisible(h: isize) -> i32;
        fn PostMessageW(h: isize, msg: u32, w: usize, l: isize) -> i32;
        fn ClientToScreen(h: isize, p: *mut Pt) -> i32;
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
        let mut p = Pt { x: x as i32, y: y as i32 };
        ClientToScreen(hwnd, &mut p);
        let l = ((p.y as isize & 0xffff) << 16) | (p.x as isize & 0xffff);
        let w = (delta as u16 as usize) << 16;
        PostMessageW(hwnd, 0x020A, w, l); // WM_MOUSEWHEEL
    }
}

#[cfg(not(windows))]
pub(in crate::workspace) fn wheel(_: f32, _: f32, _: i16) {}

/// A left click: move, press, release.
pub(in crate::workspace) fn click(x: f32, y: f32) {
    post(Mouse::Move, x, y, false);
    post(Mouse::Down, x, y, true);
    post(Mouse::Up, x, y, false);
}

pub(in crate::workspace) fn middle_click(x: f32, y: f32) {
    post(Mouse::Move, x, y, false);
    post(Mouse::MiddleDown, x, y, false);
    post(Mouse::MiddleUp, x, y, false);
}

pub(in crate::workspace) fn right_click(x: f32, y: f32) {
    post(Mouse::Move, x, y, false);
    post(Mouse::RightDown, x, y, false);
    post(Mouse::RightUp, x, y, false);
}

/// Types `text` as real key presses posted to our own window: WM_KEYDOWN, the WM_CHAR that
/// TranslateMessage would add, WM_KEYUP. Letters, digits, space and Enter only.
#[cfg(windows)]
pub(in crate::workspace) fn type_keys(text: &str) {
    #[link(name = "user32")]
    extern "system" {
        fn EnumThreadWindows(thread: u32, f: extern "system" fn(isize, isize) -> i32, l: isize) -> i32;
        fn IsWindowVisible(h: isize) -> i32;
        fn PostMessageW(h: isize, msg: u32, w: usize, l: isize) -> i32;
        fn MapVirtualKeyW(code: u32, map: u32) -> u32;
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
        for c in text.chars() {
            let vk = match c {
                ' ' => 0x20,
                '\n' => 0x0d,
                c if c.is_ascii_alphanumeric() => c.to_ascii_uppercase() as u32,
                _ => continue,
            };
            let scan = MapVirtualKeyW(vk, 0) as isize; // MAPVK_VK_TO_VSC
            let down = 1 | (scan << 16);
            let up = down | (1 << 30) | (1 << 31);
            PostMessageW(hwnd, 0x0100, vk as usize, down); // WM_KEYDOWN
            PostMessageW(hwnd, 0x0102, if c == '\n' { 0x0d } else { c as usize }, down); // WM_CHAR
            PostMessageW(hwnd, 0x0101, vk as usize, up); // WM_KEYUP
        }
    }
}

#[cfg(not(windows))]
pub(in crate::workspace) fn type_keys(_: &str) {}
