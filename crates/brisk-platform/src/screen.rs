//! The primary monitor's work area: the screen minus the taskbar (wherever it is docked),
//! in logical pixels, for sizing a new window so it always fits.

/// A rectangle in logical pixels.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Rect {
    pub x: f32,
    pub y: f32,
    pub w: f32,
    pub h: f32,
}

/// The primary monitor's work area, or `None` where the system does not say (then the
/// caller uses the whole display).
pub fn primary_work_area() -> Option<Rect> {
    sys::primary_work_area()
}

/// Where a window of `want` (logical width, height) goes in `work`: never larger than 90 %
/// of it, centered. `None` when `want` does not fit with a margin: open maximized then.
pub fn place(work: Rect, want: (f32, f32)) -> (Rect, bool) {
    let w = want.0.min(work.w * 0.9).max(400f32.min(work.w));
    let h = want.1.min(work.h * 0.9).max(300f32.min(work.h));
    let fits = want.0 + 40. <= work.w && want.1 + 40. <= work.h;
    (Rect { x: work.x + (work.w - w) / 2., y: work.y + (work.h - h) / 2., w, h }, fits)
}

#[cfg(windows)]
mod sys {
    use super::Rect;
    use windows_sys::Win32::Foundation::POINT;
    use windows_sys::Win32::Graphics::Gdi::{GetMonitorInfoW, MonitorFromPoint, MONITORINFO, MONITOR_DEFAULTTOPRIMARY};
    use windows_sys::Win32::UI::HiDpi::{GetDpiForMonitor, MDT_EFFECTIVE_DPI};

    pub fn primary_work_area() -> Option<Rect> {
        unsafe {
            // (0, 0) is always on the primary monitor.
            let m = MonitorFromPoint(POINT { x: 0, y: 0 }, MONITOR_DEFAULTTOPRIMARY);
            if m.is_null() {
                return None;
            }
            let mut info: MONITORINFO = std::mem::zeroed();
            info.cbSize = std::mem::size_of::<MONITORINFO>() as u32;
            if GetMonitorInfoW(m, &mut info) == 0 {
                return None;
            }
            let (mut dx, mut dy) = (96u32, 96u32);
            let scale = if GetDpiForMonitor(m, MDT_EFFECTIVE_DPI, &mut dx, &mut dy) == 0 && dx > 0 { dx as f32 / 96. } else { 1. };
            let r = info.rcWork;
            Some(Rect { x: r.left as f32 / scale, y: r.top as f32 / scale, w: (r.right - r.left) as f32 / scale, h: (r.bottom - r.top) as f32 / scale })
        }
    }
}

#[cfg(not(windows))]
mod sys {
    pub fn primary_work_area() -> Option<super::Rect> {
        None
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn fits_every_screen() {
        // 1080p at 100 %, 125 %, 150 %, 175 %; a taskbar on the left; a tiny screen.
        for work in [
            Rect { x: 0., y: 0., w: 1920., h: 1032. },
            Rect { x: 0., y: 0., w: 1536., h: 826. },
            Rect { x: 0., y: 0., w: 1280., h: 688. },
            Rect { x: 0., y: 0., w: 1097., h: 590. },
            Rect { x: 62., y: 0., w: 1858., h: 1080. },
            Rect { x: 0., y: 0., w: 800., h: 560. },
        ] {
            let (r, _) = place(work, (1400., 900.));
            assert!(r.x >= work.x && r.y >= work.y && r.x + r.w <= work.x + work.w + 0.01 && r.y + r.h <= work.y + work.h + 0.01, "{work:?} -> {r:?}");
        }
        assert!(place(Rect { x: 0., y: 0., w: 1920., h: 1032. }, (1400., 900.)).1);
        assert!(!place(Rect { x: 0., y: 0., w: 1536., h: 826. }, (1400., 900.)).1);
    }
}

#[cfg(all(test, windows))]
mod real {
    #[test]
    fn this_machine_has_a_work_area() {
        let r = super::primary_work_area().expect("work area");
        eprintln!("work area: {r:?}");
        assert!(r.w > 300. && r.h > 200.);
    }
}
