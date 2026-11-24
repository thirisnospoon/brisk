//! SVG icons built into the binary: Lucide (ISC) plus Brisk's own tool window, file type and
//! panel icons. `assets/icons/LICENSE-lucide.txt` says which is which. GPUI renders them as alpha masks, so one file
//! serves every color.

use std::borrow::Cow;

use gpui::{px, svg, AssetSource, Bounds, Hsla, InteractiveElement, IntoElement, ParentElement, Pixels, Point, SharedString, Styled, Svg, Window};

macro_rules! icons {
    ($($name:literal),* $(,)?) => {
        const ICONS: &[(&str, &[u8])] = &[$((concat!("icons/", $name, ".svg"), include_bytes!(concat!("../assets/icons/", $name, ".svg")))),*];
    };
}

icons![
    "tw-bookmarks", "tw-breakpoints", "tw-clear", "tw-commit", "tw-copy", "tw-coverage", "tw-database", "tw-evaluate", "tw-git", "tw-mute-breakpoints", "tw-packages", "tw-pause", "tw-pin", "tw-problems", "tw-profiler", "tw-project", "tw-python-console", "tw-rerun", "tw-rerun-failed", "tw-resume", "tw-run", "tw-run-to-cursor", "tw-scroll-end", "tw-services", "tw-soft-wrap", "tw-step-into", "tw-step-out", "tw-step-over", "tw-stop", "tw-structure", "tw-terminal", "tw-todo", "tw-endpoints",
    "ft-archive", "ft-config", "ft-css", "ft-csv", "ft-docker", "ft-file", "ft-folder", "ft-html", "ft-ignore", "ft-image", "ft-js", "ft-json", "ft-letter-t", "ft-letter-y", "ft-lock", "ft-md", "ft-nb-a", "ft-nb-b", "ft-py-a", "ft-py-b", "ft-req-mark", "ft-rs", "ft-shell", "ft-sql", "ft-text", "ft-ts", "ft-xml", "arrow-down", "arrow-down-to-line", "arrow-left-right", "arrow-right-to-line", "arrow-up", "bookmark", "bug", "check",
    "check-bold", "chevron-down", "chevron-right", "chevron-up", "circle", "circle-alert", "circle-check", "circle-slash", "circle-x", "clipboard",
    "cloud", "copy", "database", "diamond", "dot", "eraser", "external-link", "file", "file-text", "folder", "git-branch", "island-bl", "island-br", "island-tl", "island-tr",
    "history", "lightbulb", "list", "list-todo", "minus", "power", "undo-2", "loader-circle", "menu", "pencil", "pin",
    "play", "play-solid", "plus", "rotate-cw", "save", "search", "settings-2", "sort-failed",
    "star", "stop-solid", "terminal", "timer", "triangle-alert", "x", "zap",
    "align-justify", "columns-2", "fold-vertical", "unfold-vertical",
    "chevrons-right", "chevrons-left", "arrow-left-to-line", "check-check", "wand-sparkles",
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

/// `icon` at the UI scale (chrome, panels, popups; the editor's own icons use `icon`).
pub fn ui_icon(name: &str, size: f32, color: impl Into<Hsla>) -> Svg {
    icon(name, size * crate::theme::ui_scale(), color)
}

/// A tool window stripe or Run / Debug toolbar icon ("tw-<name>": Brisk's own, 24x24, stroke
/// 1.6, the file-type icons' line), at the UI scale like the rest of the chrome.
pub fn tool_icon(name: &str, size: f32, color: impl Into<Hsla>) -> Svg {
    ui_icon(&format!("tw-{name}"), size, color)
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
    // A file type (`file_kind`): its own colors, 16 px where glyphs are 14, at the UI scale.
    if g.starts_with("ft:") {
        return file_icon_el(&g, size + 2.);
    }
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

/// The icon kind of a file or folder for `glyph` ("ft:py", "ft:dir-test", ...): one style for
/// the Project tree, tabs, Commit / Log, Find, Recent Files, Search Everywhere, breadcrumbs.
pub fn file_kind(name: &str, is_dir: bool) -> &'static str {
    let lower = name.to_ascii_lowercase();
    if is_dir {
        return match lower.as_str() {
            "src" | "lib" | "source" => "ft:dir-src",
            "test" | "tests" | "testing" => "ft:dir-test",
            ".venv" | "venv" | "env" | ".env" | ".tox" | ".nox" | "node_modules" | "__pycache__" | ".pytest_cache" | ".mypy_cache" | ".ruff_cache" | "build" | "dist" | "target" | ".idea" | ".git" => "ft:dir-ex",
            _ => "ft:dir",
        };
    }
    if lower == "dockerfile" || lower.ends_with(".dockerfile") || lower.starts_with("dockerfile.") {
        return "ft:docker";
    }
    if lower.starts_with("requirements") && lower.ends_with(".txt") {
        return "ft:req";
    }
    if lower == ".env" || lower.starts_with(".env.") {
        return "ft:env";
    }
    match lower.as_str() {
        ".gitignore" | ".gitattributes" | ".gitmodules" => return "ft:gitignore",
        ".dockerignore" => return "ft:dockerignore",
        _ => {}
    }
    match lower.rsplit('.').next().unwrap_or("") {
        "py" | "pyw" => "ft:py",
        "pyi" => "ft:pyi",
        "ipynb" => "ft:ipynb",
        "json" | "jsonc" | "json5" => "ft:json",
        "toml" => "ft:toml",
        "yaml" | "yml" => "ft:yaml",
        "ini" | "cfg" | "conf" | "properties" | "editorconfig" => "ft:cfg",
        "md" | "markdown" | "rst" => "ft:md",
        "txt" | "log" => "ft:txt",
        "rs" => "ft:rs",
        "js" | "jsx" | "mjs" | "cjs" => "ft:js",
        "ts" | "tsx" | "mts" | "cts" => "ft:ts",
        "html" | "htm" => "ft:html",
        "css" => "ft:css",
        "scss" | "sass" | "less" => "ft:scss",
        "sql" => "ft:sql",
        "sh" | "bash" | "zsh" | "bat" | "cmd" => "ft:sh",
        "ps1" | "psm1" | "psd1" => "ft:ps1",
        "xml" | "xsd" | "xsl" | "plist" => "ft:xml",
        "csv" | "tsv" => "ft:csv",
        "lock" => "ft:lock",
        "png" | "jpg" | "jpeg" | "gif" | "svg" | "ico" | "webp" | "bmp" => "ft:image",
        "zip" | "tar" | "gz" | "tgz" | "bz2" | "xz" | "7z" | "rar" | "whl" | "egg" => "ft:archive",
        _ => "ft:file",
    }
}

/// A file kind's layers: SVG masks and their colors (two for two-color marks).
fn file_layers(kind: &str) -> &'static [(&'static str, u32)] {
    const GRAY: u32 = 0x9aa0a6;
    match kind {
        "ft:py" => &[("ft-py-a", 0x3d7ab8), ("ft-py-b", 0xf2c744)],
        "ft:pyi" => &[("ft-py-a", 0x7f95b3), ("ft-py-b", 0xb8a96e)],
        "ft:ipynb" => &[("ft-nb-a", 0xf0812b), ("ft-nb-b", GRAY)],
        "ft:json" => &[("ft-json", 0xe0b84a)],
        "ft:toml" => &[("ft-file", GRAY), ("ft-letter-t", 0xd08a4f)],
        "ft:yaml" => &[("ft-file", GRAY), ("ft-letter-y", 0xd9534f)],
        "ft:cfg" => &[("ft-file", GRAY), ("ft-config", 0x6f9fd8)],
        "ft:env" => &[("ft-file", GRAY), ("ft-config", 0xe0b84a)],
        "ft:md" => &[("ft-md", 0x6a9ad8)],
        "ft:txt" => &[("ft-text", GRAY)],
        "ft:req" => &[("ft-file", GRAY), ("ft-req-mark", 0x3d7ab8)],
        "ft:rs" => &[("ft-rs", 0xdea584)],
        "ft:js" => &[("ft-js", 0xe8c547)],
        "ft:ts" => &[("ft-ts", 0x3d8fe0)],
        "ft:html" => &[("ft-html", 0xe8663d)],
        "ft:css" => &[("ft-css", 0x8a6fd6)],
        "ft:scss" => &[("ft-css", 0xcf649a)],
        "ft:sql" => &[("ft-sql", 0x5ba6d6)],
        "ft:sh" => &[("ft-shell", GRAY)],
        "ft:ps1" => &[("ft-shell", 0x4f8fd6)],
        "ft:xml" => &[("ft-xml", 0xf0a050)],
        "ft:csv" => &[("ft-csv", 0x5fb865)],
        "ft:lock" => &[("ft-lock", GRAY)],
        "ft:gitignore" => &[("ft-file", GRAY), ("ft-ignore", 0xf05133)],
        "ft:dockerignore" => &[("ft-file", GRAY), ("ft-ignore", 0x2496ed)],
        "ft:docker" => &[("ft-docker", 0x2496ed)],
        "ft:image" => &[("ft-image", 0x4fb3a6)],
        "ft:archive" => &[("ft-archive", 0xc9a052)],
        "ft:dir" => &[("ft-folder", GRAY)],
        "ft:dir-src" => &[("ft-folder", 0x4b8bbe)],
        "ft:dir-test" => &[("ft-folder", 0x5fb865)],
        "ft:dir-ex" => &[("ft-folder", 0xc48a3c)],
        _ => &[("ft-file", GRAY)],
    }
}

/// A file kind's icon, `size` UI px: its layers stacked in one square.
pub fn file_icon_el(kind: &str, size: f32) -> gpui::AnyElement {
    let s = size * crate::theme::ui_scale();
    let mut d = gpui::div().relative().size(px(s)).flex_none();
    for (i, &(name, c)) in file_layers(kind).iter().enumerate() {
        let layer = icon(name, s, gpui::rgb(c));
        d = if i == 0 { d.child(layer) } else { d.child(gpui::div().absolute().top_0().left_0().child(layer)) };
    }
    d.into_any_element()
}

/// A round mark centered on `center`: filled, or a ring `ring` px wide. An analytic quad
/// (GPUI's rounded-rect SDF) on an even square of whole device pixels, so the edge is
/// antialiased alike on every side at any scale; an SVG sprite is resampled and can come
/// out lopsided at 125% / 150%.
pub fn paint_dot(window: &mut Window, center: Point<Pixels>, size: f32, color: Hsla, ring: Option<f32>) {
    let scale = window.scale_factor();
    let b = snapped_square(center, size, scale);
    let r = b.size.width / 2.;
    match ring {
        None => window.paint_quad(gpui::fill(b, color).corner_radii(r)),
        Some(w) => {
            // The ring's width in whole device pixels, leaving a hole of at least two.
            let d = f32::from(b.size.width) * scale;
            let w = (w * scale).round().clamp(1., ((d - 2.) / 2.).max(1.));
            window.paint_quad(gpui::quad(b, r, gpui::transparent_black(), px(w / scale), color, gpui::BorderStyle::Solid));
        }
    }
}

/// A round mark as an element (the dirty dot on a tab, status dots): `size` UI px in a
/// slot two px wider, painted with `paint_dot`.
pub fn dot(size: f32, color: impl Into<Hsla>) -> gpui::AnyElement {
    let color = color.into();
    let s = size * crate::theme::ui_scale();
    gpui::canvas(|_, _, _| {}, move |b, _, window, _| paint_dot(window, b.center(), s, color, None))
        .size(px(s + 2.))
        .flex_none()
        .into_any_element()
}

/// Paints an icon centered on `center` (inside an element's paint phase).
pub fn paint_icon(window: &mut Window, cx: &gpui::App, name: &str, center: Point<Pixels>, size: f32, color: Hsla) {
    let b = snapped_square(center, size, window.scale_factor());
    let _ = window.paint_svg(b, path(name), gpui::TransformationMatrix::unit(), color, cx);
}

#[cfg(test)]
mod usage_tests;

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

    /// What GPUI's quad shader does for `paint_dot`: per device pixel, coverage of the
    /// circle inscribed in the snapped square (distance from the pixel center to the edge,
    /// clamped to 0..1). The mark must come out mirror-symmetric left/right and top/bottom
    /// at every scale, wherever its center falls.
    #[test]
    fn round_marks_are_symmetric_at_fractional_scales() {
        for scale in [1.0f32, 1.25, 1.5, 1.75, 2.0] {
            for (cx, cy) in [(100.3f32, 57.77f32), (40.5, 12.125), (7.0, 9.9)] {
                for size in [6.0f32, 8., 11.] {
                    let b = snapped_square(gpui::point(px(cx), px(cy)), size, scale);
                    let (x0, y0) = (f32::from(b.origin.x) * scale, f32::from(b.origin.y) * scale);
                    let d = f32::from(b.size.width) * scale;
                    let n = d.round() as i32;
                    assert!((x0 - x0.round()).abs() < 1e-3 && (y0 - y0.round()).abs() < 1e-3, "{scale} {size}: off-grid");
                    let (ccx, ccy, r) = (x0 + d / 2., y0 + d / 2., d / 2.);
                    let cov = |i: i32, j: i32| {
                        let (px_, py_) = (x0 + i as f32 + 0.5, y0 + j as f32 + 0.5);
                        let dist = ((px_ - ccx).powi(2) + (py_ - ccy).powi(2)).sqrt() - r;
                        (0.5 - dist).clamp(0., 1.)
                    };
                    for i in 0..n {
                        for j in 0..n {
                            let c = cov(i, j);
                            assert!((c - cov(n - 1 - i, j)).abs() < 1e-4, "{scale} {size}: left/right");
                            assert!((c - cov(i, n - 1 - j)).abs() < 1e-4, "{scale} {size}: top/bottom");
                        }
                    }
                }
            }
        }
    }

    #[test]
    fn file_kinds_have_icons() {
        for (name, dir, kind) in [
            ("main.py", false, "ft:py"), ("stubs.pyi", false, "ft:pyi"), ("a.ipynb", false, "ft:ipynb"), ("package.json", false, "ft:json"),
            ("pyproject.toml", false, "ft:toml"), ("ci.yml", false, "ft:yaml"), ("setup.cfg", false, "ft:cfg"), (".env", false, "ft:env"),
            ("README.md", false, "ft:md"), ("notes.txt", false, "ft:txt"), ("requirements-dev.txt", false, "ft:req"), ("lib.rs", false, "ft:rs"),
            ("app.tsx", false, "ft:ts"), ("index.html", false, "ft:html"), ("db.sql", false, "ft:sql"), ("run.ps1", false, "ft:ps1"),
            ("Dockerfile", false, "ft:docker"), (".gitignore", false, "ft:gitignore"), ("poetry.lock", false, "ft:lock"), ("logo.PNG", false, "ft:image"),
            ("x.whl", false, "ft:archive"), ("LICENSE", false, "ft:file"), ("tests", true, "ft:dir-test"), (".venv", true, "ft:dir-ex"), ("pkg", true, "ft:dir"),
        ] {
            assert_eq!(file_kind(name, dir), kind, "{name}");
            for (layer, _) in file_layers(kind) {
                assert!(Assets.load(&format!("icons/{layer}.svg")).unwrap().is_some(), "{kind}: {layer}");
            }
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
