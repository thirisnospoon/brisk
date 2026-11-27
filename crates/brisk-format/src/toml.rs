//! TOML: `key = value` with one space around `=`, `[table]` headers without inner spaces,
//! keys and headers at the start of the line, the lines of a multi-line array indented, at
//! most two blank lines in a row, no trailing spaces. Strings (multi-line ones too) and
//! comments are kept as written.

#[derive(Clone, Copy, PartialEq)]
enum Multi {
    None,
    Basic,
    Literal,
}

/// The line's bracket depth change and the multi-line string state after it; `Err` for an
/// unclosed one-line string.
fn scan(line: &str, mut multi: Multi, depth: &mut i32) -> Result<(Multi, Option<usize>), &'static str> {
    let b = line.as_bytes();
    let mut i = 0;
    // The byte where a comment starts, if any.
    let mut comment = None;
    while i < b.len() {
        match multi {
            Multi::Basic | Multi::Literal => {
                let close = if multi == Multi::Basic { "\"\"\"" } else { "'''" };
                if multi == Multi::Basic && b[i] == b'\\' {
                    i += 2;
                    continue;
                }
                if line[i..].starts_with(close) {
                    i += 3;
                    // `""""` ends with a quote inside.
                    while b.get(i) == Some(&close.as_bytes()[0]) {
                        i += 1;
                    }
                    multi = Multi::None;
                    continue;
                }
                i += 1;
            }
            Multi::None => {
                let c = b[i];
                if line[i..].starts_with("\"\"\"") {
                    multi = Multi::Basic;
                    i += 3;
                } else if line[i..].starts_with("'''") {
                    multi = Multi::Literal;
                    i += 3;
                } else if c == b'"' || c == b'\'' {
                    let mut j = i + 1;
                    loop {
                        match b.get(j) {
                            None => return Err("unclosed string"),
                            Some(b'\\') if c == b'"' => j += 2,
                            Some(&q) if q == c => break,
                            _ => j += 1,
                        }
                    }
                    i = j + 1;
                } else if c == b'#' {
                    comment = Some(i);
                    break;
                } else {
                    match c {
                        b'[' | b'{' => *depth += 1,
                        b']' | b'}' => *depth -= 1,
                        _ => {}
                    }
                    i += 1;
                }
            }
        }
    }
    Ok((multi, comment))
}

/// `key=value` → `key = value`; the first `=` outside quotes.
fn key_value(line: &str) -> String {
    let b = line.as_bytes();
    let mut q: Option<u8> = None;
    for (i, &c) in b.iter().enumerate() {
        match (q, c) {
            (Some(x), c) if c == x => q = None,
            (Some(_), _) => {}
            (None, b'"' | b'\'') => q = Some(c),
            (None, b'#') => break,
            (None, b'=') => return format!("{} = {}", line[..i].trim_end(), line[i + 1..].trim_start()),
            _ => {}
        }
    }
    line.to_string()
}

/// `[ a . b ]` → `[a.b]`, `[[ x ]]` → `[[x]]`, outside quotes; a comment after it stays.
fn header(line: &str) -> String {
    let double = line.starts_with("[[");
    let open = if double { 2 } else { 1 };
    let close = if double { "]]" } else { "]" };
    let mut q: Option<char> = None;
    let mut end = None;
    for (i, c) in line.char_indices().skip(open) {
        match (q, c) {
            (Some(x), c) if c == x => q = None,
            (Some(_), _) => {}
            (None, '"' | '\'') => q = Some(c),
            (None, ']') => {
                end = Some(i);
                break;
            }
            _ => {}
        }
    }
    let Some(end) = end else { return line.to_string() };
    if !line[end..].starts_with(close) {
        return line.to_string();
    }
    let name: String = {
        // Spaces around dots and the brackets go; quoted parts stay.
        let mut out = String::new();
        let mut q: Option<char> = None;
        for c in line[open..end].trim().chars() {
            match (q, c) {
                (Some(x), c) => {
                    out.push(c);
                    if c == x {
                        q = None;
                    }
                }
                (None, '"' | '\'') => {
                    q = Some(c);
                    out.push(c);
                }
                (None, c) if c.is_whitespace() => {}
                _ => out.push(c),
            }
        }
        out
    };
    let rest = line[end + close.len()..].trim();
    let mut s = format!("{}{name}{close}", &line[..open]);
    if !rest.is_empty() {
        s.push(' ');
        s.push_str(rest);
    }
    s
}

pub fn format(text: &str, indent: &str) -> Result<String, String> {
    let mut out = String::with_capacity(text.len());
    let mut multi = Multi::None;
    let mut depth = 0i32;
    let mut blanks = 0;
    for (n, raw) in text.split('\n').enumerate() {
        let raw = raw.trim_end_matches('\r');
        // Inside a multi-line string the line is text.
        if multi != Multi::None {
            let (m, _) = scan(raw, multi, &mut depth).map_err(|e| format!("line {}: {e}", n + 1))?;
            multi = m;
            out.push_str(raw);
            out.push('\n');
            continue;
        }
        let line = raw.trim();
        if line.is_empty() {
            blanks += 1;
            if blanks <= 2 && !out.is_empty() {
                out.push('\n');
            }
            continue;
        }
        blanks = 0;
        let before = depth;
        let (m, _) = scan(line, multi, &mut depth).map_err(|e| format!("line {}: {e}", n + 1))?;
        if depth < 0 {
            return Err(format!("line {}: unexpected closing bracket", n + 1));
        }
        let text = if before > 0 {
            line.to_string()
        } else if line.starts_with('[') {
            header(line)
        } else if line.starts_with('#') {
            line.to_string()
        } else {
            key_value(line)
        };
        // A line that starts by closing a bracket goes one level out.
        let closes = usize::from(line.starts_with([']', '}']));
        for _ in 0..(before.max(0) as usize).saturating_sub(closes) {
            out.push_str(indent);
        }
        // A string starting a multi-line text keeps its trailing spaces.
        out.push_str(if m == Multi::None { text.trim_end() } else { &text });
        out.push('\n');
        multi = m;
    }
    if multi != Multi::None {
        return Err("a multi-line string is not closed".into());
    }
    if depth != 0 {
        return Err("a bracket is not closed".into());
    }
    let mut s = out.trim_end().to_string();
    s.push('\n');
    Ok(s)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn tidy() {
        let t = "# top\n[ package ]\nname=\"x\"   \n  version = '1=2' # v\n\n\n\n[[ bin ]]\nname  =  \"a\"\n[ tool . \"a b\" ]\nlist=[\n1,\n  [2, 3],\n]\ntext = \"\"\"\n  keep  = this\n\"\"\"\ninline = { a = 1, b = [1] }\n";
        let want = "# top\n[package]\nname = \"x\"\nversion = '1=2' # v\n\n\n[[bin]]\nname = \"a\"\n[tool.\"a b\"]\nlist = [\n    1,\n    [2, 3],\n]\ntext = \"\"\"\n  keep  = this\n\"\"\"\ninline = { a = 1, b = [1] }\n";
        let got = format(t, "    ").unwrap();
        assert_eq!(got, want);
        assert_eq!(format(&got, "    ").unwrap(), got);
        assert!(format("a = \"x", "  ").is_err());
        assert!(format("a = [1,", "  ").is_err());
    }
}
