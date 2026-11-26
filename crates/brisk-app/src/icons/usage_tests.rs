//! Every icon name the code passes to an icon helper has an asset (a missing one renders
//! blank, silently), and every asset is still named somewhere, so dead SVGs do not pile up in
//! the binary. The sources are scanned as text: calls of the helpers below, the literals in
//! their icon argument (`if a { "x" } else { "y" }` gives both).

use super::ICONS;
use std::collections::BTreeMap;
use std::path::{Path, PathBuf};

/// (file suffix, or "" for every file; callee; index of the icon argument; asset prefix).
/// `tool_icon` and the toolbars built on it draw "tw-<name>".
const HELPERS: &[(&str, &str, usize, &str)] = &[
    ("", "icon", 0, ""),
    ("", "ui_icon", 0, ""),
    ("", "tool_icon", 0, "tw-"),
    ("", "paint_icon", 2, ""),
    ("", "empty_state", 0, ""),
    ("", "icon_button", 1, ""),
    ("", "title_button", 1, ""),
    ("", "stripe_button", 1, "tw-"),
    // diff_view::tool, also imported by merge_view and vcs_popup.
    ("diff_view.rs", "tool", 1, ""),
    ("merge_view.rs", "tool", 1, ""),
    ("vcs_popup.rs", "tool", 1, ""),
    ("merge_view.rs", "mini", 1, ""),
    ("window.rs", "tool", 1, "tw-"),
    ("terminal_view.rs", "button", 1, "tw-"),
    ("terminal_view.rs", "toggle", 1, "tw-"),
];

fn sources() -> Vec<(PathBuf, String)> {
    let mut out = Vec::new();
    let mut stack = vec![Path::new(env!("CARGO_MANIFEST_DIR")).join("src")];
    while let Some(d) = stack.pop() {
        for e in std::fs::read_dir(&d).unwrap().flatten() {
            let p = e.path();
            if p.is_dir() {
                stack.push(p);
            } else if p.extension().is_some_and(|x| x == "rs") {
                let text = std::fs::read_to_string(&p).unwrap();
                out.push((p, text));
            }
        }
    }
    out
}

/// The top-level arguments of the call whose `(` is at `open`.
fn call_args(s: &str, open: usize) -> Vec<&str> {
    let b = s.as_bytes();
    let (mut depth, mut i, mut start) = (0i32, open + 1, open + 1);
    let mut args = Vec::new();
    while i < b.len() {
        match b[i] {
            b'"' => {
                i += 1;
                while i < b.len() && b[i] != b'"' {
                    i += if b[i] == b'\\' { 2 } else { 1 };
                }
            }
            // A char literal like '(' or ',': skip it whole (lifetimes have no closing quote).
            b'\'' if i + 2 < b.len() && b[i + 2] == b'\'' => i += 2,
            b'(' | b'[' | b'{' => depth += 1,
            b')' | b']' | b'}' if depth > 0 => depth -= 1,
            b')' => {
                args.push(&s[start..i]);
                return args;
            }
            b',' if depth == 0 => {
                args.push(&s[start..i]);
                start = i + 1;
            }
            _ => {}
        }
        i += 1;
    }
    args
}

/// String literals that look like icon names.
fn name_literals(arg: &str) -> Vec<&str> {
    arg.split('"')
        .skip(1)
        .step_by(2)
        .filter(|l| {
            !l.is_empty() && l.starts_with(|c: char| c.is_ascii_lowercase()) && l.chars().all(|c| c.is_ascii_lowercase() || c.is_ascii_digit() || c == '-')
        })
        .collect()
}

/// Icon names used through the helpers: asset name -> where.
fn used() -> BTreeMap<String, Vec<String>> {
    let mut used: BTreeMap<String, Vec<String>> = BTreeMap::new();
    for (path, text) in sources() {
        let file = path.file_name().unwrap().to_string_lossy().into_owned();
        for &(only, callee, arg, prefix) in HELPERS {
            if !only.is_empty() && file != only {
                continue;
            }
            let pat = format!("{callee}(");
            for (at, _) in text.match_indices(&pat) {
                let before = text[..at].chars().next_back();
                if before.is_some_and(|c| c.is_alphanumeric() || c == '_') || text[..at].ends_with("fn ") {
                    continue;
                }
                let args = call_args(&text, at + callee.len());
                let Some(a) = args.get(arg) else { continue };
                let line = text[..at].lines().count();
                for n in name_literals(a) {
                    used.entry(format!("{prefix}{n}")).or_default().push(format!("{file}:{line}"));
                }
            }
        }
    }
    used
}

fn registered() -> Vec<&'static str> {
    ICONS.iter().map(|(p, _)| p.trim_start_matches("icons/").trim_end_matches(".svg")).collect()
}

#[test]
fn every_icon_used_in_code_has_an_asset() {
    let reg = registered();
    let missing: Vec<String> = used().into_iter().filter(|(n, _)| !reg.contains(&n.as_str())).map(|(n, at)| format!("{n} ({})", at.join(", "))).collect();
    assert!(missing.is_empty(), "icons without an asset in assets/icons (and icons![]): {missing:?}");
}

#[test]
fn call_args_split_at_top_level() {
    let s = r#"tool("a,b", if x { "p" } else { f(1, ')') }, g[0], "c")"#;
    assert_eq!(call_args(s, 4), vec![r#""a,b""#, r#" if x { "p" } else { f(1, ')') }"#, " g[0]", r#" "c""#]);
    assert_eq!(name_literals(r#"if on { "check-bold" } else { "Not An Icon" }"#), vec!["check-bold"]);
}

#[test]
fn the_scan_sees_the_helpers() {
    // Guards the scanner itself: names known to go through each kind of helper.
    let u = used();
    for n in ["play", "search", "tw-step-into", "tw-mute-breakpoints", "rotate-cw", "list-todo", "play-solid", "arrow-right-to-line"] {
        assert!(u.contains_key(n), "{n} not found by the scan");
    }
}

#[test]
fn every_asset_is_named_somewhere() {
    // An asset counts as used when the helper scan finds it, or when its name is a string
    // literal anywhere else in the sources (file-kind and glyph tables, status tuples,
    // `tw-` names given without the prefix). Delete the SVG and its icons![] entry otherwise.
    let u = used();
    let all: String = sources().into_iter().map(|(_, t)| t).collect();
    let icons_rs = std::fs::read_to_string(Path::new(env!("CARGO_MANIFEST_DIR")).join("src/icons.rs")).unwrap();
    let registry = icons_rs.split("icons![").nth(1).and_then(|r| r.split("];").next()).unwrap_or("");
    let rest = all.replace(registry, "");
    let count = |n: &str| rest.matches(&format!("\"{n}\"")).count();
    let attributed = |n: &str| u.get(n).map_or(0, Vec::len);
    let dead: Vec<&str> = registered()
        .into_iter()
        .filter(|n| !u.contains_key(*n))
        .filter(|n| match n.strip_prefix("tw-") {
            // A `tw-` asset named without its prefix outside the helpers.
            Some(b) => count(b) == 0,
            // A plain literal that the scan already gave to a `tw-` helper does not count.
            None => count(n) <= attributed(&format!("tw-{n}")),
        })
        .collect();
    assert!(dead.is_empty(), "assets no code names any more (remove the SVG and its icons![] entry): {dead:?}");
}
