//! Tag editing in HTML, Vue, Svelte and JSX, as PyCharm does it: the closing tag added when
//! `>` ends an opening tag, `</` completed to the innermost open tag, the paired tag renamed
//! with the one being edited, and a small Emmet (`ul>li.item*3`, `a[href=#]{Home}`). Plain
//! text work, no syntax tree: it runs on every key.

use std::ops::Range;

/// Elements that never have a closing tag.
const VOID: &[&str] = &["area", "base", "br", "col", "embed", "hr", "img", "input", "link", "meta", "param", "source", "track", "wbr", "!doctype"];

fn is_name_char(c: char) -> bool {
    c.is_alphanumeric() || matches!(c, '-' | '_' | ':' | '.')
}

/// One tag of the text: its name range (bytes), whether it closes (`</x>`) and ends with `/>`.
#[derive(Clone, Debug, PartialEq, Eq)]
struct Tag {
    name: Range<usize>,
    closing: bool,
    self_closing: bool,
}

/// The tags of `text` in order, skipping comments, `<script>` / `<style>` bodies and quoted
/// attribute values. JSX expressions with `<` comparisons are not tags (`a < b` has a space).
fn tags(text: &str) -> Vec<Tag> {
    let b = text.as_bytes();
    let mut out = Vec::new();
    let mut i = 0;
    while i < b.len() {
        if b[i] != b'<' {
            i += 1;
            continue;
        }
        if text[i..].starts_with("<!--") {
            i = text[i..].find("-->").map_or(b.len(), |e| i + e + 3);
            continue;
        }
        let closing = b.get(i + 1) == Some(&b'/');
        let start = i + 1 + closing as usize;
        let end = start + text[start..].find(|c: char| !is_name_char(c) && c != '!').unwrap_or(text.len() - start);
        if end == start && !(closing && b.get(end) == Some(&b'>')) {
            i += 1;
            continue;
        }
        // The tag's end: `>` outside quotes.
        let mut j = end;
        let mut quote: Option<u8> = None;
        let mut brace = 0usize;
        while j < b.len() {
            match (quote, b[j]) {
                (Some(q), c) if c == q => quote = None,
                (Some(_), _) => {}
                (None, b'"' | b'\'') if brace == 0 => quote = Some(b[j]),
                (None, b'{') => brace += 1,
                (None, b'}') => brace = brace.saturating_sub(1),
                (None, b'>') if brace == 0 => break,
                _ => {}
            }
            j += 1;
        }
        let self_closing = j > 0 && b.get(j - 1) == Some(&b'/');
        out.push(Tag { name: start..end, closing, self_closing });
        let name = text[start..end].to_ascii_lowercase();
        // Raw text elements: skip to their closing tag.
        if !closing && (name == "script" || name == "style") && !self_closing {
            let close = format!("</{name}");
            i = text[j..].to_ascii_lowercase().find(&close).map_or(b.len(), |e| j + e);
            continue;
        }
        i = j + 1;
    }
    out
}

/// What to insert after `>` just typed at byte `at` (the `>` is at `at - 1`): `</name>` when
/// it ends an opening tag that is not void or self-closing and is not closed already. In
/// JSX (`jsx`) a `<` right after a name is a type argument (`Array<string>`), not a tag.
pub fn close_tag_after(text: &str, at: usize, jsx: bool) -> Option<String> {
    let before = &text[..at];
    if !before.ends_with('>') || before.ends_with("/>") || before.ends_with("->") || before.ends_with("=>") {
        return None;
    }
    let ident = |c: char| c.is_alphanumeric() || c == '_' || c == '$' || c == '.';
    // A JSX fragment `<>` closes with `</>`.
    if jsx && before.ends_with("<>") && !before[..at - 2].ends_with(ident) && !text[at..].trim_start().starts_with("</>") {
        return Some("</>".into());
    }
    let ts = tags(before);
    let t = ts.last()?;
    // The `>` typed must be this tag's end.
    if t.closing || t.self_closing || before[t.name.end..].contains('<') {
        return None;
    }
    let lt = t.name.start - 1;
    if jsx && before[..lt].ends_with(ident) {
        return None;
    }
    // A generic arrow function in TSX: `<T,>(x: T) =>`, `<T extends U>`.
    let rest = before[t.name.end..at - 1].trim_start();
    if jsx && (rest.starts_with(',') || rest.starts_with("extends ")) {
        return None;
    }
    let name = &before[t.name.clone()];
    if name.is_empty() || VOID.contains(&name.to_ascii_lowercase().as_str()) || name.starts_with('!') {
        return None;
    }
    // Already closed right after (typing `>` again inside `<div></div>`).
    if text[at..].trim_start().starts_with(&format!("</{name}")) {
        return None;
    }
    Some(format!("</{name}>"))
}

/// After `</` typed at byte `at`: the innermost tag still open there, as `name>`.
pub fn close_open_tag(text: &str, at: usize) -> Option<String> {
    let before = &text[..at];
    if !before.ends_with("</") {
        return None;
    }
    let mut stack: Vec<&str> = Vec::new();
    for t in tags(&before[..before.len() - 2]) {
        let name = &before[t.name.clone()];
        if t.self_closing || VOID.contains(&name.to_ascii_lowercase().as_str()) {
            continue;
        }
        if t.closing {
            if let Some(p) = stack.iter().rposition(|n| n.eq_ignore_ascii_case(name)) {
                stack.truncate(p);
            }
        } else {
            stack.push(name);
        }
    }
    stack.last().map(|n| format!("{n}>"))
}

/// The tag name range around byte `at` (the caret may be at its end), and the range of the
/// paired tag's name: the closing tag of an opening one, or the other way round.
pub fn paired_name(text: &str, at: usize) -> Option<(Range<usize>, Range<usize>)> {
    let ts = tags(text);
    let k = ts.iter().position(|t| t.name.start <= at && at <= t.name.end)?;
    let me = &ts[k];
    if me.self_closing {
        return None;
    }
    let name = &text[me.name.clone()];
    let same = |t: &Tag| text[t.name.clone()].eq_ignore_ascii_case(name);
    let mut depth = 0usize;
    if !me.closing {
        for t in &ts[k + 1..] {
            if t.self_closing || !same(t) {
                continue;
            }
            if t.closing {
                if depth == 0 {
                    return Some((me.name.clone(), t.name.clone()));
                }
                depth -= 1;
            } else {
                depth += 1;
            }
        }
    } else {
        for t in ts[..k].iter().rev() {
            if t.self_closing || !same(t) {
                continue;
            }
            if !t.closing {
                if depth == 0 {
                    return Some((me.name.clone(), t.name.clone()));
                }
                depth -= 1;
            } else {
                depth += 1;
            }
        }
    }
    None
}

// --- Emmet ---------------------------------------------------------------------------

/// The abbreviation that ends `line_prefix` (the line up to the caret): the text after the
/// last space or `>` of markup, if it looks like one (`ul>li*3`, `.card`, `a[href=x]`).
pub fn emmet_abbr(line_prefix: &str) -> Option<&str> {
    // Inside braces or brackets the space belongs to the abbreviation: walk back to a space
    // at depth zero.
    let mut depth = 0i32;
    let mut start = 0;
    for (i, c) in line_prefix.char_indices().rev() {
        match c {
            '}' | ']' | ')' => depth += 1,
            '{' | '[' | '(' => depth -= 1,
            ' ' | '\t' if depth == 0 => {
                start = i + 1;
                break;
            }
            _ => {}
        }
        if depth < 0 {
            return None;
        }
    }
    let a = &line_prefix[start..];
    let first = a.chars().next()?;
    let ok = (first.is_ascii_alphabetic() || first == '.' || first == '#')
        && a.chars().all(|c| c.is_alphanumeric() || "-_.#>+^*[]=\"'{}$@:/ ()!".contains(c))
        // A plain word is a tag only if it is a known one or has an operator.
        && (a.contains(['>', '+', '^', '*', '.', '#', '[', '{']) || KNOWN.contains(&a));
    // `</div` or text after a tag is not an abbreviation.
    (ok && !line_prefix[..start].trim_end().ends_with('<')).then_some(a)
}

/// Tags a bare word expands as.
const KNOWN: &[&str] = &[
    "a",
    "abbr",
    "article",
    "aside",
    "audio",
    "b",
    "blockquote",
    "body",
    "br",
    "button",
    "canvas",
    "caption",
    "code",
    "dd",
    "details",
    "div",
    "dl",
    "dt",
    "em",
    "fieldset",
    "figcaption",
    "figure",
    "footer",
    "form",
    "h1",
    "h2",
    "h3",
    "h4",
    "h5",
    "h6",
    "head",
    "header",
    "hr",
    "html",
    "i",
    "iframe",
    "img",
    "input",
    "label",
    "legend",
    "li",
    "link",
    "main",
    "meta",
    "nav",
    "ol",
    "option",
    "p",
    "pre",
    "script",
    "section",
    "select",
    "small",
    "span",
    "strong",
    "style",
    "summary",
    "table",
    "tbody",
    "td",
    "template",
    "textarea",
    "tfoot",
    "th",
    "thead",
    "title",
    "tr",
    "u",
    "ul",
    "video",
];

#[derive(Debug, Default, Clone)]
struct Node {
    tag: String,
    id: String,
    classes: Vec<String>,
    attrs: Vec<(String, String)>,
    text: String,
    children: Vec<Node>,
}

/// Default attributes Emmet adds.
fn default_attrs(tag: &str) -> &'static [(&'static str, &'static str)] {
    match tag {
        "a" => &[("href", "")],
        "img" => &[("src", ""), ("alt", "")],
        "input" => &[("type", "text")],
        "link" => &[("rel", "stylesheet"), ("href", "")],
        "script" => &[("src", "")],
        "form" => &[("action", "")],
        "label" => &[("for", "")],
        _ => &[],
    }
}

/// Parses one element: `tag.cls#id[attr=v]{text}*n`; returns the copies and the rest.
fn element(s: &str) -> Option<(Vec<Node>, &str)> {
    let mut n = Node::default();
    let mut rest = s;
    fn take_name(r: &str) -> (String, &str) {
        let end = r.find(|c: char| !(c.is_alphanumeric() || c == '-' || c == '_' || c == ':' || c == '$' || c == '@')).unwrap_or(r.len());
        (r[..end].to_string(), &r[end..])
    }
    let (tag, r) = take_name(rest);
    n.tag = tag;
    rest = r;
    let mut count = 1usize;
    loop {
        if let Some(r) = rest.strip_prefix('.') {
            let (c, r) = take_name(r);
            if c.is_empty() {
                return None;
            }
            n.classes.push(c);
            rest = r;
        } else if let Some(r) = rest.strip_prefix('#') {
            let (c, r) = take_name(r);
            n.id = c;
            rest = r;
        } else if let Some(r) = rest.strip_prefix('[') {
            let end = r.find(']')?;
            for a in r[..end].split_whitespace() {
                let (k, v) = a.split_once('=').unwrap_or((a, ""));
                n.attrs.push((k.to_string(), v.trim_matches(['"', '\'']).to_string()));
            }
            rest = &r[end + 1..];
        } else if let Some(r) = rest.strip_prefix('{') {
            let end = r.find('}')?;
            n.text = r[..end].to_string();
            rest = &r[end + 1..];
        } else if let Some(r) = rest.strip_prefix('*') {
            let end = r.find(|c: char| !c.is_ascii_digit()).unwrap_or(r.len());
            count = r[..end].parse().ok().filter(|&c| (1..=100).contains(&c))?;
            rest = &r[end..];
        } else {
            break;
        }
    }
    if n.tag.is_empty() {
        n.tag = "div".into();
    }
    Some(((1..=count).map(|i| numbered(&n, i)).collect(), rest))
}

/// `$` in names and text becomes the copy's number (`item$` → `item1`).
fn numbered(n: &Node, i: usize) -> Node {
    let f = |s: &str| s.replace('$', &i.to_string());
    Node {
        tag: n.tag.clone(),
        id: f(&n.id),
        classes: n.classes.iter().map(|c| f(c)).collect(),
        attrs: n.attrs.iter().map(|(k, v)| (k.clone(), f(v))).collect(),
        text: f(&n.text),
        children: Vec::new(),
    }
}

/// `a>b+c>d`: siblings joined by `+`, `>` nests what follows in the last one, `^` climbs.
fn parse(s: &str) -> Option<Vec<Node>> {
    // Levels of the current chain: siblings at each depth.
    // Levels of the current chain: siblings at each depth, and how many nodes the last
    // element group there has (`li*3` is three).
    let mut levels: Vec<(Vec<Node>, usize)> = vec![(Vec::new(), 0)];
    let mut rest = s;
    loop {
        let (nodes, r) = element(rest)?;
        let level = levels.last_mut()?;
        level.1 = nodes.len();
        level.0.extend(nodes);
        rest = r;
        match rest.chars().next() {
            None => break,
            Some('>') => {
                levels.push((Vec::new(), 0));
                rest = &rest[1..];
            }
            Some('+') => rest = &rest[1..],
            Some('^') => {
                let mut r = rest;
                while let Some(x) = r.strip_prefix('^') {
                    fold(&mut levels);
                    r = x;
                }
                rest = r;
            }
            _ => return None,
        }
    }
    while levels.len() > 1 {
        fold(&mut levels);
    }
    levels.pop().map(|l| l.0)
}

/// Puts the deepest level's nodes into every node of the last element group above it.
fn fold(levels: &mut Vec<(Vec<Node>, usize)>) {
    if levels.len() < 2 {
        return;
    }
    let (kids, _) = levels.pop().unwrap_or_default();
    if let Some((parents, n)) = levels.last_mut() {
        let from = parents.len().saturating_sub(*n);
        for p in &mut parents[from..] {
            p.children.extend(kids.iter().cloned());
        }
    }
}

/// `className` instead of `class` in JSX.
fn render(nodes: &[Node], indent: &str, unit: &str, jsx: bool, out: &mut String, caret: &mut Option<usize>) {
    for (k, n) in nodes.iter().enumerate() {
        if k > 0 || !out.is_empty() {
            out.push('\n');
            out.push_str(indent);
        }
        out.push('<');
        out.push_str(&n.tag);
        if !n.id.is_empty() {
            out.push_str(&format!(" id=\"{}\"", n.id));
        }
        if !n.classes.is_empty() {
            out.push_str(&format!(" {}=\"{}\"", if jsx { "className" } else { "class" }, n.classes.join(" ")));
        }
        let mut attrs: Vec<(String, String)> = n.attrs.clone();
        for (k, v) in default_attrs(&n.tag) {
            if !attrs.iter().any(|(a, _)| a == k) {
                attrs.push((k.to_string(), v.to_string()));
            }
        }
        for (a, v) in &attrs {
            out.push_str(&format!(" {a}=\""));
            if v.is_empty() && caret.is_none() {
                *caret = Some(out.len());
            }
            out.push_str(v);
            out.push('"');
        }
        if VOID.contains(&n.tag.as_str()) {
            out.push_str(if jsx { " />" } else { ">" });
            continue;
        }
        out.push('>');
        if n.children.is_empty() {
            out.push_str(&n.text);
            if n.text.is_empty() && caret.is_none() {
                *caret = Some(out.len());
            }
        } else {
            out.push_str(&n.text);
            let inner = format!("{indent}{unit}");
            out.push('\n');
            out.push_str(&inner);
            let mut child = String::new();
            let mut child_caret = None;
            render(&n.children, &inner, unit, jsx, &mut child, &mut child_caret);
            if caret.is_none() {
                *caret = child_caret.map(|c| out.len() + c);
            }
            out.push_str(&child);
            out.push('\n');
            out.push_str(indent);
        }
        out.push_str(&format!("</{}>", n.tag));
    }
}

/// The markup for an abbreviation and where the caret goes in it (the first empty
/// attribute or element), indented with `indent` (the line's) and `unit` per level.
pub fn expand(abbr: &str, indent: &str, unit: &str, jsx: bool) -> Option<(String, usize)> {
    let nodes = parse(abbr)?;
    let mut out = String::new();
    let mut caret = None;
    render(&nodes, indent, unit, jsx, &mut out, &mut caret);
    let len = out.len();
    Some((out, caret.unwrap_or(len)))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn closes_opening_tags() {
        let t = "<div class=\"a>b\">";
        assert_eq!(close_tag_after(t, t.len(), false).as_deref(), Some("</div>"));
        assert_eq!(close_tag_after("<br>", 4, false), None);
        assert_eq!(close_tag_after("<img src=x />", 13, false), None);
        assert_eq!(close_tag_after("</p>", 4, false), None);
        assert_eq!(close_tag_after("const f = () =>", 15, false), None);
        let t = "<Button onClick={() => go()}>";
        assert_eq!(close_tag_after(t, t.len(), false).as_deref(), Some("</Button>"));
        let t = "<div></div>";
        assert_eq!(close_tag_after(t, 5, false), None);
        let t = "const a: Array<string>";
        assert_eq!(close_tag_after(t, t.len(), true), None);
        let t = "const f = <T,>";
        assert_eq!(close_tag_after(t, t.len(), true), None);
        let t = "const g = <T extends object>";
        assert_eq!(close_tag_after(t, t.len(), true), None);
        let t = "return <>";
        assert_eq!(close_tag_after(t, t.len(), true).as_deref(), Some("</>"));
        assert_eq!(close_tag_after(t, t.len(), false), None);
    }

    #[test]
    fn completes_closing_slash() {
        let t = "<ul><li>a</li><li><b>x</b></";
        assert_eq!(close_open_tag(t, t.len()).as_deref(), Some("li>"));
        assert_eq!(close_open_tag("<br></", 6), None);
    }

    #[test]
    fn finds_paired_names() {
        let t = "<div><div>in</div></div>";
        // The outer opening tag pairs with the last closing one.
        let (me, other) = paired_name(t, 2).unwrap();
        assert_eq!((&t[me.clone()], other.start), ("div", 20));
        let (_, other) = paired_name(t, 20).unwrap();
        assert_eq!(other, 1..4);
        assert_eq!(paired_name("<br/>", 2), None);
    }

    #[test]
    fn emmet_abbreviations() {
        assert_eq!(emmet_abbr("  ul>li*2"), Some("ul>li*2"));
        assert_eq!(emmet_abbr("<p>hello world"), None);
        assert_eq!(emmet_abbr("a[href=x title=y]"), Some("a[href=x title=y]"));
        assert_eq!(emmet_abbr("word"), None);
        assert_eq!(emmet_abbr("div"), Some("div"));
        let (s, c) = expand("ul>li.item$*2", "", "  ", false).unwrap();
        assert_eq!(s, "<ul>\n  <li class=\"item1\"></li>\n  <li class=\"item2\"></li>\n</ul>");
        assert_eq!(&s[..c], "<ul>\n  <li class=\"item1\">");
        let (s, c) = expand("a{Home}", "", "  ", false).unwrap();
        assert_eq!(s, "<a href=\"\">Home</a>");
        assert_eq!(c, 9);
        let (s, _) = expand(".card>h2{Title}+p", "", "  ", true).unwrap();
        assert_eq!(s, "<div className=\"card\">\n  <h2>Title</h2>\n  <p></p>\n</div>");
        let (s, _) = expand("img", "", "  ", true).unwrap();
        assert_eq!(s, "<img src=\"\" alt=\"\" />");
        // Every repeated item gets the children; `^` climbs back up.
        let (s, _) = expand("ul>li*2>a", "", "  ", false).unwrap();
        assert_eq!(s, "<ul>\n  <li>\n    <a href=\"\"></a>\n  </li>\n  <li>\n    <a href=\"\"></a>\n  </li>\n</ul>");
        assert_eq!(emmet_abbr("div>p^span"), Some("div>p^span"));
        let (s, _) = expand("div>p^span", "", "  ", false).unwrap();
        assert_eq!(s, "<div>\n  <p></p>\n</div>\n<span></span>");
    }
}
