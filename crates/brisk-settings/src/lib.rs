//! User settings (Settings dialog, Ctrl+Alt+S): theme, editor font, size, line spacing and
//! inlay hints, smooth scrolling, the caret and sticky lines.
//! Stored in `<config dir>/settings.json`; read by painting code through cheap globals.

use std::path::Path;
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
    /// Editor › General › Appearance: "Show whitespaces" (the default for every editor).
    pub show_whitespaces: bool,
    /// Editor › General › Soft Wraps: "Soft-wrap these files" (`;`-separated masks).
    pub soft_wrap_files: String,
    /// "Use original line's indent for wrapped parts", plus this many columns.
    pub soft_wrap_indent: bool,
    pub soft_wrap_extra_indent: u32,
    /// Code Style › Python › Code Generation: "Line comment at first column" and
    /// "Add a space at line comment start" (both on in PyCharm).
    pub line_comment_first_column: bool,
    pub line_comment_space: bool,
    /// Appearance: popups, dialogs and tool windows ease in (off: they appear at once).
    pub animations: bool,
    /// System: look for a new Brisk release once a day.
    pub check_updates: bool,
}

/// PyCharm's default for "Soft-wrap these files".
pub const DEFAULT_SOFT_WRAP_FILES: &str = "*.md; *.txt; *.rst; *.adoc";

impl Default for Settings {
    fn default() -> Self {
        Self {
            light: false,
            editor_font: brisk_platform::fonts::mono().to_string(),
            font_size: DEFAULT_FONT_SIZE,
            line_spacing: 1.0,
            inlay_hints: true,
            islands: false,
            tab_limit: DEFAULT_TAB_LIMIT,
            tab_rows: false,
            ui_font_size: DEFAULT_UI_FONT_SIZE,
            ui_zoom: 1.0,
            reformat_on_save: false,
            optimize_imports_on_save: false,
            smooth_scrolling: true,
            caret_blinking: true,
            smooth_caret: false,
            sticky_lines: true,
            soft_wrap_files: DEFAULT_SOFT_WRAP_FILES.into(),
            soft_wrap_indent: true,
            soft_wrap_extra_indent: 0,
            line_comment_first_column: true,
            line_comment_space: true,
            show_whitespaces: false,
            animations: true,
            check_updates: true,
        }
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
static SHOW_WHITESPACES: AtomicBool = AtomicBool::new(false);
static ANIMATIONS: AtomicBool = AtomicBool::new(true);
/// The font size in use (the setting, or a zoomed size: Alt+Shift+. / Ctrl+wheel).
static FONT_SIZE: AtomicU32 = AtomicU32::new(0);
static SPACING: AtomicU32 = AtomicU32::new(0);
static CURRENT: RwLock<Option<Settings>> = RwLock::new(None);

fn path() -> std::path::PathBuf {
    brisk_platform::dirs::config_dir().join("settings.json")
}

pub fn current() -> Settings {
    CURRENT.read().ok().and_then(|s| s.clone()).unwrap_or_default()
}

/// Read the saved settings (at start-up).
pub fn load() -> Settings {
    let s = load_file();
    if let Ok(mut l) = LOADED.lock() {
        *l = Some(to_json(&s));
    }
    s
}

/// What this process last read or wrote: `save` writes only the keys that differ from it,
/// so two Brisk windows (two processes) do not undo each other's changes.
static LOADED: std::sync::Mutex<Option<serde_json::Value>> = std::sync::Mutex::new(None);

fn load_file() -> Settings {
    let s = read(&path());
    apply(&s);
    s
}

/// The settings in the file `p`: defaults for a missing or broken file, and for each key it
/// lacks.
fn read(p: &Path) -> Settings {
    let mut s = Settings::default();
    if let Ok(text) = std::fs::read_to_string(p) {
        let parsed = serde_json::from_str::<serde_json::Value>(&text);
        if parsed.is_err() && !text.trim().is_empty() {
            // Hand-edited into invalid JSON: keep a copy before defaults overwrite it.
            let _ = std::fs::copy(p, p.with_extension("json.bak"));
        }
        if let Ok(v) = parsed {
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
            s.show_whitespaces = flag("show_whitespaces").unwrap_or(s.show_whitespaces);
            s.animations = flag("animations").unwrap_or(s.animations);
            s.check_updates = flag("check_updates").unwrap_or(s.check_updates);
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
    SHOW_WHITESPACES.store(s.show_whitespaces, Ordering::Relaxed);
    ANIMATIONS.store(s.animations, Ordering::Relaxed);
    // An editor zoom (Alt+Shift+. / Ctrl+wheel) lasts until the font size setting itself
    // changes: editing another setting, or Cancel after not touching the size, keeps it.
    let size_changed = CURRENT.read().ok().and_then(|c| c.as_ref().map(|c| c.font_size)) != Some(s.font_size);
    if size_changed || FONT_SIZE.load(Ordering::Relaxed) == 0 {
        FONT_SIZE.store(s.font_size.to_bits(), Ordering::Relaxed);
    }
    SPACING.store(s.line_spacing.to_bits(), Ordering::Relaxed);
    if let Ok(mut c) = CURRENT.write() {
        *c = Some(s.clone());
    }
}

pub fn save(s: &Settings) -> std::io::Result<()> {
    let mut loaded = LOADED.lock().unwrap_or_else(|e| e.into_inner());
    save_at(&path(), s, &mut loaded)
}

/// Write `s` to the file `p`. Keys the file has and Brisk does not know stay, and only the
/// keys whose value differs from `loaded` (what this process last read or wrote) are
/// written; then `loaded` becomes `s`.
fn save_at(p: &Path, s: &Settings, loaded: &mut Option<serde_json::Value>) -> std::io::Result<()> {
    let new = to_json(s);
    let mut merged = std::fs::read_to_string(p)
        .ok()
        .and_then(|t| serde_json::from_str::<serde_json::Value>(&t).ok())
        .filter(|v| v.is_object())
        .unwrap_or_else(|| serde_json::json!({}));
    if let (Some(m), Some(n)) = (merged.as_object_mut(), new.as_object()) {
        for (k, v) in n {
            if loaded.as_ref().and_then(|l| l.get(k)) != Some(v) {
                m.insert(k.clone(), v.clone());
            }
        }
    }
    if let Some(d) = p.parent() {
        std::fs::create_dir_all(d)?;
    }
    brisk_platform::fs::write_atomic(p, serde_json::to_string_pretty(&merged).unwrap_or_default().as_bytes())?;
    *loaded = Some(new);
    Ok(())
}

fn to_json(s: &Settings) -> serde_json::Value {
    serde_json::json!({
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
        "show_whitespaces": s.show_whitespaces,
        "animations": s.animations,
        "check_updates": s.check_updates,
        "soft_wrap_files": s.soft_wrap_files,
        "soft_wrap_indent": s.soft_wrap_indent,
        "soft_wrap_extra_indent": s.soft_wrap_extra_indent,
        "line_comment_first_column": s.line_comment_first_column,
        "line_comment_space": s.line_comment_space,
    })
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

pub fn animations() -> bool {
    ANIMATIONS.load(Ordering::Relaxed)
}

pub fn show_whitespaces() -> bool {
    SHOW_WHITESPACES.load(Ordering::Relaxed)
}

pub fn sticky_lines() -> bool {
    STICKY_LINES.load(Ordering::Relaxed)
}

pub fn inlay_hints() -> bool {
    INLAY_HINTS.load(Ordering::Relaxed)
}

pub fn font_size() -> f32 {
    let v = f32::from_bits(FONT_SIZE.load(Ordering::Relaxed));
    if v > 0.0 {
        v
    } else {
        DEFAULT_FONT_SIZE
    }
}

pub fn line_height() -> f32 {
    let sp = f32::from_bits(SPACING.load(Ordering::Relaxed));
    let sp = if sp > 0.0 { sp } else { 1.0 };
    (font_size() * LINE_FACTOR * sp).round().max(8.0)
}

pub fn editor_font() -> String {
    CURRENT.read().ok().and_then(|s| s.as_ref().map(|s| s.editor_font.clone())).unwrap_or_else(|| brisk_platform::fonts::mono().to_string())
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

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;
    use std::path::PathBuf;

    /// `settings.json` in a new, empty folder.
    fn file(name: &str) -> PathBuf {
        let dir = std::env::temp_dir().join(format!("brisk-settings-{name}-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        dir.join("settings.json")
    }

    fn json_of(p: &Path) -> serde_json::Value {
        serde_json::from_str(&std::fs::read_to_string(p).unwrap()).unwrap()
    }

    fn cleanup(p: &Path) {
        if let Some(d) = p.parent() {
            let _ = std::fs::remove_dir_all(d);
        }
    }

    /// Every setting away from its default.
    fn changed() -> Settings {
        Settings {
            light: true,
            editor_font: "JetBrains Mono".into(),
            font_size: 15.5,
            line_spacing: 1.2,
            inlay_hints: false,
            islands: true,
            tab_limit: 25,
            tab_rows: true,
            ui_font_size: 14.0,
            ui_zoom: 1.25,
            reformat_on_save: true,
            optimize_imports_on_save: true,
            smooth_scrolling: false,
            caret_blinking: false,
            smooth_caret: true,
            sticky_lines: false,
            show_whitespaces: true,
            soft_wrap_files: "*.md".into(),
            soft_wrap_indent: false,
            soft_wrap_extra_indent: 4,
            line_comment_first_column: false,
            line_comment_space: false,
            animations: false,
            check_updates: false,
        }
    }

    #[test]
    fn round_trip() {
        let p = file("round-trip");
        save_at(&p, &changed(), &mut None).unwrap();
        assert_eq!(read(&p), changed());
        save_at(&p, &Settings::default(), &mut None).unwrap();
        assert_eq!(read(&p), Settings::default());
        cleanup(&p);
    }

    /// The key names and value forms of `settings.json`: a change here makes Brisk lose the
    /// settings users already have.
    #[test]
    fn file_format() {
        let p = file("format");
        let s = Settings { editor_font: "Consolas".into(), ..Settings::default() };
        save_at(&p, &s, &mut None).unwrap();
        let text = std::fs::read_to_string(&p).unwrap();
        assert!(text.starts_with("{\n  \""), "{text}");
        let expected = json!({
            "theme": "dark",
            "editor_font": "Consolas",
            "font_size": 13.5,
            "line_spacing": 1.0,
            "inlay_hints": true,
            "ui_style": "classic",
            "tab_limit": 10,
            "tab_rows": false,
            "ui_font_size": 13.0,
            "ui_zoom": 1.0,
            "reformat_on_save": false,
            "optimize_imports_on_save": false,
            "smooth_scrolling": true,
            "caret_blinking": true,
            "smooth_caret": false,
            "sticky_lines": true,
            "show_whitespaces": false,
            "animations": true,
            "check_updates": true,
            "soft_wrap_files": "*.md; *.txt; *.rst; *.adoc",
            "soft_wrap_indent": true,
            "soft_wrap_extra_indent": 0,
            "line_comment_first_column": true,
            "line_comment_space": true,
        });
        assert_eq!(json_of(&p), expected);
        // An old file with a few keys: those are read, the rest are defaults; values out of
        // range are clamped.
        std::fs::write(&p, r#"{"theme": "light", "font_size": 99, "ui_style": "islands"}"#).unwrap();
        assert_eq!(read(&p), Settings { light: true, font_size: MAX_FONT_SIZE, islands: true, ..Settings::default() });
        cleanup(&p);
    }

    #[test]
    fn unknown_keys_and_other_windows_changes_are_kept() {
        let p = file("merge");
        std::fs::write(&p, r#"{"theme": "light", "future_option": {"x": [1, 2]}}"#).unwrap();
        let s = read(&p);
        let mut loaded = Some(to_json(&s));
        // Another Brisk window saves a font size meanwhile.
        let mut other = json_of(&p);
        other["font_size"] = json!(18.0);
        std::fs::write(&p, other.to_string()).unwrap();
        // This one changes only the tab rows.
        let saved = Settings { tab_rows: true, ..s };
        save_at(&p, &saved, &mut loaded).unwrap();
        let v = json_of(&p);
        assert_eq!(v["future_option"], json!({"x": [1, 2]}));
        assert_eq!(v["theme"], json!("light"));
        assert_eq!(v["font_size"], json!(18.0));
        assert_eq!(v["tab_rows"], json!(true));
        assert_eq!(loaded, Some(to_json(&saved)));
        cleanup(&p);
    }

    #[test]
    fn broken_file_is_kept_as_bak() {
        let p = file("broken");
        let broken = "{ \"theme\": \"light\", ";
        std::fs::write(&p, broken).unwrap();
        assert_eq!(read(&p), Settings::default());
        assert_eq!(std::fs::read_to_string(p.with_extension("json.bak")).unwrap(), broken);
        assert_eq!(std::fs::read_to_string(&p).unwrap(), broken);
        // An empty file is not worth a copy.
        std::fs::remove_file(p.with_extension("json.bak")).unwrap();
        std::fs::write(&p, "  \n").unwrap();
        assert_eq!(read(&p), Settings::default());
        assert!(!p.with_extension("json.bak").exists());
        cleanup(&p);
    }
}
