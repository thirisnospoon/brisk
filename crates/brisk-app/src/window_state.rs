//! Where the main window opens: the last size (and whether it was maximized), else 1400 × 900,
//! always fitted into the primary monitor's work area (the screen minus the taskbar, in
//! logical pixels at its scaling). A window that would not fit opens maximized, as PyCharm
//! does on small or highly scaled screens, so the title bar is never off screen.

use brisk_platform::screen::{self, Rect};
use gpui::{px, size, App, Bounds, WindowBounds};
use serde_json::json;

const DEFAULT: (f32, f32) = (1400., 900.);

fn file() -> std::path::PathBuf {
    brisk_db::config_dir().join("window.json")
}

/// The last window: (width, height, maximized).
fn saved() -> Option<(f32, f32, bool)> {
    let v: serde_json::Value = serde_json::from_str(&std::fs::read_to_string(file()).ok()?).ok()?;
    let (w, h) = (v["width"].as_f64()? as f32, v["height"].as_f64()? as f32);
    (w >= 200. && h >= 150.).then_some((w, h, v["maximized"].as_bool().unwrap_or(false)))
}

/// The work area: the system's, else the whole primary display.
fn work_area(cx: &App) -> Option<Rect> {
    screen::primary_work_area().filter(|r| r.w > 0. && r.h > 0.).or_else(|| {
        cx.primary_display().map(|d| {
            let b = d.bounds();
            Rect { x: f32::from(b.origin.x), y: f32::from(b.origin.y), w: f32::from(b.size.width), h: f32::from(b.size.height) }
        })
    })
}

pub fn initial(cx: &App) -> WindowBounds {
    let last = saved();
    let want = last.map_or(DEFAULT, |(w, h, _)| (w, h));
    let Some(work) = work_area(cx) else {
        return WindowBounds::Windowed(Bounds::centered(None, size(px(want.0), px(want.1)), cx));
    };
    let (r, fits) = screen::place(work, want);
    let bounds = Bounds { origin: gpui::point(px(r.x), px(r.y)), size: size(px(r.w), px(r.h)) };
    if last.is_some_and(|l| l.2) || !fits {
        WindowBounds::Maximized(bounds)
    } else {
        WindowBounds::Windowed(bounds)
    }
}

/// Remembers the window's size and state for the next start (written only when it changed).
pub fn remember(window: &gpui::Window) {
    // Smokes move and size their windows: that must not become the user's next start.
    if std::env::vars_os().any(|(k, _)| k.to_string_lossy().starts_with("BRISK_SMOKE") || k == "BRISK_TOUR_SYNC") {
        return;
    }
    let maximized = window.is_maximized();
    // While maximized the restored size is what counts.
    let b = window.window_bounds().get_bounds();
    let (w, h) = (f32::from(b.size.width).round(), f32::from(b.size.height).round());
    if saved() == Some((w, h, maximized)) || w < 200. || h < 150. {
        return;
    }
    let _ = std::fs::create_dir_all(brisk_db::config_dir());
    let _ = std::fs::write(file(), json!({"width": w, "height": h, "maximized": maximized}).to_string());
}
