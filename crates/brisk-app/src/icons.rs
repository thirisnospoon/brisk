//! SVG icons built into the binary (Lucide, ISC, and a few of Brisk's own in the same style:
//! see `assets/icons/LICENSE-lucide.txt`). GPUI renders them as alpha masks, so one file
//! serves every color.

use std::borrow::Cow;

use gpui::{px, svg, AssetSource, Bounds, Hsla, Pixels, Point, SharedString, Styled, Svg, Window};

macro_rules! icons {
    ($($name:literal),* $(,)?) => {
        const ICONS: &[(&str, &[u8])] = &[$((concat!("icons/", $name, ".svg"), include_bytes!(concat!("../assets/icons/", $name, ".svg")))),*];
    };
}

icons![
    "arrow-down-to-line", "arrow-right-to-line", "bookmark", "breakpoints", "bug", "calculator", "chart-pie", "chevron-down", "chevron-right", "check", "check-bold", "circle",
    "circle-alert", "circle-check", "circle-slash", "circle-x", "copy", "database", "dot", "dot-hole", "folder", "git-branch", "git-commit", "list-todo",
    "list-tree", "loader-circle", "menu", "mute-breakpoints", "package", "pause", "pin", "play", "play-solid", "rerun-failed", "ring", "rotate-cw",
    "search", "server", "square-terminal", "step-into", "step-out", "step-over", "stop-solid", "terminal", "timer", "trash", "triangle-alert",
    "wrap-text", "x",
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
