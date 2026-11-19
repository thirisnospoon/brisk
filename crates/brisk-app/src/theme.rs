//! Dark and light themes. Layout and density follow PyCharm's New UI; colors and icons are
//! our own. The theme in use comes from Settings (`settings::is_light`).

use brisk_syntax::Highlight;
use gpui::{rgb, Hsla};

pub struct Theme;

/// The dark color, or the light one in the light theme.
fn pick(dark: u32, light: u32) -> Hsla {
    rgb(if crate::settings::is_light() { light } else { dark }).into()
}

#[allow(dead_code)]
impl Theme {
    pub fn is_light() -> bool { crate::settings::is_light() }
    pub fn bg() -> Hsla { ui("bg", 0x1e1f22, 0xf7f8fa) }
    /// The window frame around the islands: a little darker than everything on it.
    pub fn frame() -> Hsla {
        let mut c = Self::bg();
        c.l = if crate::settings::is_light() { (c.l - 0.06).max(0.) } else { c.l * 0.66 };
        c
    }
    pub fn panel() -> Hsla { ui("panel", 0x2b2d30, 0xf7f8fa) }
    pub fn panel_alt() -> Hsla { ui("panel_alt", 0x252629, 0xffffff) }
    pub fn border() -> Hsla { ui("border", 0x1e1f22, 0xdfe1e5) }
    pub fn divider() -> Hsla { ui("divider", 0x393b40, 0xdfe1e5) }
    pub fn text() -> Hsla { ui("text", 0xdfe1e5, 0x1e1f22) }
    pub fn text_muted() -> Hsla { ui("text_muted", 0x8c8f96, 0x5e6370) }
    pub fn text_dim() -> Hsla { ui("text_dim", 0x6f737a, 0x8c909c) }
    pub fn accent() -> Hsla { ui("accent", 0x3574f0, 0x3574f0) }
    pub fn selection_row() -> Hsla { ui("selection_row", 0x2e436e, 0xd5e1ff) }
    pub fn hover_row() -> Hsla { ui("hover_row", 0x35373b, 0xe6e8ed) }
    /// Inlay hint chip (a little lighter than the editor) and its text (cool grey).
    pub fn inlay_bg() -> Hsla { ui("inlay_bg", 0x393b40, 0xebecf0) }
    pub fn inlay_fg() -> Hsla { ui("inlay_fg", 0x8c8f99, 0x6c707e) }
    /// A button while the mouse is pressed on it.
    pub fn press() -> Hsla { ui("press", 0x43454a, 0xd3d5db) }
    pub fn excluded() -> Hsla { pick(0xc77c32, 0xb35f00) }
    pub fn modified() -> Hsla { ui("modified", 0x6c9be8, 0x0a50a1) }
    /// The background of warning banners (file changed on disk).
    pub fn warning_bg() -> Hsla { pick(0x3d3423, 0xfff4d6) }
    pub fn warning() -> Hsla { pick(0xe0b84a, 0xa86f00) }
    /// Scrollbar thumb.
    pub fn scroll_thumb() -> Hsla {
        if Self::is_light() { gpui::hsla(0., 0., 0., 0.18) } else { gpui::hsla(0., 0., 1., 0.13) }
    }
    /// Scrollbar thumb under the pointer or being dragged.
    pub fn scroll_thumb_hot() -> Hsla {
        if Self::is_light() { gpui::hsla(0., 0., 0., 0.34) } else { gpui::hsla(0., 0., 1., 0.28) }
    }
    /// Scrollbar track under the pointer.
    pub fn scroll_track_hot() -> Hsla {
        if Self::is_light() { gpui::hsla(0., 0., 0., 0.05) } else { gpui::hsla(0., 0., 1., 0.05) }
    }

    // Editor
    pub fn editor_bg() -> Hsla { ui("editor_bg", 0x1e1f22, 0xffffff) }
    pub fn gutter_fg() -> Hsla { ui("gutter_fg", 0x4b5059, 0xaeb3c2) }
    pub fn gutter_fg_active() -> Hsla { ui("gutter_fg_active", 0xa1a3ab, 0x5e6370) }
    pub fn current_line() -> Hsla { ui("current_line", 0x26282e, 0xf5f8fe) }
    pub fn caret() -> Hsla { ui("caret", 0xced0d6, 0x000000) }
    pub fn selection() -> Hsla { ui("selection", 0x214283, 0xa6d2ff) }
    pub fn indent_guide() -> Hsla { ui("indent_guide", 0x313438, 0xebecf0) }

    // VCS
    pub fn vcs_added() -> Hsla { pick(0x549159, 0x9fd09c) }
    pub fn vcs_modified() -> Hsla { pick(0x375fad, 0x8caee0) }
    pub fn vcs_deleted() -> Hsla { pick(0x868a91, 0xb4b8bf) }
    /// Run with Coverage gutter bars: executed and missed lines.
    pub fn coverage_hit() -> Hsla { pick(0x4f9a52, 0x86c98a) }
    pub fn coverage_miss() -> Hsla { pick(0xb5473f, 0xe3867f) }
    /// File name colors in the project tree, tabs and the commit window.
    pub fn file_added() -> Hsla { pick(0x73bd79, 0x2f7d36) }
    pub fn file_modified() -> Hsla { pick(0x6c9be8, 0x0a50a1) }
    pub fn file_untracked() -> Hsla { pick(0xd1675a, 0xb83b2c) }
    pub fn file_deleted() -> Hsla { pick(0x8c8f96, 0x6c707e) }
    pub fn file_conflict() -> Hsla { pick(0xe55765, 0xd5301f) }
    pub fn diff_added_bg() -> Hsla { pick(0x2b3d2e, 0xe0f5e3) }
    pub fn diff_deleted_bg() -> Hsla { pick(0x3e3a3a, 0xf6e1e1) }
    pub fn diff_modified_bg() -> Hsla { pick(0x283850, 0xdde8fa) }
    pub fn diff_filler_bg() -> Hsla { pick(0x232427, 0xf2f3f5) }
    /// Search match highlight.
    pub fn search_hit() -> Hsla { ui("search_hit", 0x32593d, 0xfbe8a8) }
    pub fn search_current() -> Hsla { ui("search_current", 0x2e7d4a, 0xf3c85a) }
    /// The bracket next to the caret and its partner.
    pub fn bracket_match() -> Hsla { ui("bracket_match", 0x3b514d, 0xbfe3e3) }
    /// Other occurrences of the identifier under the caret (and their scrollbar marks).
    pub fn occurrence() -> Hsla { ui("occurrence", 0x344134, 0xedebfc) }
    pub fn occurrence_mark() -> Hsla { pick(0xa7a77c, 0x9b8fe0) }
    /// Bookmark marks in the gutter.
    pub fn bookmark() -> Hsla { ui("bookmark", 0x5f8dd3, 0x3574f0) }
    pub fn error() -> Hsla { pick(0xe55765, 0xd5301f) }
    /// Breakpoint circles in the gutter.
    pub fn breakpoint() -> Hsla { pick(0xdb5c5c, 0xe55765) }
    /// Run icons in the gutter and toolbars: a calm green.
    pub fn run_icon() -> Hsla { pick(0x5a9e60, 0x3f8f4c) }
    pub fn run_passed() -> Hsla { pick(0x5fb865, 0x369650) }
    /// The line where the debugger is paused.
    pub fn exec_line() -> Hsla { ui("exec_line", 0x2d4670, 0xc7dbff) }
    /// Lane colors in the Git log graph.
    pub fn graph_lane(i: usize) -> Hsla {
        const C: [u32; 8] = [0x6c9be8, 0x5fb865, 0xe0a85a, 0xc77dbb, 0x2aacb8, 0xe55765, 0xb3ae60, 0x9876aa];
        rgb(C[i % C.len()]).into()
    }

    pub fn syntax(h: Highlight) -> Hsla {
        use Highlight::*;
        let (d, l) = match h {
            Keyword => (0xcf8e6d, 0x0033b3),
            Function => (0x56a8f5, 0x00627a),
            FunctionBuiltin => (0x8888c6, 0x000080),
            Method => (0x56a8f5, 0x00627a),
            Type => (0xbcbec4, 0x000000),
            String => (0x6aab73, 0x067d17),
            Escape => (0xcf8e6d, 0x0037a6),
            Comment => (0x7a7e85, 0x8c8c8c),
            Number => (0x2aacb8, 0x1750eb),
            Constant => (0xc77dbb, 0x871094),
            ConstantBuiltin => (0xcf8e6d, 0x0033b3),
            Property => (0xaa4926, 0x871094),
            Decorator => (0xb3ae60, 0x9e880d),
            Operator => (0xbcbec4, 0x080808),
            Punctuation => (0xbcbec4, 0x080808),
            SelfParam => (0x94558d, 0x94558d),
        };
        if !crate::settings::is_light() {
            if let Some(c) = design_syntax(design(), h) {
                return rgb(c).into();
            }
        }
        pick(d, l)
    }
}

/// A design variant of the dark theme, picked with `BRISK_DESIGN` (graphite, midnight, mocha)
/// to compare looks; 0 is the default PyCharm-like palette.
fn design() -> u8 {
    static D: std::sync::OnceLock<u8> = std::sync::OnceLock::new();
    // "islands" is a layout, not a palette: it combines with one ("islands,graphite").
    *D.get_or_init(|| {
        let v = std::env::var("BRISK_DESIGN").unwrap_or_default();
        v.split(',')
            .map(|p| match p.trim() {
                "graphite" => 1,
                "midnight" => 2,
                "mocha" => 3,
                _ => 0,
            })
            .max()
            .unwrap_or(0)
    })
}

/// The UI scale (Settings › Appearance › UI font size, times View › Zoom IDE).
pub fn ui_scale() -> f32 {
    crate::settings::ui_scale()
}

/// `x` UI pixels at the current UI scale: font sizes, row and header heights, icon sizes.
pub fn ui_px(x: f32) -> gpui::Pixels {
    gpui::px(x * ui_scale())
}

/// The "Islands" layout: tool windows and the editor are rounded panels with gaps between
/// them on a darker frame, and editor tabs are pills. The UI style setting picks it (live);
/// `BRISK_DESIGN=islands` or `=classic` overrides the setting.
pub fn islands() -> bool {
    islands_override().unwrap_or_else(crate::settings::islands)
}

/// The UI style forced by `BRISK_DESIGN` ("islands" / "classic"), if any.
pub fn islands_override() -> Option<bool> {
    static I: std::sync::OnceLock<Option<bool>> = std::sync::OnceLock::new();
    *I.get_or_init(|| {
        let v = std::env::var("BRISK_DESIGN").unwrap_or_default();
        let parts: Vec<&str> = v.split(',').map(str::trim).collect();
        if parts.contains(&"islands") {
            Some(true)
        } else if parts.contains(&"classic") {
            Some(false)
        } else {
            None
        }
    })
}

/// A UI color: the design variant's, else the theme's.
fn ui(role: &'static str, dark: u32, light: u32) -> Hsla {
    if !crate::settings::is_light() {
        if let Some(c) = design_ui(design(), role) {
            return rgb(c).into();
        }
    }
    pick(dark, light)
}

fn design_ui(d: u8, role: &str) -> Option<u32> {
    Some(match (d, role) {
        // Graphite: macOS dark. Neutral greys, a lighter sidebar, hairline borders, system blue.
        (1, "bg") => 0x1c1c1e,
        (1, "panel") => 0x252527,
        (1, "panel_alt") => 0x202022,
        (1, "border") => 0x161618,
        (1, "divider") => 0x323235,
        (1, "text") => 0xe8e8ea,
        (1, "text_muted") => 0x98989d,
        (1, "text_dim") => 0x6e6e73,
        (1, "accent") => 0x0a84ff,
        (1, "selection_row") => 0x1e3a5f,
        (1, "hover_row") => 0x2e2e31,
        (1, "press") => 0x3a3a3d,
        (1, "editor_bg") => 0x1c1c1e,
        (1, "gutter_fg") => 0x4c4c50,
        (1, "gutter_fg_active") => 0xa1a1a6,
        (1, "current_line") => 0x232325,
        (1, "caret") => 0xffffff,
        (1, "selection") => 0x2b4a73,
        (1, "indent_guide") => 0x2c2c2f,
        (1, "modified") => 0x64a8ff,
        (1, "bookmark") => 0x0a84ff,
        (1, "exec_line") => 0x23395a,
        (1, "search_hit") => 0x5a4a1e,
        (1, "search_current") => 0x8a6d14,
        (1, "occurrence") => 0x2d3340,
        (1, "bracket_match") => 0x2f3d4d,
        // Midnight: near-black blue-greys and a violet accent, like Linear.
        (2, "bg") => 0x0e0f12,
        (2, "panel") => 0x15161a,
        (2, "panel_alt") => 0x111216,
        (2, "border") => 0x0b0c0e,
        (2, "divider") => 0x24262c,
        (2, "text") => 0xe4e5e9,
        (2, "text_muted") => 0x8a8f9c,
        (2, "text_dim") => 0x5d616d,
        (2, "accent") => 0x6e79d6,
        (2, "selection_row") => 0x23274a,
        (2, "hover_row") => 0x1c1e24,
        (2, "press") => 0x262932,
        (2, "editor_bg") => 0x0e0f12,
        (2, "gutter_fg") => 0x3a3d47,
        (2, "gutter_fg_active") => 0x9aa0ad,
        (2, "current_line") => 0x15171c,
        (2, "caret") => 0xc9ccff,
        (2, "selection") => 0x2a2f5c,
        (2, "indent_guide") => 0x1d1f25,
        (2, "modified") => 0x8f98ff,
        (2, "bookmark") => 0x6e79d6,
        (2, "exec_line") => 0x22264a,
        (2, "search_hit") => 0x3d3a1f,
        (2, "search_current") => 0x5e5420,
        (2, "occurrence") => 0x232633,
        (2, "bracket_match") => 0x2a2f45,
        // Mocha: soft warm pastels on a violet-grey base (Catppuccin Mocha).
        (3, "bg") => 0x1e1e2e,
        (3, "panel") => 0x181825,
        (3, "panel_alt") => 0x1b1b29,
        (3, "border") => 0x11111b,
        (3, "divider") => 0x2c2d3e,
        (3, "text") => 0xcdd6f4,
        (3, "text_muted") => 0xa6adc8,
        (3, "text_dim") => 0x6c7086,
        (3, "accent") => 0xcba6f7,
        (3, "selection_row") => 0x353650,
        (3, "hover_row") => 0x262738,
        (3, "press") => 0x313244,
        (3, "editor_bg") => 0x1e1e2e,
        (3, "gutter_fg") => 0x45475a,
        (3, "gutter_fg_active") => 0xb4befe,
        (3, "current_line") => 0x24253a,
        (3, "caret") => 0xf5e0dc,
        (3, "selection") => 0x3d3f5c,
        (3, "indent_guide") => 0x2a2b3d,
        (3, "modified") => 0x89b4fa,
        (3, "bookmark") => 0xcba6f7,
        (3, "exec_line") => 0x33345a,
        (3, "search_hit") => 0x4a4436,
        (3, "search_current") => 0x6b5d3c,
        (3, "occurrence") => 0x2e3047,
        (3, "bracket_match") => 0x3a3c5a,
        _ => return None,
    })
}

fn design_syntax(d: u8, h: Highlight) -> Option<u32> {
    use Highlight::*;
    Some(match d {
        // Xcode's default dark.
        1 => match h {
            Keyword | ConstantBuiltin | Escape => 0xfc5fa3,
            Function | Method => 0x67b7a4,
            FunctionBuiltin => 0xa167e6,
            Type => 0x5dd8ff,
            String => 0xfc6a5d,
            Comment => 0x6c7986,
            Number => 0xd0bf69,
            Constant => 0xa167e6,
            Property => 0x9ef1dd,
            Decorator => 0xfd8f3f,
            SelfParam => 0xfc5fa3,
            _ => 0xe8e8ea,
        },
        2 => match h {
            Keyword | ConstantBuiltin | Escape => 0xc792ea,
            Function | Method => 0x82aaff,
            FunctionBuiltin => 0x89ddff,
            Type => 0xffcb6b,
            String => 0xc3e88d,
            Comment => 0x545a6b,
            Number => 0xf78c6c,
            Constant => 0xf78c6c,
            Property => 0xb0bec5,
            Decorator => 0xffcb6b,
            SelfParam => 0xf07178,
            _ => 0xbfc4d4,
        },
        3 => match h {
            Keyword | ConstantBuiltin => 0xcba6f7,
            Escape => 0xf5c2e7,
            Function | Method => 0x89b4fa,
            FunctionBuiltin => 0xf38ba8,
            Type => 0xf9e2af,
            String => 0xa6e3a1,
            Comment => 0x6c7086,
            Number => 0xfab387,
            Constant => 0xfab387,
            Property => 0xb4befe,
            Decorator => 0xf9e2af,
            SelfParam => 0xf38ba8,
            _ => 0x9399b2,
        },
        _ => return None,
    })
}

#[cfg(target_os = "windows")]
const DEFAULT_MONO_FONT: &str = "Consolas";
#[cfg(target_os = "macos")]
const DEFAULT_MONO_FONT: &str = "Menlo";
#[cfg(not(any(target_os = "windows", target_os = "macos")))]
const DEFAULT_MONO_FONT: &str = "DejaVu Sans Mono";

#[cfg(target_os = "windows")]
const DEFAULT_UI_FONT: &str = "Segoe UI";
#[cfg(target_os = "macos")]
const DEFAULT_UI_FONT: &str = ".SystemUIFont";
#[cfg(not(any(target_os = "windows", target_os = "macos")))]
const DEFAULT_UI_FONT: &str = "Noto Sans";

/// The UI font: `BRISK_UI_FONT` overrides the platform default (for comparing fonts).
pub fn ui_font() -> &'static str {
    static F: std::sync::OnceLock<&'static str> = std::sync::OnceLock::new();
    F.get_or_init(|| env_font("BRISK_UI_FONT").unwrap_or(DEFAULT_UI_FONT))
}

/// The default editor and console font: `BRISK_MONO_FONT` overrides the platform default.
pub fn mono_font() -> &'static str {
    static F: std::sync::OnceLock<&'static str> = std::sync::OnceLock::new();
    F.get_or_init(|| env_font("BRISK_MONO_FONT").unwrap_or(DEFAULT_MONO_FONT))
}

fn env_font(var: &str) -> Option<&'static str> {
    std::env::var(var).ok().filter(|f| !f.trim().is_empty()).map(|f| &*Box::leak(f.into_boxed_str()))
}
