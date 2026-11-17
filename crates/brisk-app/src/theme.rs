//! Dark theme. Layout and density follow PyCharm's New UI; colors and icons are our own.

use brisk_syntax::Highlight;
use gpui::{rgb, Hsla};

pub struct Theme;

#[allow(dead_code)]
impl Theme {
    pub fn bg() -> Hsla { rgb(0x1e1f22).into() }
    pub fn panel() -> Hsla { rgb(0x2b2d30).into() }
    pub fn panel_alt() -> Hsla { rgb(0x252629).into() }
    pub fn border() -> Hsla { rgb(0x1e1f22).into() }
    pub fn divider() -> Hsla { rgb(0x393b40).into() }
    pub fn text() -> Hsla { rgb(0xdfe1e5).into() }
    pub fn text_muted() -> Hsla { rgb(0x8c8f96).into() }
    pub fn text_dim() -> Hsla { rgb(0x6f737a).into() }
    pub fn accent() -> Hsla { rgb(0x3574f0).into() }
    pub fn selection_row() -> Hsla { rgb(0x2e436e).into() }
    pub fn hover_row() -> Hsla { rgb(0x35373b).into() }
    pub fn excluded() -> Hsla { rgb(0xc77c32).into() }
    pub fn modified() -> Hsla { rgb(0x6c9be8).into() }

    // Editor
    pub fn editor_bg() -> Hsla { rgb(0x1e1f22).into() }
    pub fn gutter_fg() -> Hsla { rgb(0x4b5059).into() }
    pub fn gutter_fg_active() -> Hsla { rgb(0xa1a3ab).into() }
    pub fn current_line() -> Hsla { rgb(0x26282e).into() }
    pub fn caret() -> Hsla { rgb(0xced0d6).into() }
    pub fn selection() -> Hsla { rgb(0x214283).into() }
    pub fn indent_guide() -> Hsla { rgb(0x313438).into() }

    // VCS
    pub fn vcs_added() -> Hsla { rgb(0x549159).into() }
    pub fn vcs_modified() -> Hsla { rgb(0x375fad).into() }
    pub fn vcs_deleted() -> Hsla { rgb(0x868a91).into() }
    /// File name colors in the project tree, tabs and the commit window.
    pub fn file_added() -> Hsla { rgb(0x73bd79).into() }
    pub fn file_modified() -> Hsla { rgb(0x6c9be8).into() }
    pub fn file_untracked() -> Hsla { rgb(0xd1675a).into() }
    pub fn file_deleted() -> Hsla { rgb(0x8c8f96).into() }
    pub fn file_conflict() -> Hsla { rgb(0xe55765).into() }
    pub fn diff_added_bg() -> Hsla { rgb(0x2b3d2e).into() }
    pub fn diff_deleted_bg() -> Hsla { rgb(0x3e3a3a).into() }
    pub fn diff_modified_bg() -> Hsla { rgb(0x283850).into() }
    pub fn diff_filler_bg() -> Hsla { rgb(0x232427).into() }
    /// Search match highlight.
    pub fn search_hit() -> Hsla { rgb(0x32593d).into() }
    pub fn search_current() -> Hsla { rgb(0x2e7d4a).into() }
    /// The bracket next to the caret and its partner.
    pub fn bracket_match() -> Hsla { rgb(0x3b514d).into() }
    /// Other occurrences of the identifier under the caret (and their scrollbar marks).
    pub fn occurrence() -> Hsla { rgb(0x344134).into() }
    pub fn occurrence_mark() -> Hsla { rgb(0xa7a77c).into() }
    pub fn error() -> Hsla { rgb(0xe55765).into() }
    /// Lane colors in the Git log graph.
    pub fn graph_lane(i: usize) -> Hsla {
        const C: [u32; 8] = [0x6c9be8, 0x5fb865, 0xe0a85a, 0xc77dbb, 0x2aacb8, 0xe55765, 0xb3ae60, 0x9876aa];
        rgb(C[i % C.len()]).into()
    }

    pub fn syntax(h: Highlight) -> Hsla {
        use Highlight::*;
        rgb(match h {
            Keyword => 0xcf8e6d,
            Function => 0x56a8f5,
            FunctionBuiltin => 0x8888c6,
            Method => 0x56a8f5,
            Type => 0xbcbec4,
            String => 0x6aab73,
            Escape => 0xcf8e6d,
            Comment => 0x7a7e85,
            Number => 0x2aacb8,
            Constant => 0xc77dbb,
            ConstantBuiltin => 0xcf8e6d,
            Property => 0xaa4926,
            Decorator => 0xb3ae60,
            Operator => 0xbcbec4,
            Punctuation => 0xbcbec4,
            SelfParam => 0x94558d,
        })
        .into()
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
