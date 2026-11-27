//! HTML and XML. Two passes, both whitespace-safe: tags written one after another (`><`) go
//! on their own lines (in HTML only around block elements, so text and inline tags keep
//! their spacing), then every line is indented by the elements open at its start. A tag
//! spread over lines gets one extra level for its attribute lines. `<pre>`, `<textarea>`,
//! `<script>`, `<style>`, comments and CDATA keep their text as written.

const VOID: &[&str] = &["area", "base", "br", "col", "embed", "hr", "img", "input", "link", "meta", "param", "source", "track", "wbr"];
const RAW: &[&str] = &["script", "style", "pre", "textarea"];
/// Elements whose end tag HTML lets you leave out: a new one closes the open one.
const SELF_ENDING: &[&str] = &["li", "p", "td", "th", "tr", "option", "dt", "dd"];
const ITEM_STARTS: &[&str] = &["li", "tr", "td", "th", "dt", "dd", "option", "thead", "tbody", "tfoot"];
const LIST_ENDS: &[&str] = &["ul", "ol", "dl", "table", "thead", "tbody", "tfoot", "tr", "select"];
const INLINE: &[&str] = &[
    "a", "abbr", "b", "bdi", "bdo", "br", "button", "cite", "code", "data", "dfn", "em", "i", "img", "input", "kbd", "label", "mark", "q", "s", "samp",
    "select", "small", "span", "strong", "sub", "sup", "time", "u", "var", "wbr",
];

#[derive(Clone, Copy, PartialEq, Debug)]
enum State {
    Text,
    /// Inside `<…` before its `>`; the quote being read, if any.
    Tag(Option<u8>),
    Comment,
    Cdata,
    /// Inside a raw element's text, until `</name`.
    Raw(usize),
}

/// A tag read from `<` to `>`: its name (lower case for HTML), closing, self-closing.
fn tag_parts(tag: &str, html: bool) -> (String, bool, bool) {
    let inner = tag.trim_start_matches('<').trim_end_matches('>');
    let closing = inner.starts_with('/');
    let name: String = inner.trim_start_matches('/').chars().take_while(|c| !c.is_whitespace() && *c != '/' && *c != '>').collect();
    let name = if html { name.to_ascii_lowercase() } else { name };
    (name, closing, inner.trim_end().ends_with('/'))
}

/// Pass 1: `><` with only spaces between becomes a line break where it is safe.
fn split(text: &str, html: bool) -> String {
    let b = text.as_bytes();
    let mut out = String::with_capacity(text.len() + text.len() / 8);
    let mut i = 0;
    while i < b.len() {
        if b[i] == b'<' {
            let last_tag: (String, bool);
            // Comments and CDATA: copied whole.
            let special = [("<!--", "-->"), ("<![CDATA[", "]]>")].into_iter().find(|(open, _)| text[i..].starts_with(open));
            if let Some((_, close)) = special {
                let end = text[i..].find(close).map_or(b.len(), |e| i + e + close.len());
                out.push_str(&text[i..end]);
                i = end;
                last_tag = ("!".into(), false);
            } else {
                let end = tag_end(b, i).unwrap_or(b.len());
                let tag = &text[i..end];
                let (name, closing, _) = tag_parts(tag, html);
                out.push_str(tag);
                i = end;
                if html && !closing && RAW.contains(&name.as_str()) {
                    let close = format!("</{name}");
                    let stop = text[i..].to_ascii_lowercase().find(&close).map_or(b.len(), |e| i + e);
                    out.push_str(&text[i..stop]);
                    i = stop;
                    // Its end tag follows its text directly.
                    continue;
                }
                last_tag = (name, closing);
            }
            // Spaces up to the next tag.
            let ws_end = i + text[i..].find(|c: char| !c.is_whitespace()).unwrap_or(b.len() - i);
            if ws_end < b.len() && b[ws_end] == b'<' && !text[i..ws_end].contains('\n') {
                let next = &text[ws_end..];
                let (next_name, next_closing, _) = tag_parts(&next[..tag_end(next.as_bytes(), 0).unwrap_or(next.len())], html);
                let (prev, prev_closing) = last_tag;
                let inline = |n: &str| INLINE.contains(&n);
                // `<x></x>` stays on one line.
                let empty = !prev_closing && next_closing && prev == next_name;
                let safe = !empty && (!html || (!inline(&prev) && !inline(&next_name) && !prev.is_empty() && !next_name.is_empty()));
                if safe {
                    out.push('\n');
                    i = ws_end;
                }
            }
            continue;
        }
        // Text: a list item, row or cell starting after it, or a list or table ending after
        // it, goes on its own line (the whitespace there does not show).
        let ch = text[i..].chars().next().unwrap_or(' ');
        out.push(ch);
        i += ch.len_utf8();
        if html && !ch.is_whitespace() && b.get(i) == Some(&b'<') {
            let next = &text[i..];
            let (name, closing, _) = tag_parts(&next[..tag_end(next.as_bytes(), 0).unwrap_or(next.len())], html);
            let breaks = if closing { LIST_ENDS.contains(&name.as_str()) } else { ITEM_STARTS.contains(&name.as_str()) };
            if breaks {
                out.push('\n');
            }
        }
    }
    out
}

/// The byte after the `>` of the tag starting at `i`, outside quotes.
fn tag_end(b: &[u8], i: usize) -> Option<usize> {
    let mut q: Option<u8> = None;
    for (k, &c) in b.iter().enumerate().skip(i + 1) {
        match (q, c) {
            (Some(x), c) if c == x => q = None,
            (Some(_), _) => {}
            (None, b'"' | b'\'') => q = Some(c),
            (None, b'>') => return Some(k + 1),
            _ => {}
        }
    }
    None
}

/// Pass 2: indentation by the open elements.
fn indent(text: &str, unit: &str, html: bool) -> Result<String, String> {
    let mut out = String::with_capacity(text.len() + text.len() / 4);
    let mut stack: Vec<String> = Vec::new();
    let mut state = State::Text;
    let mut tag_buf = String::new();
    let mut raw_names: Vec<String> = Vec::new();
    for (n, line) in text.split('\n').enumerate() {
        let line = line.trim_end_matches('\r');
        if n > 0 {
            out.push('\n');
        }
        let trimmed = line.trim();
        // Raw text, comments and CDATA keep their lines as written.
        let verbatim = matches!(state, State::Comment | State::Cdata | State::Raw(_))
            && !(matches!(state, State::Raw(_)) && trimmed.to_ascii_lowercase().starts_with("</"));
        let continuation = matches!(state, State::Tag(_));
        if continuation {
            tag_buf.push(' ');
        }
        let depth_before = stack.len();
        // The depth after the closing tags that start the line: its indentation.
        let mut lead = depth_before;
        let mut leading = true;
        // Scan the line.
        let b = line.as_bytes();
        let mut i = 0;
        while i < b.len() {
            match state {
                State::Comment => match line[i..].find("-->") {
                    Some(e) => {
                        i += e + 3;
                        state = State::Text;
                    }
                    None => break,
                },
                State::Cdata => match line[i..].find("]]>") {
                    Some(e) => {
                        i += e + 3;
                        state = State::Text;
                    }
                    None => break,
                },
                State::Raw(k) => {
                    let close = format!("</{}", raw_names[k]);
                    match line[i..].to_ascii_lowercase().find(&close) {
                        Some(e) => {
                            i += e;
                            state = State::Text;
                        }
                        None => break,
                    }
                }
                State::Tag(q) => {
                    let c = b[i];
                    tag_buf.push(c as char);
                    match (q, c) {
                        (Some(x), c) if c == x => state = State::Tag(None),
                        (Some(_), _) => {}
                        (None, b'"' | b'\'') => state = State::Tag(Some(c)),
                        (None, b'>') => {
                            state = State::Text;
                            let (name, closing, self_closing) = tag_parts(&tag_buf, html);
                            let void = html && VOID.contains(&name.as_str());
                            let special = name.starts_with('!') || name.starts_with('?');
                            if closing {
                                if !html && stack.last() != Some(&name) {
                                    return Err(match stack.last() {
                                        Some(open) => format!("line {}: </{name}> closes <{open}>", n + 1),
                                        None => format!("line {}: </{name}> closes nothing", n + 1),
                                    });
                                }
                                if let Some(p) = stack.iter().rposition(|s| *s == name) {
                                    stack.truncate(p);
                                }
                                if leading {
                                    lead = stack.len();
                                }
                            } else {
                                leading = false;
                                if !self_closing && !void && !special {
                                    if html && SELF_ENDING.contains(&name.as_str()) && stack.last() == Some(&name) {
                                        stack.pop();
                                        if line.trim_start().starts_with('<') && i < line.len() && depth_before > stack.len() {
                                            lead = lead.min(stack.len());
                                        }
                                    }
                                    if html && RAW.contains(&name.as_str()) {
                                        raw_names.push(name.clone());
                                        state = State::Raw(raw_names.len() - 1);
                                    }
                                    stack.push(name);
                                }
                            }
                            tag_buf.clear();
                        }
                        _ => {}
                    }
                    i += 1;
                    continue;
                }
                State::Text => {
                    let c = b[i];
                    if c == b'<' {
                        if line[i..].starts_with("<!--") {
                            leading = false;
                            state = State::Comment;
                            i += 4;
                            continue;
                        }
                        if line[i..].starts_with("<![CDATA[") {
                            leading = false;
                            state = State::Cdata;
                            i += 9;
                            continue;
                        }
                        state = State::Tag(None);
                        tag_buf.clear();
                        tag_buf.push('<');
                    } else if !c.is_ascii_whitespace() {
                        leading = false;
                    }
                    i += 1;
                    continue;
                }
            }
        }
        if verbatim {
            out.push_str(line);
            continue;
        }
        if trimmed.is_empty() {
            continue;
        }
        let level = if continuation { depth_before + 1 } else { lead.min(depth_before) };
        for _ in 0..level {
            out.push_str(unit);
        }
        out.push_str(trimmed);
    }
    if matches!(state, State::Tag(_)) {
        return Err("a tag is not closed with '>'".into());
    }
    if !html {
        if let Some(open) = stack.last() {
            return Err(format!("<{open}> is not closed"));
        }
    }
    let mut s = out.trim_end().to_string();
    s.push('\n');
    Ok(s)
}

pub fn format(text: &str, unit: &str, html: bool) -> Result<String, String> {
    indent(&split(text, html), unit, html)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn minified_html() {
        let t = "<!doctype html><html><head><meta charset=\"utf-8\"><title>T</title></head><body><div class=\"a\"><p>Hi <b>you</b> there</p><ul><li>One<li>Two</ul></div><pre>  keep\n    this</pre></body></html>";
        let want = "<!doctype html>\n<html>\n  <head>\n    <meta charset=\"utf-8\">\n    <title>T</title>\n  </head>\n  <body>\n    <div class=\"a\">\n      <p>Hi <b>you</b> there</p>\n      <ul>\n        <li>One\n        <li>Two\n      </ul>\n    </div>\n    <pre>  keep\n    this</pre>\n  </body>\n</html>\n";
        let got = format(t, "  ", true).unwrap();
        assert_eq!(got, want);
        assert_eq!(format(&got, "  ", true).unwrap(), got);
    }

    #[test]
    fn xml_and_attributes() {
        let t = "<?xml version=\"1.0\"?><root><item id=\"1\"><name>A &amp; B</name><empty/></item><!-- note --><item\nid=\"2\"\nkind=\"x\"></item></root>";
        let got = format(t, "    ", false).unwrap();
        let want = "<?xml version=\"1.0\"?>\n<root>\n    <item id=\"1\">\n        <name>A &amp; B</name>\n        <empty/>\n    </item>\n    <!-- note -->\n    <item\n        id=\"2\"\n        kind=\"x\"></item>\n</root>\n";
        assert_eq!(got, want);
        assert_eq!(format(&got, "    ", false).unwrap(), got);
        assert!(format("<a><b></a>", "  ", false).is_err());
    }

    #[test]
    fn script_kept() {
        let t = "<div>\n<script>\n  if (a < b) { x(); }\n</script>\n</div>";
        assert_eq!(format(t, "  ", true).unwrap(), "<div>\n  <script>\n  if (a < b) { x(); }\n  </script>\n</div>\n");
    }
}
