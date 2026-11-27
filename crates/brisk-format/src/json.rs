//! JSON and JSONC: one member or element per line, `key: value`, empty `{}` / `[]` kept
//! short. Comments stay where they were (a trailing one on its line), strings and key order
//! are untouched, so a minified API response becomes readable and a config keeps its notes.

use crate::Out;

#[derive(Debug, PartialEq)]
enum Tok<'a> {
    Open(char),
    Close(char),
    Comma,
    Colon,
    /// A string, number, `true` / `false` / `null` or other bare word, as written.
    Atom(&'a str),
    /// A comment and whether a line break came before it in the source.
    Comment(&'a str, bool),
}

fn tokens(text: &str) -> Result<Vec<Tok<'_>>, String> {
    let b = text.as_bytes();
    let mut out = Vec::new();
    let mut i = 0;
    let mut newline = false;
    let line_of = |i: usize| text[..i].matches('\n').count() + 1;
    while i < b.len() {
        let c = b[i];
        match c {
            b'\n' => {
                newline = true;
                i += 1;
                continue;
            }
            b' ' | b'\t' | b'\r' => {
                i += 1;
                continue;
            }
            b'{' | b'[' => out.push(Tok::Open(c as char)),
            b'}' | b']' => out.push(Tok::Close(c as char)),
            b',' => out.push(Tok::Comma),
            b':' => out.push(Tok::Colon),
            b'/' if b.get(i + 1) == Some(&b'/') => {
                let end = text[i..].find('\n').map_or(b.len(), |e| i + e);
                out.push(Tok::Comment(text[i..end].trim_end(), newline));
                i = end;
                newline = false;
                continue;
            }
            b'/' if b.get(i + 1) == Some(&b'*') => {
                let end = text[i + 2..].find("*/").map(|e| i + 2 + e + 2).ok_or_else(|| format!("line {}: unclosed comment", line_of(i)))?;
                out.push(Tok::Comment(&text[i..end], newline));
                i = end;
                newline = false;
                continue;
            }
            b'"' | b'\'' => {
                let mut j = i + 1;
                loop {
                    match b.get(j) {
                        None | Some(b'\n') => return Err(format!("line {}: unclosed string", line_of(i))),
                        Some(b'\\') => j += 2,
                        Some(&q) if q == c => break,
                        _ => j += 1,
                    }
                }
                out.push(Tok::Atom(&text[i..=j]));
                i = j + 1;
                newline = false;
                continue;
            }
            _ => {
                let end = i + text[i..].find(|ch: char| ch.is_whitespace() || "{}[],:\"'".contains(ch) || text[i..].starts_with("//")).unwrap_or(b.len() - i);
                let end = end.max(i + 1);
                out.push(Tok::Atom(&text[i..end]));
                i = end;
                newline = false;
                continue;
            }
        }
        newline = false;
        i += 1;
    }
    Ok(out)
}

pub fn format(text: &str, indent: &str) -> Result<String, String> {
    let toks = tokens(text)?;
    let mut stack: Vec<char> = Vec::new();
    for t in &toks {
        match t {
            Tok::Open(c) => stack.push(*c),
            Tok::Close(c) => {
                let open = stack.pop().ok_or_else(|| format!("unexpected '{c}'"))?;
                if (open == '{') != (*c == '}') {
                    return Err(format!("'{open}' closed by '{c}'"));
                }
            }
            _ => {}
        }
    }
    if let Some(c) = stack.last() {
        return Err(format!("'{c}' is not closed"));
    }
    let mut o = Out::new(indent);
    // After a line comment nothing may follow on its line.
    let mut must_break = false;
    let significant = |k: usize| toks[k..].iter().find(|t| !matches!(t, Tok::Comment(..)));
    for (k, t) in toks.iter().enumerate() {
        if must_break && !matches!(t, Tok::Comma) {
            o.newline();
            must_break = false;
        }
        match t {
            Tok::Open(c) => {
                o.push_str(&c.to_string());
                let empty = matches!(toks.get(k + 1), Some(Tok::Close(_)));
                o.depth += 1;
                if !empty {
                    o.newline();
                }
            }
            Tok::Close(c) => {
                o.depth = o.depth.saturating_sub(1);
                if !matches!(toks.get(k.wrapping_sub(1)), Some(Tok::Open(_))) || k == 0 {
                    o.newline();
                }
                o.push_str(&c.to_string());
            }
            Tok::Comma => {
                if must_break {
                    // A comma after a line comment belongs before it: rare, keep it valid.
                    o.newline();
                    must_break = false;
                }
                o.push_str(",");
                // A trailing comma before the closer (JSONC) stays; the closer breaks the line.
                if !matches!(significant(k + 1), Some(Tok::Close(_))) {
                    // A comment on the same source line follows the comma.
                    if !matches!(toks.get(k + 1), Some(Tok::Comment(_, false))) {
                        o.newline();
                    }
                }
            }
            Tok::Colon => o.push_str(": "),
            Tok::Atom(a) => o.push_str(a),
            Tok::Comment(c, before) => {
                if !*before && !o.at_line_start() {
                    o.push_str(" ");
                } else if !o.at_line_start() {
                    o.newline();
                }
                o.push_str(c);
                if c.starts_with("//") || *before || o.at_line_start() {
                    must_break = true;
                }
                // A comment after `{` or `,` that started a line keeps the next item on its own.
                if matches!(toks.get(k + 1), Some(Tok::Close(_))) {
                    must_break = true;
                }
            }
        }
    }
    Ok(o.finish())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn minified_becomes_readable() {
        let t = r#"{"a":1,"b":[1,2,{"c":null}],"d":{},"e":[],"s":"x, {y}: z"}"#;
        let want = "{\n  \"a\": 1,\n  \"b\": [\n    1,\n    2,\n    {\n      \"c\": null\n    }\n  ],\n  \"d\": {},\n  \"e\": [],\n  \"s\": \"x, {y}: z\"\n}\n";
        assert_eq!(format(t, "  ").unwrap(), want);
        // Formatting twice changes nothing.
        assert_eq!(format(want, "  ").unwrap(), want);
    }

    #[test]
    fn comments_stay() {
        let t = "{\n// head\n\"a\": 1, // one\n/* block */\n\"b\": 2,\n}";
        let got = format(t, "    ").unwrap();
        assert_eq!(got, "{\n    // head\n    \"a\": 1, // one\n    /* block */\n    \"b\": 2,\n}\n");
        assert_eq!(format(&got, "    ").unwrap(), got);
    }

    #[test]
    fn errors() {
        assert!(format("{\"a\": [1, 2}", "  ").unwrap_err().contains("closed by"));
        assert!(format("{\"a\": \"x}", "  ").unwrap_err().contains("line 1"));
        assert!(format("[1, 2", "  ").is_err());
    }
}
