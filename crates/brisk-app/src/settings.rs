//! User settings (Settings dialog, Ctrl+Alt+S): theme, editor font, size, line spacing and
//! inlay hints, smooth scrolling, the caret and sticky lines.
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
    /// Inlay hints (inferred types, parameter names) in the editor.
    pub inlay_hints: bool,
    /// UI style: "Islands" (rounded panels on a darker frame) instead of "Classic".
    pub islands: bool,
    /// Editor Tabs: how many tabs a group keeps open; past it the least recently used
    /// unpinned, unmodified tab closes.
    pub tab_limit: u32,
    /// Editor Tabs: "Show tabs in multiple rows" instead of one scrolling row.
    pub tab_rows: bool,
    /// Appearance: the UI font size (11–16); the whole UI scales with it (13 = 100%).
    pub ui_font_size: f32,
    /// View › Appearance › Zoom IDE (Ctrl+Alt+= / Ctrl+Alt+- / Ctrl+Alt+0): 1.0 = 100%.
    pub ui_zoom: f32,
    /// Tools › Actions on Save (PyCharm): Reformat code and Optimize imports, with ruff.
    pub reformat_on_save: bool,
    pub optimize_imports_on_save: bool,
    /// Editor › General › Scrolling: "Enable smooth scrolling".
    pub smooth_scrolling: bool,
    /// Editor › General › Appearance: "Caret blinking".
    pub caret_blinking: bool,
    /// The caret slides to its new place instead of jumping (off: PyCharm's caret jumps).
    pub smooth_caret: bool,
    /// Editor › General › Sticky Lines: "Show sticky lines".
    pub sticky_lines: bool,
    /// Editor › General › Soft Wraps: "Soft-wrap these files" (`;`-separated masks).
    pub soft_wrap_files: String,
    /// "Use original line's indent for wrapped parts", plus this many columns.
    pub soft_wrap_indent: bool,
    pub soft_wrap_extra_indent: u32,
    /// Code Style › Python › Code Generation: "Line comment at first column" and
    /// "Add a space at line comment start" (both on in PyCharm).
    pub line_comment_first_column: bool,
    pub line_comment_space: bool,
}

/// PyCharm's default for "Soft-wrap these files".
pub const DEFAULT_SOFT_WRAP_FILES: &str = "*.md; *.txt; *.rst; *.adoc";

impl Default for Settings {
    fn default() -> Self {
        Self { light: false, editor_font: crate::theme::mono_font().to_string(), font_size: DEFAULT_FONT_SIZE, line_spacing: 1.0, inlay_hints: true, islands: false, tab_limit: DEFAULT_TAB_LIMIT, tab_rows: false, ui_font_size: DEFAULT_UI_FONT_SIZE, ui_zoom: 1.0, reformat_on_save: false, optimize_imports_on_save: false, smooth_scrolling: true, caret_blinking: true, smooth_caret: false, sticky_lines: true, soft_wrap_files: DEFAULT_SOFT_WRAP_FILES.into(), soft_wrap_indent: true, soft_wrap_extra_indent: 0, line_comment_first_column: true, line_comment_space: true }
    }
}

static LIGHT: AtomicBool = AtomicBool::new(false);
static INLAY_HINTS: AtomicBool = AtomicBool::new(true);
static ISLANDS: AtomicBool = AtomicBool::new(false);
static TAB_LIMIT: AtomicU32 = AtomicU32::new(DEFAULT_TAB_LIMIT);
static TAB_ROWS: AtomicBool = AtomicBool::new(false);
pub const DEFAULT_TAB_LIMIT: u32 = 10;
static UI_FONT_SIZE: AtomicU32 = AtomicU32::new(0);
static UI_ZOOM: AtomicU32 = AtomicU32::new(0);
pub const DEFAULT_UI_FONT_SIZE: f32 = 13.;
pub const MIN_UI_FONT_SIZE: f32 = 11.;
pub const MAX_UI_FONT_SIZE: f32 = 16.;
pub const MIN_UI_ZOOM: f32 = 0.5;
pub const MAX_UI_ZOOM: f32 = 2.0;
static SMOOTH_SCROLLING: AtomicBool = AtomicBool::new(true);
static CARET_BLINKING: AtomicBool = AtomicBool::new(true);
static SMOOTH_CARET: AtomicBool = AtomicBool::new(false);
static STICKY_LINES: AtomicBool = AtomicBool::new(true);
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
            if let Some(b) = v.get("inlay_hints").and_then(|t| t.as_bool()) {
                s.inlay_hints = b;
            }
            if let Some(t) = v.get("ui_style").and_then(|t| t.as_str()) {
                s.islands = t == "islands";
            }
            if let Some(n) = v.get("tab_limit").and_then(|t| t.as_u64()) {
                s.tab_limit = (n as u32).clamp(1, 100);
            }
            if let Some(b) = v.get("tab_rows").and_then(|t| t.as_bool()) {
                s.tab_rows = b;
            }
            if let Some(n) = v.get("ui_font_size").and_then(|t| t.as_f64()) {
                s.ui_font_size = (n as f32).clamp(MIN_UI_FONT_SIZE, MAX_UI_FONT_SIZE);
            }
            if let Some(b) = v.get("reformat_on_save").and_then(|t| t.as_bool()) {
                s.reformat_on_save = b;
            }
            if let Some(b) = v.get("optimize_imports_on_save").and_then(|t| t.as_bool()) {
                s.optimize_imports_on_save = b;
            }
            if let Some(n) = v.get("ui_zoom").and_then(|t| t.as_f64()) {
                s.ui_zoom = (n as f32).clamp(MIN_UI_ZOOM, MAX_UI_ZOOM);
            }
            let flag = |k: &str| v.get(k).and_then(|t| t.as_bool());
            s.smooth_scrolling = flag("smooth_scrolling").unwrap_or(s.smooth_scrolling);
            s.caret_blinking = flag("caret_blinking").unwrap_or(s.caret_blinking);
            s.smooth_caret = flag("smooth_caret").unwrap_or(s.smooth_caret);
            s.sticky_lines = flag("sticky_lines").unwrap_or(s.sticky_lines);
            if let Some(t) = v.get("soft_wrap_files").and_then(|t| t.as_str()) {
                s.soft_wrap_files = t.to_string();
            }
            if let Some(b) = v.get("soft_wrap_indent").and_then(|t| t.as_bool()) {
                s.soft_wrap_indent = b;
            }
            if let Some(n) = v.get("soft_wrap_extra_indent").and_then(|t| t.as_u64()) {
                s.soft_wrap_extra_indent = (n as u32).min(16);
            }
            if let Some(b) = v.get("line_comment_first_column").and_then(|t| t.as_bool()) {
                s.line_comment_first_column = b;
            }
            if let Some(b) = v.get("line_comment_space").and_then(|t| t.as_bool()) {
                s.line_comment_space = b;
            }
        }
    }
    apply(&s);
    s
}

/// Make `s` current (painting picks it up on the next frame).
pub fn apply(s: &Settings) {
    LIGHT.store(s.light, Ordering::Relaxed);
    INLAY_HINTS.store(s.inlay_hints, Ordering::Relaxed);
    ISLANDS.store(s.islands, Ordering::Relaxed);
    TAB_LIMIT.store(s.tab_limit.clamp(1, 100), Ordering::Relaxed);
    TAB_ROWS.store(s.tab_rows, Ordering::Relaxed);
    UI_FONT_SIZE.store(s.ui_font_size.clamp(MIN_UI_FONT_SIZE, MAX_UI_FONT_SIZE).to_bits(), Ordering::Relaxed);
    UI_ZOOM.store(s.ui_zoom.clamp(MIN_UI_ZOOM, MAX_UI_ZOOM).to_bits(), Ordering::Relaxed);
    SMOOTH_SCROLLING.store(s.smooth_scrolling, Ordering::Relaxed);
    CARET_BLINKING.store(s.caret_blinking, Ordering::Relaxed);
    SMOOTH_CARET.store(s.smooth_caret, Ordering::Relaxed);
    STICKY_LINES.store(s.sticky_lines, Ordering::Relaxed);
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
        "inlay_hints": s.inlay_hints,
        "ui_style": if s.islands { "islands" } else { "classic" },
        "tab_limit": s.tab_limit,
        "tab_rows": s.tab_rows,
        "ui_font_size": (s.ui_font_size as f64 * 10.0).round() / 10.0,
        "ui_zoom": (s.ui_zoom as f64 * 100.0).round() / 100.0,
        "reformat_on_save": s.reformat_on_save,
        "optimize_imports_on_save": s.optimize_imports_on_save,
        "smooth_scrolling": s.smooth_scrolling,
        "caret_blinking": s.caret_blinking,
        "smooth_caret": s.smooth_caret,
        "sticky_lines": s.sticky_lines,
        "soft_wrap_files": s.soft_wrap_files,
        "soft_wrap_indent": s.soft_wrap_indent,
        "soft_wrap_extra_indent": s.soft_wrap_extra_indent,
        "line_comment_first_column": s.line_comment_first_column,
        "line_comment_space": s.line_comment_space,
    });
    let p = path();
    if let Some(d) = p.parent() {
        std::fs::create_dir_all(d)?;
    }
    std::fs::write(p, serde_json::to_string_pretty(&v).unwrap_or_default())
}

/// Whether a new editor for `path` starts with Soft-Wrap on ("Soft-wrap these files").
pub fn soft_wrap_for(path: &std::path::Path) -> bool {
    let name = path.file_name().map(|n| n.to_string_lossy().to_lowercase()).unwrap_or_default();
    current().soft_wrap_files.split([';', ',']).map(|m| m.trim().to_lowercase()).filter(|m| !m.is_empty()).any(|m| mask_matches(&m, &name))
}

/// `*` (any run) and `?` (one char) in a file name mask.
fn mask_matches(mask: &str, name: &str) -> bool {
    fn go(m: &[char], n: &[char]) -> bool {
        match m.first() {
            None => n.is_empty(),
            Some('*') => (0..=n.len()).any(|i| go(&m[1..], &n[i..])),
            Some('?') => !n.is_empty() && go(&m[1..], &n[1..]),
            Some(c) => n.first() == Some(c) && go(&m[1..], &n[1..]),
        }
    }
    go(&mask.chars().collect::<Vec<_>>(), &name.chars().collect::<Vec<_>>())
}

pub fn soft_wrap_indent() -> bool {
    current().soft_wrap_indent
}

pub fn soft_wrap_extra_indent() -> usize {
    current().soft_wrap_extra_indent as usize
}

/// (line comment at first column, add a space after the comment mark).
pub fn line_comment_style() -> (bool, bool) {
    let s = current();
    (s.line_comment_first_column, s.line_comment_space)
}

pub fn is_light() -> bool {
    LIGHT.load(Ordering::Relaxed)
}

/// The UI style setting (`theme::islands()` is what is in effect: BRISK_DESIGN overrides it).
pub fn islands() -> bool {
    ISLANDS.load(Ordering::Relaxed)
}

/// The UI scale: the UI font size against the default 13, times the IDE zoom. The editor
/// font has its own size and does not follow it.
pub fn ui_scale() -> f32 {
    let f = f32::from_bits(UI_FONT_SIZE.load(Ordering::Relaxed));
    let z = f32::from_bits(UI_ZOOM.load(Ordering::Relaxed));
    let f = if f > 0.0 { f } else { DEFAULT_UI_FONT_SIZE };
    let z = if z > 0.0 { z } else { 1.0 };
    f / DEFAULT_UI_FONT_SIZE * z
}

/// Zoom IDE: `delta` (±0.1 = ±10%) or back to 100% with `None`; saved like PyCharm's.
/// Returns the new zoom.
pub fn zoom_ide(delta: Option<f32>) -> f32 {
    let mut s = current();
    s.ui_zoom = match delta {
        Some(d) => (((s.ui_zoom + d) * 10.0).round() / 10.0).clamp(MIN_UI_ZOOM, MAX_UI_ZOOM),
        None => 1.0,
    };
    apply(&s);
    let _ = save(&s);
    s.ui_zoom
}

pub fn tab_limit() -> usize {
    TAB_LIMIT.load(Ordering::Relaxed) as usize
}

pub fn tab_rows() -> bool {
    TAB_ROWS.load(Ordering::Relaxed)
}

pub fn smooth_scrolling() -> bool {
    SMOOTH_SCROLLING.load(Ordering::Relaxed)
}

pub fn caret_blinking() -> bool {
    CARET_BLINKING.load(Ordering::Relaxed)
}

pub fn smooth_caret() -> bool {
    SMOOTH_CARET.load(Ordering::Relaxed)
}

pub fn sticky_lines() -> bool {
    STICKY_LINES.load(Ordering::Relaxed)
}

pub fn inlay_hints() -> bool {
    INLAY_HINTS.load(Ordering::Relaxed)
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
    CURRENT.read().ok().and_then(|s| s.as_ref().map(|s| s.editor_font.clone())).unwrap_or_else(|| crate::theme::mono_font().to_string())
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
