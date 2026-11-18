//! The small part of Markdown that ty puts into hover and completion documentation:
//! paragraphs with hard line breaks, HTML entities, backslash escapes, inline code,
//! emphasis and links. Fenced code and rules are split off earlier by `doc_blocks`.

use std::ops::Range;

/// How a run of documentation text is drawn.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum InlineStyle {
    Code,
    Bold,
    Italic,
    Link(String),
}

/// A documentation text block flattened to plain text plus styled byte ranges
/// (sorted, non-overlapping). Line breaks are `\n`, paragraphs are separated by a blank line.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct DocText {
    pub text: String,
    pub styles: Vec<(Range<usize>, InlineStyle)>,
}

/// `&gt;` → `>`, `\_` → `_`: what ty escapes in docstrings so they are not read as Markdown.
pub fn unescape_md(s: &str) -> String {
    let mut out = String::with_capacity(s.len());
    let mut it = s.char_indices().peekable();
    while let Some((i, c)) = it.next() {
        match c {
            '\\' => match it.peek() {
                Some(&(_, n)) if n.is_ascii_punctuation() => {
                    out.push(n);
                    it.next();
                }
                _ => out.push(c),
            },
            '&' => match entity(&s[i..]) {
                Some((ch, len)) => {
                    out.push(ch);
                    for _ in 1..len {
                        it.next();
                    }
                }
                None => out.push(c),
            },
            _ => out.push(c),
        }
    }
    out
}

/// An HTML entity at the start of `s`: the character and the entity's length in bytes
/// (all entities here are ASCII, so bytes = chars).
fn entity(s: &str) -> Option<(char, usize)> {
    let end = s.as_bytes().iter().take(10).position(|&b| b == b';')?;
    let name = &s[1..end];
    let ch = match name {
        "gt" => '>',
        "lt" => '<',
        "amp" => '&',
        "quot" => '"',
        "apos" => '\'',
        "nbsp" => '\u{a0}',
        _ => {
            let n = name.strip_prefix('#')?;
            let code = match n.strip_prefix(['x', 'X']) {
                Some(h) => u32::from_str_radix(h, 16).ok()?,
                None => n.parse().ok()?,
            };
            char::from_u32(code)?
        }
    };
    Some((ch, end + 1))
}

/// Lines → text: a line ending in two spaces (ty ends every docstring line so) or a
/// backslash breaks there, other lines join with a space; headings turn bold, list markers
/// become bullets, blank lines start a new paragraph. `reflow` joins two-space breaks too,
/// for narrow panels where docstrings' own 79-column lines would wrap raggedly.
fn join_lines(md: &str, reflow: bool) -> String {
    let mut out = String::new();
    let mut hard = true;
    let mut blank = false;
    for raw in md.lines() {
        let line = raw.trim_end();
        if line.is_empty() {
            blank = !out.is_empty();
            hard = true;
            continue;
        }
        let trimmed = line.trim_start();
        let indent = &line[..line.len() - trimmed.len()];
        let (mut body, mut brk) = match line.strip_suffix('\\') {
            Some(b) => (b.trim_start(), true),
            // Signature lines (`str(object='') -> str`) and lead-ins ending in ':' keep
            // their break even when reflowing.
            None => (trimmed, raw.ends_with("  ") && (!reflow || decode_entities(trimmed).contains(" -> ") || trimmed.ends_with(':'))),
        };
        let heading = body.strip_prefix('#').map(|h| h.trim_start_matches('#')).filter(|h| h.starts_with(' '));
        let item = body.strip_prefix("- ").or_else(|| body.strip_prefix("* ")).or_else(|| body.strip_prefix("+ "));
        if blank {
            out.push_str("\n\n");
            blank = false;
        } else if !out.is_empty() {
            // List items and headings always start their own line.
            out.push(if hard || heading.is_some() || item.is_some() { '\n' } else { ' ' });
        }
        if let Some(h) = heading {
            out.push_str("**");
            out.push_str(h.trim());
            out.push_str("**");
            hard = true;
            continue;
        }
        if let Some(item) = item {
            out.extend(indent.chars().map(|_| '\u{a0}'));
            out.push_str("• ");
            body = item;
            brk = true;
        } else if hard {
            // Indentation matters in docstrings (examples, parameter lists); keep it as
            // non-breaking spaces so the text layout does not drop it.
            out.extend(indent.chars().map(|_| '\u{a0}'));
        }
        out.push_str(body);
        hard = brk || indent.len() >= 4;
    }
    out
}

/// Parse a documentation text block (no fences) into plain text and styles.
pub fn parse_doc_text(md: &str, reflow: bool) -> DocText {
    let src = join_lines(md, reflow);
    let mut d = DocText::default();
    inline(&src, &mut d, None);
    d
}

fn push_styled(d: &mut DocText, text: &str, style: Option<&InlineStyle>) {
    if text.is_empty() {
        return;
    }
    let start = d.text.len();
    d.text.push_str(text);
    if let Some(s) = style {
        // Merge with the previous run of the same style so ranges stay few.
        if let Some((r, last)) = d.styles.last_mut() {
            if r.end == start && last == s {
                r.end = d.text.len();
                return;
            }
        }
        d.styles.push((start..d.text.len(), s.clone()));
    }
}

/// Inline Markdown. `style` is the enclosing emphasis or link: nested markup inside it is
/// read as text with that one style, which keeps the ranges non-overlapping.
fn inline(s: &str, d: &mut DocText, style: Option<&InlineStyle>) {
    let b = s.as_bytes();
    let mut plain = String::new();
    let mut i = 0;
    let flush = |plain: &mut String, d: &mut DocText| {
        push_styled(d, plain, style);
        plain.clear();
    };
    while i < s.len() {
        let c = b[i];
        // Backslash escape.
        if c == b'\\' && i + 1 < s.len() && b[i + 1].is_ascii_punctuation() {
            plain.push(b[i + 1] as char);
            i += 2;
            continue;
        }
        if c == b'&' {
            if let Some((ch, len)) = entity(&s[i..]) {
                plain.push(ch);
                i += len;
                continue;
            }
        }
        // `code` and ``code``: the closing run has the same length; contents are literal.
        if c == b'`' {
            let n = b[i..].iter().take_while(|&&x| x == b'`').count();
            let fence = &s[i..i + n];
            if let Some(off) = find_run(&s[i + n..], fence) {
                let code = &s[i + n..i + n + off];
                let code = if code.len() > 2 && code.starts_with(' ') && code.ends_with(' ') { &code[1..code.len() - 1] } else { code };
                flush(&mut plain, d);
                // Code inside a link stays part of the link; elsewhere it is code.
                let st = match style {
                    Some(l @ InlineStyle::Link(_)) => l.clone(),
                    _ => InlineStyle::Code,
                };
                push_styled(d, &decode_entities(code), Some(&st));
                i += n + off + n;
                continue;
            }
            plain.push_str(fence);
            i += n;
            continue;
        }
        if style.is_none() {
            // [text](url)
            if c == b'[' {
                if let Some((text, url, len)) = link(&s[i..]) {
                    flush(&mut plain, d);
                    inline(text, d, Some(&InlineStyle::Link(url.to_string())));
                    i += len;
                    continue;
                }
            }
            // <https://…>
            if c == b'<' {
                if let Some(end) = s[i..].find('>') {
                    let url = &s[i + 1..i + end];
                    if is_url(url) && !url.contains(char::is_whitespace) {
                        flush(&mut plain, d);
                        push_styled(d, url, Some(&InlineStyle::Link(url.to_string())));
                        i += end + 1;
                        continue;
                    }
                }
            }
            // Bare https://… up to whitespace, minus trailing punctuation.
            if (c == b'h' || c == b'w') && is_url(&s[i..]) && (i == 0 || !b[i - 1].is_ascii_alphanumeric()) {
                let len = s[i..].find(|ch: char| ch.is_whitespace() || ch == '<' || ch == '>' || ch == '"').unwrap_or(s.len() - i);
                let url = s[i..i + len].trim_end_matches(['.', ',', ';', ':', ')', '\'', '!', '?']);
                flush(&mut plain, d);
                push_styled(d, url, Some(&InlineStyle::Link(url.to_string())));
                i += url.len();
                continue;
            }
            // **bold** / __bold__, *italic* / _italic_.
            if c == b'*' || c == b'_' {
                let n = if b.get(i + 1) == Some(&c) { 2 } else { 1 };
                let marker = &s[i..i + n];
                let opens = b.get(i + n).is_some_and(|x| !x.is_ascii_whitespace() && *x != c);
                // `snake_case` words are not emphasis.
                let word_inside = c == b'_' && i > 0 && b[i - 1].is_ascii_alphanumeric();
                if opens && !word_inside {
                    if let Some(off) = find_closing(&s[i + n..], marker) {
                        flush(&mut plain, d);
                        let st = if n == 2 { InlineStyle::Bold } else { InlineStyle::Italic };
                        inline(&s[i + n..i + n + off], d, Some(&st));
                        i += n + off + n;
                        continue;
                    }
                }
            }
        }
        let ch = s[i..].chars().next().unwrap();
        plain.push(ch);
        i += ch.len_utf8();
    }
    flush(&mut plain, d);
}

fn is_url(s: &str) -> bool {
    s.starts_with("https://") || s.starts_with("http://") || s.starts_with("www.")
}

fn decode_entities(s: &str) -> String {
    let mut out = String::with_capacity(s.len());
    let mut i = 0;
    while i < s.len() {
        if s.as_bytes()[i] == b'&' {
            if let Some((ch, len)) = entity(&s[i..]) {
                out.push(ch);
                i += len;
                continue;
            }
        }
        let ch = s[i..].chars().next().unwrap();
        out.push(ch);
        i += ch.len_utf8();
    }
    out
}

/// Offset of a backtick run exactly as long as `fence`.
fn find_run(s: &str, fence: &str) -> Option<usize> {
    let b = s.as_bytes();
    let mut i = 0;
    while i < b.len() {
        if b[i] == b'`' {
            let n = b[i..].iter().take_while(|&&x| x == b'`').count();
            if n == fence.len() {
                return Some(i);
            }
            i += n;
        } else {
            i += 1;
        }
    }
    None
}

/// The closing emphasis marker: not after whitespace, not part of a longer run, within the line.
fn find_closing(s: &str, marker: &str) -> Option<usize> {
    let b = s.as_bytes();
    let m = marker.as_bytes()[0];
    let mut i = 0;
    while i < b.len() {
        match b[i] {
            b'\n' => return None,
            b'\\' => i += 1,
            x if x == m => {
                let n = b[i..].iter().take_while(|&&x| x == m).count();
                let after_word = m == b'_' && b.get(i + n).is_some_and(|x| x.is_ascii_alphanumeric());
                if n == marker.len() && i > 0 && !b[i - 1].is_ascii_whitespace() && !after_word {
                    return Some(i);
                }
                i += n;
                continue;
            }
            _ => {}
        }
        i += 1;
    }
    None
}

/// `[text](url)` at the start of `s`: text, url and the whole length.
fn link(s: &str) -> Option<(&str, &str, usize)> {
    let close = s.find("](")?;
    let text = &s[1..close];
    if text.contains('\n') || text.is_empty() {
        return None;
    }
    let rest = &s[close + 2..];
    let end = rest.find(')')?;
    let url = rest[..end].split_whitespace().next()?;
    Some((text, url, close + 2 + end + 1))
}
