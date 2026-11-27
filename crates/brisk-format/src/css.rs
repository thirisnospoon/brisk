//! CSS, SCSS and Less: `selector {` with one `property: value;` per line inside, nested
//! blocks (`@media`, SCSS nesting) indented, a missing last `;` added. Comments, strings,
//! `url()` and SCSS `#{…}` interpolation are kept as written; one blank line between rules
//! is kept where the source had one.

use crate::Out;

enum Tok<'a> {
    Open,
    Close,
    Semi,
    /// Selector or declaration text (may hold strings and parentheses).
    Text(&'a str),
    /// A comment and whether a line break came before it.
    Comment(&'a str, bool),
    /// A blank line in the source.
    Blank,
}

fn flush<'a>(text: &'a str, out: &mut Vec<Tok<'a>>, from: usize, to: usize) {
    if !text[from..to].trim().is_empty() {
        out.push(Tok::Text(&text[from..to]));
    }
}

fn tokens(text: &str) -> Result<Vec<Tok<'_>>, String> {
    let b = text.as_bytes();
    let mut out = Vec::new();
    let line_of = |i: usize| text[..i].matches('\n').count() + 1;
    let mut i = 0;
    let mut start = 0;
    let mut newlines = 0;
    let mut paren = 0i32;
    while i < b.len() {
        let c = b[i];
        match c {
            b'\n' => {
                newlines += 1;
                if newlines == 2 && text[start..i].trim().is_empty() {
                    out.push(Tok::Blank);
                }
            }
            b' ' | b'\t' | b'\r' => {}
            _ => {
                if c == b'/' && b.get(i + 1) == Some(&b'*') {
                    flush(text, &mut out, start, i);
                    let end = text[i + 2..].find("*/").map(|e| i + 2 + e + 2).ok_or_else(|| format!("line {}: unclosed comment", line_of(i)))?;
                    out.push(Tok::Comment(&text[i..end], newlines > 0 || out.is_empty()));
                    i = end;
                    start = i;
                    newlines = 0;
                    continue;
                }
                // `//` comments (SCSS, Less), not in `url(http://…)`.
                if c == b'/' && b.get(i + 1) == Some(&b'/') && paren == 0 && (i == 0 || b[i - 1] != b':') {
                    flush(text, &mut out, start, i);
                    let end = text[i..].find('\n').map_or(b.len(), |e| i + e);
                    out.push(Tok::Comment(text[i..end].trim_end(), newlines > 0 || out.is_empty()));
                    i = end;
                    start = i;
                    newlines = 0;
                    continue;
                }
                newlines = 0;
                match c {
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
                        i = j;
                    }
                    b'(' => paren += 1,
                    b')' => paren -= 1,
                    // SCSS interpolation: part of the text.
                    b'#' if b.get(i + 1) == Some(&b'{') => {
                        let end = text[i..].find('}').ok_or_else(|| format!("line {}: unclosed #{{", line_of(i)))?;
                        i += end;
                    }
                    b'{' | b'}' | b';' if paren <= 0 => {
                        flush(text, &mut out, start, i);
                        out.push(match c {
                            b'{' => Tok::Open,
                            b'}' => Tok::Close,
                            _ => Tok::Semi,
                        });
                        start = i + 1;
                    }
                    _ => {}
                }
            }
        }
        i += 1;
    }
    flush(text, &mut out, start, b.len());
    Ok(out)
}

/// Collapses runs of whitespace outside strings; `a,b` → `a, b` outside parentheses.
fn tidy(t: &str) -> String {
    let mut out = String::new();
    let mut quote: Option<char> = None;
    let mut space = false;
    let mut paren = 0;
    for c in t.trim().chars() {
        if let Some(q) = quote {
            out.push(c);
            if c == q {
                quote = None;
            }
            continue;
        }
        match c {
            '"' | '\'' => {
                if space {
                    out.push(' ');
                    space = false;
                }
                quote = Some(c);
                out.push(c);
            }
            c if c.is_whitespace() => space = true,
            ',' => {
                out.push(',');
                space = paren == 0;
            }
            _ => {
                if c == '(' {
                    paren += 1;
                } else if c == ')' {
                    paren -= 1;
                }
                if space && !out.is_empty() && !(paren > 0 && out.ends_with('(')) {
                    out.push(' ');
                }
                space = false;
                out.push(c);
            }
        }
    }
    out
}

/// `color:red` → `color: red`; at-rules and `&:hover`-like text stay.
fn declaration(t: &str) -> String {
    let t = tidy(t);
    if t.starts_with('@') || t.starts_with('&') {
        return t;
    }
    match t.split_once(':') {
        Some((p, v)) if !p.trim().is_empty() && !p.contains(' ') || p.starts_with("--") || p.starts_with('$') => format!("{}: {}", p.trim(), v.trim()),
        _ => t,
    }
}

pub fn format(text: &str, indent: &str) -> Result<String, String> {
    let toks = tokens(text)?;
    let mut depth = 0i32;
    for t in &toks {
        match t {
            Tok::Open => depth += 1,
            Tok::Close => {
                depth -= 1;
                if depth < 0 {
                    return Err("unexpected '}'".into());
                }
            }
            _ => {}
        }
    }
    if depth != 0 {
        return Err("a '{' is not closed".into());
    }
    let mut o = Out::new(indent);
    let mut must_break = false;
    // A blank line in the source before the next item (kept, one at most).
    let mut blank = false;
    let mut k = 0;
    while k < toks.len() {
        let line_start = |o: &mut Out, must: &mut bool, blank: &mut bool| {
            if !o.at_line_start() || *must {
                o.newline();
            }
            *must = false;
            if std::mem::take(blank) && !o.s.trim_end().ends_with('{') && !o.s.trim().is_empty() {
                let at = o.s.rfind('\n').map_or(0, |i| i + 1);
                o.s.insert(at, '\n');
            }
        };
        match &toks[k] {
            Tok::Blank => blank = true,
            Tok::Text(t) => {
                line_start(&mut o, &mut must_break, &mut blank);
                match toks.get(k + 1) {
                    Some(Tok::Open) => {
                        o.push_str(&tidy(t));
                        o.push_str(" {");
                        o.depth += 1;
                        must_break = true;
                        k += 1;
                    }
                    Some(Tok::Semi) => {
                        o.push_str(&declaration(t));
                        o.push_str(";");
                        k += 1;
                    }
                    // The last declaration of a block: PyCharm adds the `;`.
                    _ => {
                        o.push_str(&declaration(t));
                        o.push_str(";");
                    }
                }
            }
            Tok::Open => {
                line_start(&mut o, &mut must_break, &mut blank);
                o.push_str("{");
                o.depth += 1;
                must_break = true;
            }
            Tok::Close => {
                blank = false;
                o.depth = o.depth.saturating_sub(1);
                o.newline();
                o.push_str("}");
                must_break = true;
            }
            Tok::Semi => {}
            Tok::Comment(c, before) => {
                if *before || o.s.trim_end().ends_with('{') || must_break && o.s.trim_end().ends_with('}') {
                    line_start(&mut o, &mut must_break, &mut blank);
                } else {
                    o.push_str(" ");
                }
                o.push_str(c);
                must_break = true;
            }
        }
        k += 1;
    }
    Ok(o.finish())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn minified_css() {
        let t = "a,b{color:red;margin:0 auto}@media (max-width:600px){.x{display:none}}";
        let want = "a, b {\n  color: red;\n  margin: 0 auto;\n}\n@media (max-width:600px) {\n  .x {\n    display: none;\n  }\n}\n";
        assert_eq!(format(t, "  ").unwrap(), want);
        assert_eq!(format(want, "  ").unwrap(), want);
    }

    #[test]
    fn scss_and_comments() {
        let t = "/* top */\n.card{\n  // note\n  &:hover{color:#{$c}}\n  background:url(\"a;b.png\");  /* bg */\n\n\n  $x : 1px;\n}\n\n.y{}";
        let got = format(t, "    ").unwrap();
        let want = "/* top */\n.card {\n    // note\n    &:hover {\n        color: #{$c};\n    }\n    background: url(\"a;b.png\"); /* bg */\n\n    $x: 1px;\n}\n\n.y {\n}\n";
        assert_eq!(got, want);
        assert_eq!(format(&got, "    ").unwrap(), got);
        assert!(format(".a { color: red;", "  ").is_err());
    }
}
