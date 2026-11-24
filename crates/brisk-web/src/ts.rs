//! Small helpers over tree-sitter-python trees.

use tree_sitter::{Node, Parser, Tree};

pub fn parse(src: &str) -> Option<Tree> {
    let mut p = Parser::new();
    p.set_language(&tree_sitter_python::LANGUAGE.into()).ok()?;
    p.parse(src, None)
}

pub fn text<'a>(n: Node, src: &'a str) -> &'a str {
    src.get(n.byte_range()).unwrap_or("")
}

/// Every node under `n`, depth first, in source order.
pub fn walk<'t>(n: Node<'t>, f: &mut impl FnMut(Node<'t>)) {
    let mut c = n.walk();
    let mut stack = vec![n];
    while let Some(n) = stack.pop() {
        f(n);
        let children: Vec<Node> = n.children(&mut c).collect();
        stack.extend(children.into_iter().rev());
    }
}

/// The value of a plain string literal (`"a"`, `'a'`, `r"^a$"`); None for f-strings and
/// anything else.
pub fn string_value(n: Node, src: &str) -> Option<String> {
    if n.kind() != "string" {
        return None;
    }
    let mut c = n.walk();
    let mut out = String::new();
    for ch in n.children(&mut c) {
        match ch.kind() {
            "string_content" | "escape_sequence" => out.push_str(text(ch, src)),
            "interpolation" => return None,
            _ => {}
        }
    }
    Some(out)
}

/// `render` for `render(...)` and `shortcuts.render(...)`.
pub fn call_name<'a>(call: Node, src: &'a str) -> Option<&'a str> {
    let f = call.child_by_field_name("function")?;
    match f.kind() {
        "identifier" => Some(text(f, src)),
        "attribute" => f.child_by_field_name("attribute").map(|a| text(a, src)),
        _ => None,
    }
}

/// Positional arguments and keyword arguments of a call.
pub fn args<'t>(call: Node<'t>) -> (Vec<Node<'t>>, Vec<(Node<'t>, Node<'t>)>) {
    let (mut pos, mut kw) = (Vec::new(), Vec::new());
    let Some(list) = call.child_by_field_name("arguments") else { return (pos, kw) };
    let mut c = list.walk();
    for a in list.named_children(&mut c) {
        match a.kind() {
            "comment" => {}
            "keyword_argument" => {
                if let (Some(k), Some(v)) = (a.child_by_field_name("name"), a.child_by_field_name("value")) {
                    kw.push((k, v));
                }
            }
            _ => pos.push(a),
        }
    }
    (pos, kw)
}

pub fn kwarg<'t>(kw: &[(Node<'t>, Node<'t>)], name: &str, src: &str) -> Option<Node<'t>> {
    kw.iter().find(|(k, _)| text(*k, src) == name).map(|(_, v)| *v)
}

/// The string elements of a list or tuple literal: `methods=["GET", "POST"]`.
pub fn string_list(n: Node, src: &str) -> Vec<String> {
    let mut c = n.walk();
    n.named_children(&mut c).filter_map(|e| string_value(e, src)).collect()
}
