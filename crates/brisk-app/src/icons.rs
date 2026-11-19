//! SVG icons built into the binary (Lucide, ISC, and a few of Brisk's own in the same style:
//! see `assets/icons/LICENSE-lucide.txt`). GPUI renders them as alpha masks, so one file
//! serves every color.

use std::borrow::Cow;

use gpui::{px, svg, AssetSource, Bounds, Hsla, InteractiveElement, IntoElement, ParentElement, Pixels, Point, SharedString, Styled, Svg, Window};

macro_rules! icons {
    ($($name:literal),* $(,)?) => {
        const ICONS: &[(&str, &[u8])] = &[$((concat!("icons/", $name, ".svg"), include_bytes!(concat!("../assets/icons/", $name, ".svg")))),*];
    };
}

icons![
    "arrow-down", "arrow-down-to-line", "arrow-left-right", "arrow-right-to-line", "arrow-up", "bookmark", "breakpoints", "bug", "calculator", "chart-pie", "check",
    "check-bold", "chevron-down", "chevron-right", "chevron-up", "circle", "circle-alert", "circle-check", "circle-slash", "circle-x", "clipboard",
    "cloud", "copy", "database", "diamond", "dot", "dot-hole", "eraser", "external-link", "file", "file-text", "folder", "git-branch", "git-commit", "island-bl", "island-br", "island-tl", "island-tr",
    "history", "lightbulb", "list", "list-todo", "list-tree", "minus", "power", "undo-2", "loader-circle", "menu", "mute-breakpoints", "package", "pause", "pencil", "pin",
    "play", "play-solid", "plus", "rerun-failed", "ring", "rotate-cw", "save", "search", "server", "settings-2", "sort-failed", "square-terminal",
    "star", "step-into", "step-out", "step-over", "stop-solid", "terminal", "timer", "trash", "triangle-alert", "wrap-text", "x", "zap",
    "align-justify", "columns-2", "fold-vertical", "unfold-vertical",
];

/// The embedded icons as GPUI assets ("icons/<name>.svg").
pub struct Assets;

impl AssetSource for Assets {
    fn load(&self, path: &str) -> gpui::Result<Option<Cow<'static, [u8]>>> {
        Ok(ICONS.iter().find(|(p, _)| *p == path).map(|(_, b)| Cow::Borrowed(*b)))
    }

    fn list(&self, path: &str) -> gpui::Result<Vec<SharedString>> {
        Ok(ICONS.iter().filter(|(p, _)| p.starts_with(path)).map(|(p, _)| SharedString::from(*p)).collect())
    }
}

fn path(name: &str) -> SharedString {
    format!("icons/{name}.svg").into()
}

/// An icon element: `size` logical px square, tinted with `color`.
pub fn icon(name: &str, size: f32, color: impl Into<Hsla>) -> Svg {
    svg().path(path(name)).size(px(size)).flex_none().text_color(color.into())
}

/// The icon that stands for a UI glyph still used as a short label (tool buttons, popup
/// rows, file kinds); `None` keeps the glyph as text (letters like "C", "m", "{").
pub fn glyph_icon(g: &str) -> Option<&'static str> {
    Some(match g {
        "▸" => "chevron-right",
        "▾" | "▼" => "chevron-down",
        "▴" => "chevron-up",
        "×" | "✕" => "x",
        "✓" => "check",
        "✗" => "circle-x",
        "⚠" => "triangle-alert",
        "▶" => "play-solid",
        "■" => "stop-solid",
        "↻" | "⟳" => "rotate-cw",
        "↑" => "arrow-up",
        "↓" => "arrow-down",
        "↧" => "arrow-down-to-line",
        "⌫" => "eraser",
        "⊘" => "circle-slash",
        "⏱" => "timer",
        "🕘" | "◷" => "history",
        "⇪" => "external-link",
        "✗↑" => "sort-failed",
        "★" => "star",
        "☁" => "cloud",
        "⎇" => "git-branch",
        "+" => "plus",
        "⚡" => "zap",
        "▤" => "list",
        "✎" => "pencil",
        "⎙" => "save",
        "✦" => "lightbulb",
        "▰" | "▣" => "folder",
        "▮" => "bookmark",
        "¶" => "clipboard",
        "●" => "dot",
        "◆" => "diamond",
        "⌕" => "search",
        "↶" => "undo-2",
        "⇆" => "arrow-left-right",
        "⏻" => "power",
        "⧉" => "copy",
        "−" => "minus",
        "⚙" => "settings-2",
        "≡" => "file-text",
        "•" => "file",
        _ => return None,
    })
}

/// A glyph label as its icon (`size` px, tinted `color`), or as text when it has none.
pub fn glyph(g: impl Into<SharedString>, size: f32, color: impl Into<Hsla>) -> gpui::AnyElement {
    let g: SharedString = g.into();
    let color = color.into();
    match glyph_icon(&g) {
        Some(name) => icon(name, size, color).into_any_element(),
        None => gpui::div().text_color(color).child(g).into_any_element(),
    }
}

/// A tree row's expand arrow in a fixed 12 px slot: chevron right / down, empty for leaves.
pub fn tree_arrow(expandable: bool, open: bool, color: impl Into<Hsla>) -> gpui::Div {
    let slot = gpui::div().w(px(12.)).h(px(12.)).flex_none().flex().items_center().justify_center();
    if expandable {
        slot.child(icon(if open { "chevron-down" } else { "chevron-right" }, 12., color))
    } else {
        slot
    }
}

/// A close cross that brightens while the element named `group` is hovered.
pub fn close_x(group: &'static str, size: f32) -> Svg {
    icon("x", size, crate::theme::Theme::text_dim()).group_hover(group, |s| s.text_color(crate::theme::Theme::text()))
}

/// A square of `size` logical px centered on `center`, snapped to the device pixel grid with
/// an even device size, so round shapes come out symmetric at any scale (1.0, 1.25, 1.5…).
pub fn snapped_square(center: Point<Pixels>, size: f32, scale: f32) -> Bounds<Pixels> {
    let d = ((size * scale / 2.).floor() * 2.).max(2.);
    let cx = (f32::from(center.x) * scale).round();
    let cy = (f32::from(center.y) * scale).round();
    Bounds::new(gpui::point(px((cx - d / 2.) / scale), px((cy - d / 2.) / scale)), gpui::size(px(d / scale), px(d / scale)))
}

/// `b` with its edges on device pixels (at least one device pixel each way): thin marks and
/// rounded rectangles then get the same crisp edges on every side.
pub fn snap_bounds(b: Bounds<Pixels>, scale: f32) -> Bounds<Pixels> {
    let r = |v: Pixels| (f32::from(v) * scale).round();
    let (x0, y0) = (r(b.left()), r(b.top()));
    let (x1, y1) = (r(b.right()).max(x0 + 1.), r(b.bottom()).max(y0 + 1.));
    Bounds::from_corners(gpui::point(px(x0 / scale), px(y0 / scale)), gpui::point(px(x1 / scale), px(y1 / scale)))
}

/// Paints an icon centered on `center` (inside an element's paint phase).
pub fn paint_icon(window: &mut Window, cx: &gpui::App, name: &str, center: Point<Pixels>, size: f32, color: Hsla) {
    let b = snapped_square(center, size, window.scale_factor());
    let _ = window.paint_svg(b, path(name), gpui::TransformationMatrix::unit(), color, cx);
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn all_icons_parse() {
        for (p, b) in ICONS {
            let s = std::str::from_utf8(b).unwrap();
            assert!(s.starts_with("<svg") && s.contains("viewBox=\"0 0 24 24\""), "{p}");
        }
        assert!(Assets.load("icons/play.svg").unwrap().is_some());
        assert!(Assets.load("icons/nope.svg").unwrap().is_none());
        // Every glyph mapping names an embedded icon.
        for g in ["▸", "▾", "▴", "×", "✓", "✗", "⚠", "▶", "■", "↻", "↑", "↓", "↧", "⌫", "⊘", "⏱", "🕘", "⇪", "✗↑", "★", "☁", "⎇", "+", "⚡", "▤", "✎", "⎙", "✦", "▰", "▮", "¶", "●", "◆", "⚙", "≡", "•", "⌕", "↶", "⇆", "⏻", "⧉", "−"] {
            let n = glyph_icon(g).unwrap();
            assert!(Assets.load(&format!("icons/{n}.svg")).unwrap().is_some(), "{g} -> {n}");
        }
    }

    #[test]
    fn snapping_is_even_and_on_the_grid() {
        for scale in [1.0f32, 1.25, 1.5, 1.75, 2.0] {
            let b = snapped_square(gpui::point(px(100.3), px(57.77)), 11., scale);
            let d = f32::from(b.size.width) * scale;
            let x = f32::from(b.origin.x) * scale;
            assert!((d - d.round()).abs() < 1e-3 && d.round() as i32 % 2 == 0, "{scale}: {d}");
            assert!((x - x.round()).abs() < 1e-3, "{scale}: {x}");
        }
    }
}
