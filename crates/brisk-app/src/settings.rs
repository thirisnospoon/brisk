//! User settings (Settings dialog, Ctrl+Alt+S): theme, editor font, size and line spacing.
//! Stored in `<config dir>/settings.json`; read by painting code through cheap globals.

use std::sync::atomic::{AtomicBool, AtomicU32, Ordering};
use std::sync::RwLock;

pub const DEFAULT_FONT_SIZE: f32 = 13.5;
pub const MIN_FONT_SIZE: f32 = 8.0;
pub const MAX_FONT_SIZE: f32 = 40.0;
/// Line height = font size × this × line spacing (13.5 → 20 px, as before).
const LINE_FACTOR: f32 = 20.0 / 13.5;

#[derive(Clone, Debug, PartialEq)]
pub struct Settings {
    pub light: bool,
    pub editor_font: String,
    pub font_size: f32,
    pub line_spacing: f32,
}

impl Default for Settings {
    fn default() -> Self {
        Self { light: false, editor_font: crate::theme::MONO_FONT.to_string(), font_size: DEFAULT_FONT_SIZE, line_spacing: 1.0 }
    }
}

static LIGHT: AtomicBool = AtomicBool::new(false);
/// The font size in use (the setting, or a zoomed size: Alt+Shift+. / Ctrl+wheel).
static FONT_SIZE: AtomicU32 = AtomicU32::new(0);
static SPACING: AtomicU32 = AtomicU32::new(0);
static CURRENT: RwLock<Option<Settings>> = RwLock::new(None);

fn path() -> std::path::PathBuf {
    brisk_db::config_dir().join("settings.json")
}

pub fn current() -> Settings {
    CURRENT.read().ok().and_then(|s| s.clone()).unwrap_or_default()
}

/// Read the saved settings (at start-up).
pub fn load() -> Settings {
    let mut s = Settings::default();
    if let Ok(text) = std::fs::read_to_string(path()) {
        if let Ok(v) = serde_json::from_str::<serde_json::Value>(&text) {
            if let Some(t) = v.get("theme").and_then(|t| t.as_str()) {
                s.light = t == "light";
            }
            if let Some(f) = v.get("editor_font").and_then(|t| t.as_str()).filter(|f| !f.trim().is_empty()) {
                s.editor_font = f.to_string();
            }
            if let Some(n) = v.get("font_size").and_then(|t| t.as_f64()) {
                s.font_size = (n as f32).clamp(MIN_FONT_SIZE, MAX_FONT_SIZE);
            }
            if let Some(n) = v.get("line_spacing").and_then(|t| t.as_f64()) {
                s.line_spacing = (n as f32).clamp(0.8, 2.0);
            }
        }
    }
    apply(&s);
    s
}

/// Make `s` current (painting picks it up on the next frame).
pub fn apply(s: &Settings) {
    LIGHT.store(s.light, Ordering::Relaxed);
    FONT_SIZE.store(s.font_size.to_bits(), Ordering::Relaxed);
    SPACING.store(s.line_spacing.to_bits(), Ordering::Relaxed);
    if let Ok(mut c) = CURRENT.write() {
        *c = Some(s.clone());
    }
}

pub fn save(s: &Settings) -> std::io::Result<()> {
    let v = serde_json::json!({
        "theme": if s.light { "light" } else { "dark" },
        "editor_font": s.editor_font,
        "font_size": (s.font_size as f64 * 10.0).round() / 10.0,
        "line_spacing": (s.line_spacing as f64 * 10.0).round() / 10.0,
    });
    let p = path();
    if let Some(d) = p.parent() {
        std::fs::create_dir_all(d)?;
    }
    std::fs::write(p, serde_json::to_string_pretty(&v).unwrap_or_default())
}

pub fn is_light() -> bool {
    LIGHT.load(Ordering::Relaxed)
}

pub fn font_size() -> f32 {
    let v = f32::from_bits(FONT_SIZE.load(Ordering::Relaxed));
    if v > 0.0 { v } else { DEFAULT_FONT_SIZE }
}

pub fn line_height() -> f32 {
    let sp = f32::from_bits(SPACING.load(Ordering::Relaxed));
    let sp = if sp > 0.0 { sp } else { 1.0 };
    (font_size() * LINE_FACTOR * sp).round().max(8.0)
}

pub fn editor_font() -> String {
    CURRENT.read().ok().and_then(|s| s.as_ref().map(|s| s.editor_font.clone())).unwrap_or_else(|| crate::theme::MONO_FONT.to_string())
}

/// Zoom all editors (not saved, like PyCharm's "Change Font Size"): `delta` points, or back
/// to the setting with `None`.
pub fn zoom(delta: Option<f32>) -> f32 {
    let size = match delta {
        Some(d) => (font_size() + d).clamp(MIN_FONT_SIZE, MAX_FONT_SIZE),
        None => current().font_size,
    };
    FONT_SIZE.store(size.to_bits(), Ordering::Relaxed);
    size
}

/// Monospaced fonts offered in Settings, in this order, when installed.
pub const FONT_CHOICES: &[&str] = &[
    "JetBrains Mono",
    "Cascadia Code",
    "Cascadia Mono",
    "Consolas",
    "Fira Code",
    "Source Code Pro",
    "Courier New",
    "Lucida Console",
    "Menlo",
    "DejaVu Sans Mono",
];
