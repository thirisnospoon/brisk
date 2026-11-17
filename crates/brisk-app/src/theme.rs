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
    pub fn bg() -> Hsla { pick(0x1e1f22, 0xf7f8fa) }
    pub fn panel() -> Hsla { pick(0x2b2d30, 0xf7f8fa) }
    pub fn panel_alt() -> Hsla { pick(0x252629, 0xffffff) }
    pub fn border() -> Hsla { pick(0x1e1f22, 0xdfe1e5) }
    pub fn divider() -> Hsla { pick(0x393b40, 0xdfe1e5) }
    pub fn text() -> Hsla { pick(0xdfe1e5, 0x1e1f22) }
    pub fn text_muted() -> Hsla { pick(0x8c8f96, 0x5e6370) }
    pub fn text_dim() -> Hsla { pick(0x6f737a, 0x8c909c) }
    pub fn accent() -> Hsla { pick(0x3574f0, 0x3574f0) }
    pub fn selection_row() -> Hsla { pick(0x2e436e, 0xd5e1ff) }
    pub fn hover_row() -> Hsla { pick(0x35373b, 0xe6e8ed) }
    pub fn excluded() -> Hsla { pick(0xc77c32, 0xb35f00) }
    pub fn modified() -> Hsla { pick(0x6c9be8, 0x0a50a1) }
    /// The background of warning banners (file changed on disk).
    pub fn warning_bg() -> Hsla { pick(0x3d3423, 0xfff4d6) }
    pub fn warning() -> Hsla { pick(0xe0b84a, 0xa86f00) }
    /// Scrollbar thumb.
    pub fn scroll_thumb() -> Hsla {
        if Self::is_light() { gpui::hsla(0., 0., 0., 0.18) } else { gpui::hsla(0., 0., 1., 0.13) }
    }

    // Editor
    pub fn editor_bg() -> Hsla { pick(0x1e1f22, 0xffffff) }
    pub fn gutter_fg() -> Hsla { pick(0x4b5059, 0xaeb3c2) }
    pub fn gutter_fg_active() -> Hsla { pick(0xa1a3ab, 0x5e6370) }
    pub fn current_line() -> Hsla { pick(0x26282e, 0xf5f8fe) }
    pub fn caret() -> Hsla { pick(0xced0d6, 0x000000) }
    pub fn selection() -> Hsla { pick(0x214283, 0xa6d2ff) }
    pub fn indent_guide() -> Hsla { pick(0x313438, 0xebecf0) }

    // VCS
    pub fn vcs_added() -> Hsla { pick(0x549159, 0x9fd09c) }
    pub fn vcs_modified() -> Hsla { pick(0x375fad, 0x8caee0) }
    pub fn vcs_deleted() -> Hsla { pick(0x868a91, 0xb4b8bf) }
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
    pub fn search_hit() -> Hsla { pick(0x32593d, 0xfbe8a8) }
    pub fn search_current() -> Hsla { pick(0x2e7d4a, 0xf3c85a) }
    /// The bracket next to the caret and its partner.
    pub fn bracket_match() -> Hsla { pick(0x3b514d, 0xbfe3e3) }
    /// Other occurrences of the identifier under the caret (and their scrollbar marks).
    pub fn occurrence() -> Hsla { pick(0x344134, 0xedebfc) }
    pub fn occurrence_mark() -> Hsla { pick(0xa7a77c, 0x9b8fe0) }
    /// Bookmark marks in the gutter.
    pub fn bookmark() -> Hsla { pick(0x5f8dd3, 0x3574f0) }
    pub fn error() -> Hsla { pick(0xe55765, 0xd5301f) }
    /// Breakpoint circles in the gutter.
    pub fn breakpoint() -> Hsla { pick(0xdb5c5c, 0xe55765) }
    /// The line where the debugger is paused.
    pub fn exec_line() -> Hsla { pick(0x2d4670, 0xc7dbff) }
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
        pick(d, l)
    }
}

#[cfg(target_os = "windows")]
pub const MONO_FONT: &str = "Consolas";
#[cfg(target_os = "macos")]
pub const MONO_FONT: &str = "Menlo";
#[cfg(not(any(target_os = "windows", target_os = "macos")))]
pub const MONO_FONT: &str = "DejaVu Sans Mono";

#[cfg(target_os = "windows")]
pub const UI_FONT: &str = "Segoe UI";
#[cfg(target_os = "macos")]
pub const UI_FONT: &str = ".SystemUIFont";
#[cfg(not(any(target_os = "windows", target_os = "macos")))]
pub const UI_FONT: &str = "Noto Sans";
