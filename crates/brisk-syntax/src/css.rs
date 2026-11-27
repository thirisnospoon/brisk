//! CSS without a language server: colours in a line (for the gutter swatches) and a compact
//! table of properties and their keyword values for completion.

/// Named colours of CSS (the full list), as 0xRRGGBB.
const NAMED: &[(&str, u32)] = &[
    ("aliceblue", 0xf0f8ff),
    ("antiquewhite", 0xfaebd7),
    ("aqua", 0x00ffff),
    ("aquamarine", 0x7fffd4),
    ("azure", 0xf0ffff),
    ("beige", 0xf5f5dc),
    ("bisque", 0xffe4c4),
    ("black", 0x000000),
    ("blanchedalmond", 0xffebcd),
    ("blue", 0x0000ff),
    ("blueviolet", 0x8a2be2),
    ("brown", 0xa52a2a),
    ("burlywood", 0xdeb887),
    ("cadetblue", 0x5f9ea0),
    ("chartreuse", 0x7fff00),
    ("chocolate", 0xd2691e),
    ("coral", 0xff7f50),
    ("cornflowerblue", 0x6495ed),
    ("cornsilk", 0xfff8dc),
    ("crimson", 0xdc143c),
    ("cyan", 0x00ffff),
    ("darkblue", 0x00008b),
    ("darkcyan", 0x008b8b),
    ("darkgoldenrod", 0xb8860b),
    ("darkgray", 0xa9a9a9),
    ("darkgreen", 0x006400),
    ("darkgrey", 0xa9a9a9),
    ("darkkhaki", 0xbdb76b),
    ("darkmagenta", 0x8b008b),
    ("darkolivegreen", 0x556b2f),
    ("darkorange", 0xff8c00),
    ("darkorchid", 0x9932cc),
    ("darkred", 0x8b0000),
    ("darksalmon", 0xe9967a),
    ("darkseagreen", 0x8fbc8f),
    ("darkslateblue", 0x483d8b),
    ("darkslategray", 0x2f4f4f),
    ("darkslategrey", 0x2f4f4f),
    ("darkturquoise", 0x00ced1),
    ("darkviolet", 0x9400d3),
    ("deeppink", 0xff1493),
    ("deepskyblue", 0x00bfff),
    ("dimgray", 0x696969),
    ("dimgrey", 0x696969),
    ("dodgerblue", 0x1e90ff),
    ("firebrick", 0xb22222),
    ("floralwhite", 0xfffaf0),
    ("forestgreen", 0x228b22),
    ("fuchsia", 0xff00ff),
    ("gainsboro", 0xdcdcdc),
    ("ghostwhite", 0xf8f8ff),
    ("gold", 0xffd700),
    ("goldenrod", 0xdaa520),
    ("gray", 0x808080),
    ("green", 0x008000),
    ("greenyellow", 0xadff2f),
    ("grey", 0x808080),
    ("honeydew", 0xf0fff0),
    ("hotpink", 0xff69b4),
    ("indianred", 0xcd5c5c),
    ("indigo", 0x4b0082),
    ("ivory", 0xfffff0),
    ("khaki", 0xf0e68c),
    ("lavender", 0xe6e6fa),
    ("lavenderblush", 0xfff0f5),
    ("lawngreen", 0x7cfc00),
    ("lemonchiffon", 0xfffacd),
    ("lightblue", 0xadd8e6),
    ("lightcoral", 0xf08080),
    ("lightcyan", 0xe0ffff),
    ("lightgoldenrodyellow", 0xfafad2),
    ("lightgray", 0xd3d3d3),
    ("lightgreen", 0x90ee90),
    ("lightgrey", 0xd3d3d3),
    ("lightpink", 0xffb6c1),
    ("lightsalmon", 0xffa07a),
    ("lightseagreen", 0x20b2aa),
    ("lightskyblue", 0x87cefa),
    ("lightslategray", 0x778899),
    ("lightslategrey", 0x778899),
    ("lightsteelblue", 0xb0c4de),
    ("lightyellow", 0xffffe0),
    ("lime", 0x00ff00),
    ("limegreen", 0x32cd32),
    ("linen", 0xfaf0e6),
    ("magenta", 0xff00ff),
    ("maroon", 0x800000),
    ("mediumaquamarine", 0x66cdaa),
    ("mediumblue", 0x0000cd),
    ("mediumorchid", 0xba55d3),
    ("mediumpurple", 0x9370db),
    ("mediumseagreen", 0x3cb371),
    ("mediumslateblue", 0x7b68ee),
    ("mediumspringgreen", 0x00fa9a),
    ("mediumturquoise", 0x48d1cc),
    ("mediumvioletred", 0xc71585),
    ("midnightblue", 0x191970),
    ("mintcream", 0xf5fffa),
    ("mistyrose", 0xffe4e1),
    ("moccasin", 0xffe4b5),
    ("navajowhite", 0xffdead),
    ("navy", 0x000080),
    ("oldlace", 0xfdf5e6),
    ("olive", 0x808000),
    ("olivedrab", 0x6b8e23),
    ("orange", 0xffa500),
    ("orangered", 0xff4500),
    ("orchid", 0xda70d6),
    ("palegoldenrod", 0xeee8aa),
    ("palegreen", 0x98fb98),
    ("paleturquoise", 0xafeeee),
    ("palevioletred", 0xdb7093),
    ("papayawhip", 0xffefd5),
    ("peachpuff", 0xffdab9),
    ("peru", 0xcd853f),
    ("pink", 0xffc0cb),
    ("plum", 0xdda0dd),
    ("powderblue", 0xb0e0e6),
    ("purple", 0x800080),
    ("rebeccapurple", 0x663399),
    ("red", 0xff0000),
    ("rosybrown", 0xbc8f8f),
    ("royalblue", 0x4169e1),
    ("saddlebrown", 0x8b4513),
    ("salmon", 0xfa8072),
    ("sandybrown", 0xf4a460),
    ("seagreen", 0x2e8b57),
    ("seashell", 0xfff5ee),
    ("sienna", 0xa0522d),
    ("silver", 0xc0c0c0),
    ("skyblue", 0x87ceeb),
    ("slateblue", 0x6a5acd),
    ("slategray", 0x708090),
    ("slategrey", 0x708090),
    ("snow", 0xfffafa),
    ("springgreen", 0x00ff7f),
    ("steelblue", 0x4682b4),
    ("tan", 0xd2b48c),
    ("teal", 0x008080),
    ("thistle", 0xd8bfd8),
    ("tomato", 0xff6347),
    ("turquoise", 0x40e0d0),
    ("violet", 0xee82ee),
    ("wheat", 0xf5deb3),
    ("white", 0xffffff),
    ("whitesmoke", 0xf5f5f5),
    ("yellow", 0xffff00),
    ("yellowgreen", 0x9acd32),
];

/// An sRGB colour with alpha, 0..=1 each.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Color {
    pub r: f32,
    pub g: f32,
    pub b: f32,
    pub a: f32,
}

impl Color {
    fn rgb(v: u32, a: f32) -> Self {
        Color { r: ((v >> 16) & 0xff) as f32 / 255., g: ((v >> 8) & 0xff) as f32 / 255., b: (v & 0xff) as f32 / 255., a }
    }
}

fn hex(s: &str) -> Option<Color> {
    let d: Vec<u32> = s.chars().map(|c| c.to_digit(16)).collect::<Option<_>>()?;
    let two = |i: usize| (d[i] * 16 + d[i + 1]) as f32 / 255.;
    let one = |i: usize| (d[i] * 17) as f32 / 255.;
    Some(match d.len() {
        3 => Color { r: one(0), g: one(1), b: one(2), a: 1. },
        4 => Color { r: one(0), g: one(1), b: one(2), a: one(3) },
        6 => Color { r: two(0), g: two(2), b: two(4), a: 1. },
        8 => Color { r: two(0), g: two(2), b: two(4), a: two(6) },
        _ => return None,
    })
}

/// A number or percentage of a colour function; `scale` is what 100% means.
fn component(s: &str, scale: f32) -> Option<f32> {
    match s.strip_suffix('%') {
        Some(p) => p.parse::<f32>().ok().map(|v| v / 100. * scale),
        None => s.trim_end_matches("deg").parse().ok(),
    }
}

fn hsl_to_rgb(h: f32, s: f32, l: f32) -> (f32, f32, f32) {
    let h = h.rem_euclid(360.) / 360.;
    let q = if l < 0.5 { l * (1. + s) } else { l + s - l * s };
    let p = 2. * l - q;
    let f = |t: f32| {
        let t = t.rem_euclid(1.);
        if t < 1. / 6. {
            p + (q - p) * 6. * t
        } else if t < 0.5 {
            q
        } else if t < 2. / 3. {
            p + (q - p) * (2. / 3. - t) * 6.
        } else {
            p
        }
    };
    (f(h + 1. / 3.), f(h), f(h - 1. / 3.))
}

fn function(name: &str, args: &str) -> Option<Color> {
    let parts: Vec<&str> = args.split(|c: char| c == ',' || c == '/' || c.is_whitespace()).filter(|p| !p.is_empty()).collect();
    if parts.len() < 3 {
        return None;
    }
    let a = parts.get(3).map_or(Some(1.), |p| component(p, 1.))?.clamp(0., 1.);
    match name {
        "rgb" | "rgba" => {
            let c = |i: usize| component(parts[i], 255.).map(|v| (v / 255.).clamp(0., 1.));
            Some(Color { r: c(0)?, g: c(1)?, b: c(2)?, a })
        }
        "hsl" | "hsla" => {
            let h = component(parts[0], 360.)?;
            let s = component(parts[1], 1.)?.clamp(0., 1.);
            let l = component(parts[2], 1.)?.clamp(0., 1.);
            let (r, g, b) = hsl_to_rgb(h, s, l);
            Some(Color { r, g, b, a })
        }
        _ => None,
    }
}

/// The first colour written in `line`: `#hex`, `rgb()` / `rgba()`, `hsl()` / `hsla()` or a
/// named colour (as a whole word, outside comments).
pub fn first_color(line: &str) -> Option<Color> {
    let code = line.split("/*").next().unwrap_or(line);
    let code = if code.trim_start().starts_with("//") { "" } else { code };
    let b = code.as_bytes();
    let word = |c: u8| c.is_ascii_alphanumeric() || c == b'-' || c == b'_';
    let mut i = 0;
    while i < b.len() {
        if b[i] == b'#' && (i == 0 || !word(b[i - 1])) {
            let end = i + 1 + code[i + 1..].find(|c: char| !c.is_ascii_alphanumeric()).unwrap_or(code.len() - i - 1);
            if let Some(c) = hex(&code[i + 1..end]) {
                return Some(c);
            }
            i = end;
            continue;
        }
        if b[i].is_ascii_alphabetic() && (i == 0 || !word(b[i - 1]) && b[i - 1] != b'#' && b[i - 1] != b'.' && b[i - 1] != b'$' && b[i - 1] != b'@') {
            let end = i + code[i..].find(|c: char| !(c.is_ascii_alphanumeric() || c == '-' || c == '_')).unwrap_or(code.len() - i);
            let name = code[i..end].to_ascii_lowercase();
            if code[end..].starts_with('(') {
                if let Some(close) = code[end..].find(')') {
                    if let Some(c) = function(&name, &code[end + 1..end + close]) {
                        return Some(c);
                    }
                }
            } else if !code[end..].trim_start().starts_with(':') {
                // A value, not a property name (`color:`) or selector part.
                if let Ok(k) = NAMED.binary_search_by(|(n, _)| (*n).cmp(name.as_str())) {
                    return Some(Color::rgb(NAMED[k].1, 1.));
                }
                if name == "transparent" {
                    return Some(Color { r: 0., g: 0., b: 0., a: 0. });
                }
            }
            i = end;
            continue;
        }
        i += 1;
    }
    None
}

// --- Completion ------------------------------------------------------------------------

const COLORS: &[&str] = &["currentcolor", "transparent"];
const BORDER_STYLES: &[&str] = &["none", "solid", "dashed", "dotted", "double", "groove", "ridge", "inset", "outset", "hidden"];
const AUTO: &[&str] = &["auto"];
const SIZES: &[&str] = &["auto", "fit-content", "max-content", "min-content"];
const ALIGN: &[&str] = &["normal", "stretch", "center", "start", "end", "flex-start", "flex-end", "baseline", "space-between", "space-around", "space-evenly"];

/// Properties and their keyword values (colours get the named colours too).
const PROPERTIES: &[(&str, &[&str])] = &[
    ("accent-color", COLORS),
    ("align-content", ALIGN),
    ("align-items", ALIGN),
    ("align-self", ALIGN),
    ("animation", &["none", "infinite", "linear", "ease", "ease-in", "ease-out", "ease-in-out", "alternate", "forwards", "backwards", "both"]),
    ("animation-delay", &[]),
    ("animation-direction", &["normal", "reverse", "alternate", "alternate-reverse"]),
    ("animation-duration", &[]),
    ("animation-fill-mode", &["none", "forwards", "backwards", "both"]),
    ("animation-iteration-count", &["infinite"]),
    ("animation-name", &["none"]),
    ("animation-timing-function", &["linear", "ease", "ease-in", "ease-out", "ease-in-out", "step-start", "step-end"]),
    ("appearance", &["none", "auto"]),
    ("aspect-ratio", AUTO),
    ("backdrop-filter", &["none"]),
    ("background", &["none", "transparent", "no-repeat", "repeat", "center", "cover", "contain"]),
    ("background-attachment", &["scroll", "fixed", "local"]),
    ("background-clip", &["border-box", "padding-box", "content-box", "text"]),
    ("background-color", COLORS),
    ("background-image", &["none", "url()", "linear-gradient()", "radial-gradient()"]),
    ("background-position", &["center", "top", "bottom", "left", "right"]),
    ("background-repeat", &["repeat", "no-repeat", "repeat-x", "repeat-y", "space", "round"]),
    ("background-size", &["auto", "cover", "contain"]),
    ("border", BORDER_STYLES),
    ("border-bottom", BORDER_STYLES),
    ("border-collapse", &["collapse", "separate"]),
    ("border-color", COLORS),
    ("border-left", BORDER_STYLES),
    ("border-radius", &[]),
    ("border-right", BORDER_STYLES),
    ("border-spacing", &[]),
    ("border-style", BORDER_STYLES),
    ("border-top", BORDER_STYLES),
    ("border-width", &["thin", "medium", "thick"]),
    ("bottom", AUTO),
    ("box-shadow", &["none", "inset"]),
    ("box-sizing", &["content-box", "border-box"]),
    ("clear", &["none", "left", "right", "both"]),
    ("clip-path", &["none"]),
    ("color", COLORS),
    ("column-gap", &["normal"]),
    ("content", &["none", "normal", "\"\"", "attr()", "counter()"]),
    (
        "cursor",
        &[
            "auto",
            "default",
            "pointer",
            "text",
            "move",
            "wait",
            "help",
            "not-allowed",
            "grab",
            "grabbing",
            "crosshair",
            "progress",
            "none",
            "col-resize",
            "row-resize",
        ],
    ),
    ("direction", &["ltr", "rtl"]),
    (
        "display",
        &["block", "inline", "inline-block", "flex", "inline-flex", "grid", "inline-grid", "none", "contents", "table", "table-row", "table-cell", "list-item"],
    ),
    ("fill", COLORS),
    ("filter", &["none", "blur()", "brightness()", "grayscale()", "drop-shadow()"]),
    ("flex", &["none", "auto", "1"]),
    ("flex-basis", SIZES),
    ("flex-direction", &["row", "row-reverse", "column", "column-reverse"]),
    ("flex-flow", &["row", "column", "wrap", "nowrap"]),
    ("flex-grow", &[]),
    ("flex-shrink", &[]),
    ("flex-wrap", &["nowrap", "wrap", "wrap-reverse"]),
    ("float", &["none", "left", "right"]),
    ("font", &["inherit"]),
    ("font-family", &["serif", "sans-serif", "monospace", "system-ui", "cursive"]),
    ("font-size", &["small", "medium", "large", "x-large", "smaller", "larger"]),
    ("font-style", &["normal", "italic", "oblique"]),
    ("font-weight", &["normal", "bold", "lighter", "bolder", "100", "200", "300", "400", "500", "600", "700", "800", "900"]),
    ("gap", &["normal"]),
    ("grid", &["none"]),
    ("grid-area", AUTO),
    ("grid-auto-flow", &["row", "column", "dense"]),
    ("grid-column", AUTO),
    ("grid-row", AUTO),
    ("grid-template-areas", &["none"]),
    ("grid-template-columns", &["none", "repeat()", "minmax()", "auto-fill", "auto-fit"]),
    ("grid-template-rows", &["none", "repeat()", "minmax()"]),
    ("height", SIZES),
    ("inset", AUTO),
    ("justify-content", ALIGN),
    ("justify-items", ALIGN),
    ("justify-self", ALIGN),
    ("left", AUTO),
    ("letter-spacing", &["normal"]),
    ("line-height", &["normal"]),
    ("list-style", &["none", "disc", "circle", "square", "decimal", "inside", "outside"]),
    ("list-style-type", &["none", "disc", "circle", "square", "decimal"]),
    ("margin", AUTO),
    ("margin-bottom", AUTO),
    ("margin-left", AUTO),
    ("margin-right", AUTO),
    ("margin-top", AUTO),
    ("max-height", &["none", "fit-content", "max-content", "min-content"]),
    ("max-width", &["none", "fit-content", "max-content", "min-content"]),
    ("min-height", SIZES),
    ("min-width", SIZES),
    ("mix-blend-mode", &["normal", "multiply", "screen", "overlay", "darken", "lighten"]),
    ("object-fit", &["fill", "contain", "cover", "none", "scale-down"]),
    ("object-position", &["center", "top", "bottom", "left", "right"]),
    ("opacity", &[]),
    ("order", &[]),
    ("outline", &["none", "solid", "dashed", "dotted"]),
    ("outline-color", COLORS),
    ("outline-offset", &[]),
    ("overflow", &["visible", "hidden", "scroll", "auto", "clip"]),
    ("overflow-wrap", &["normal", "break-word", "anywhere"]),
    ("overflow-x", &["visible", "hidden", "scroll", "auto", "clip"]),
    ("overflow-y", &["visible", "hidden", "scroll", "auto", "clip"]),
    ("padding", &[]),
    ("padding-bottom", &[]),
    ("padding-left", &[]),
    ("padding-right", &[]),
    ("padding-top", &[]),
    ("place-content", ALIGN),
    ("place-items", ALIGN),
    ("pointer-events", &["auto", "none"]),
    ("position", &["static", "relative", "absolute", "fixed", "sticky"]),
    ("resize", &["none", "both", "horizontal", "vertical"]),
    ("right", AUTO),
    ("row-gap", &["normal"]),
    ("scroll-behavior", &["auto", "smooth"]),
    ("stroke", COLORS),
    ("table-layout", &["auto", "fixed"]),
    ("text-align", &["left", "right", "center", "justify", "start", "end"]),
    ("text-decoration", &["none", "underline", "overline", "line-through"]),
    ("text-overflow", &["clip", "ellipsis"]),
    ("text-shadow", &["none"]),
    ("text-transform", &["none", "uppercase", "lowercase", "capitalize"]),
    ("top", AUTO),
    ("transform", &["none", "translate()", "translateX()", "translateY()", "scale()", "rotate()", "skew()"]),
    ("transform-origin", &["center", "top", "bottom", "left", "right"]),
    ("transition", &["none", "all", "ease", "ease-in", "ease-out", "ease-in-out", "linear"]),
    ("user-select", &["auto", "none", "text", "all"]),
    ("vertical-align", &["baseline", "top", "middle", "bottom", "text-top", "text-bottom", "sub", "super"]),
    ("visibility", &["visible", "hidden", "collapse"]),
    ("white-space", &["normal", "nowrap", "pre", "pre-wrap", "pre-line", "break-spaces"]),
    ("width", SIZES),
    ("will-change", &["auto", "transform", "opacity"]),
    ("word-break", &["normal", "break-all", "keep-all", "break-word"]),
    ("z-index", AUTO),
];

const GLOBAL: &[&str] = &["inherit", "initial", "unset", "revert"];

/// The property names for completion.
pub fn properties() -> impl Iterator<Item = &'static str> {
    PROPERTIES.iter().map(|(n, _)| *n)
}

/// The keyword values of `property` (vendor prefixes ignored), with the named colours for
/// colour properties and the global keywords.
pub fn values(property: &str) -> Vec<&'static str> {
    let p = property.trim().to_ascii_lowercase();
    let p = p.trim_start_matches("-webkit-").trim_start_matches("-moz-").trim_start_matches("-ms-");
    let Some((_, v)) = PROPERTIES.iter().find(|(n, _)| *n == p) else { return Vec::new() };
    let mut out: Vec<&'static str> = v.to_vec();
    if std::ptr::eq(*v, COLORS) || p.ends_with("-color") {
        out.extend(NAMED.iter().map(|(n, _)| *n));
    }
    out.extend(GLOBAL);
    out
}

/// Where the caret is in a stylesheet, for completion.
#[derive(Debug, PartialEq, Eq)]
pub enum Spot {
    /// Typing a property name from byte `start`.
    Property { start: usize },
    /// Typing a value of `property` from byte `start`.
    Value { property: String, start: usize },
}

/// The completion spot at the end of `before` (the text up to the caret), or None in a
/// selector, comment, string or at-rule prelude.
pub fn spot(before: &str) -> Option<Spot> {
    let b = before.as_bytes();
    let mut depth = 0i32;
    let mut seg = 0usize;
    let mut i = 0;
    while i < b.len() {
        match b[i] {
            // An unclosed comment or string at the caret: no completion.
            b'/' if b.get(i + 1) == Some(&b'*') => i += before[i + 2..].find("*/")? + 4,
            q @ (b'"' | b'\'') => i += before[i + 1..].find(q as char)? + 2,
            c => {
                match c {
                    b'{' => depth += 1,
                    b'}' => depth -= 1,
                    _ => {}
                }
                if matches!(c, b'{' | b'}' | b';') {
                    seg = i + 1;
                }
                i += 1;
            }
        }
    }
    if depth <= 0 {
        return None;
    }
    let s = &before[seg..];
    let word_start = before.len() - s.len() + s.rfind(|c: char| !(c.is_ascii_alphanumeric() || c == '-')).map_or(0, |k| k + 1);
    match s.find(':') {
        Some(c) => {
            let property = s[..c].trim();
            (!property.is_empty() && property.chars().all(|ch| ch.is_ascii_alphanumeric() || ch == '-'))
                .then(|| Spot::Value { property: property.to_string(), start: word_start })
        }
        None => {
            let t = s.trim_start();
            (t.chars().all(|ch| ch.is_ascii_alphanumeric() || ch == '-') && !t.starts_with(|ch: char| ch.is_ascii_digit()))
                .then_some(Spot::Property { start: word_start })
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn colours_in_lines() {
        let c = first_color("  color: #ff0000;").unwrap();
        assert_eq!((c.r, c.g, c.b, c.a), (1., 0., 0., 1.));
        let c = first_color("background: rgba(0, 0, 255, 50%)").unwrap();
        assert_eq!((c.b, c.a), (1., 0.5));
        let c = first_color("border: 1px solid hsl(120 100% 50%)").unwrap();
        assert!((c.g - 1.).abs() < 1e-4 && c.r.abs() < 1e-4);
        assert_eq!(first_color("  color: rebeccapurple;"), Some(Color::rgb(0x663399, 1.)));
        assert_eq!(first_color("#main { display: block }"), None);
        assert_eq!(first_color(".red { }"), None);
        assert_eq!(first_color("/* #fff */"), None);
        assert_eq!(first_color("  color: var(--x);"), None);
        assert!(NAMED.windows(2).all(|w| w[0].0 < w[1].0), "sorted for binary search");
    }

    #[test]
    fn completion_spots() {
        assert_eq!(spot("a { col"), Some(Spot::Property { start: 4 }));
        assert_eq!(spot("a { color: re"), Some(Spot::Value { property: "color".into(), start: 11 }));
        assert_eq!(spot("a { color: red; dis"), Some(Spot::Property { start: 16 }));
        assert_eq!(spot("a.b"), None);
        assert_eq!(spot("a { /* x"), None);
        assert_eq!(spot("a { content: \"x"), None);
        assert!(values("color").contains(&"red"));
        assert!(values("-webkit-display").contains(&"flex"));
        assert!(values("no-such").is_empty());
        assert!(values("display").contains(&"flex"));
    }
}
