//! What the caret is on: a template name, a URL name, a URLconf module or a static file, in
//! Python code or in a Django / Jinja2 template.

use crate::ts;

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Ref {
    /// `"polls/detail.html"`.
    Template(String),
    /// A route name: `"polls:detail"`, `"read_item"`, `"auth.login"`.
    Url(String),
    /// A URLconf module in `include("polls.urls")`.
    Module(String),
    /// A file of a `static` folder: `{% static 'css/site.css' %}`.
    Static(String),
}

const TEMPLATE_EXT: &[&str] = &[".html", ".htm", ".txt", ".xml", ".jinja", ".jinja2", ".j2", ".djhtml", ".tpl", ".email", ".md"];

/// A string that names a template: a relative path with a template extension.
pub fn looks_like_template(s: &str) -> bool {
    let l = s.to_ascii_lowercase();
    !s.is_empty() && !s.contains(char::is_whitespace) && !s.contains("://") && !s.starts_with('/') && TEMPLATE_EXT.iter().any(|e| l.ends_with(e))
}

fn call_of(list: tree_sitter::Node) -> Option<tree_sitter::Node> {
    list.parent().filter(|c| c.kind() == "call")
}

/// The reference at a byte offset of Python source.
pub fn python_ref(src: &str, offset: usize) -> Option<Ref> {
    let tree = ts::parse(src)?;
    let mut n = tree.root_node().descendant_for_byte_range(offset, offset)?;
    for _ in 0..3 {
        if n.kind() == "string" {
            break;
        }
        n = n.parent()?;
    }
    if n.kind() != "string" {
        return None;
    }
    let value = ts::string_value(n, src)?;
    let parent = n.parent()?;
    match parent.kind() {
        "argument_list" => {
            let call = call_of(parent)?;
            let (args, _) = ts::args(call);
            let first = args.first().map(|a| a.id()) == Some(n.id());
            match ts::call_name(call, src).unwrap_or("") {
                "reverse" | "reverse_lazy" | "resolve_url" | "url_for" if first && value != "static" => return Some(Ref::Url(value)),
                "redirect" if first && !value.contains('/') && !looks_like_template(&value) => return Some(Ref::Url(value)),
                "include" if first => return Some(Ref::Module(value)),
                "static" if first => return Some(Ref::Static(value)),
                _ => {}
            }
        }
        "keyword_argument" => {
            // url_for("static", filename="css/site.css")
            let key = parent.child_by_field_name("name").map(|k| ts::text(k, src)).unwrap_or("");
            if let Some(call) = parent.parent().and_then(call_of) {
                let (args, _) = ts::args(call);
                let is_static = args.first().and_then(|a| ts::string_value(*a, src)).as_deref() == Some("static");
                if key == "filename" && is_static && ts::call_name(call, src) == Some("url_for") {
                    return Some(Ref::Static(value));
                }
            }
        }
        _ => {}
    }
    looks_like_template(&value).then_some(Ref::Template(value))
}

/// The reference at a byte offset of a template: `{% extends "base.html" %}`,
/// `{% include %}`, `{% import %}`, `{% url 'polls:detail' q.id %}`, `{% static 'a.css' %}`,
/// `{{ url_for('index') }}`.
pub fn template_ref(src: &str, offset: usize) -> Option<Ref> {
    let before = src.get(..offset)?;
    let open = [before.rfind("{%"), before.rfind("{{")].into_iter().flatten().max()?;
    let closed = [before.rfind("%}"), before.rfind("}}")].into_iter().flatten().max();
    if closed.is_some_and(|c| c > open) {
        return None;
    }
    let close = if &src[open..open + 2] == "{%" { "%}" } else { "}}" };
    let end = open + 2 + src[open + 2..].find(close)?;
    if offset > end {
        return None;
    }
    let body = &src[open + 2..end];
    let at = offset - (open + 2);
    // The quoted string around the caret.
    let mut i = 0;
    let bytes = body.as_bytes();
    let mut found = None;
    while i < bytes.len() {
        let q = bytes[i];
        if q == b'"' || q == b'\'' {
            let Some(len) = body[i + 1..].find(q as char) else { break };
            let (s, e) = (i + 1, i + 1 + len);
            if at >= i && at <= e + 1 {
                found = Some((s, e));
                break;
            }
            i = e + 1;
        } else {
            i += 1;
        }
    }
    let (s, e) = found?;
    let value = body[s..e].to_string();
    let head = body.trim_start().trim_start_matches(['-', '+']).trim_start();
    let tag = head.split(|c: char| !c.is_alphanumeric() && c != '_').next().unwrap_or("");
    let before_str = body[..s - 1].trim_end();
    if before_str.ends_with("url_for(") {
        return (value != "static").then_some(Ref::Url(value));
    }
    if close == "%}" {
        match tag {
            "extends" | "include" | "import" | "from" | "embed" => return Some(Ref::Template(value)),
            // `{% url 'name' arg %}`: the name is the first argument.
            "url" if before_str.trim_start().trim_start_matches(['-', '+']).trim() == "url" => return Some(Ref::Url(value)),
            "static" => return Some(Ref::Static(value)),
            _ => {}
        }
    }
    if before_str.ends_with("filename=") || before_str.ends_with("filename =") {
        return Some(Ref::Static(value));
    }
    looks_like_template(&value).then_some(Ref::Template(value))
}

/// Template names used in the function or class around a byte offset of Python source:
/// PyCharm's "related templates" of a view.
pub fn templates_in_scope(src: &str, offset: usize) -> Vec<String> {
    let Some(tree) = ts::parse(src) else { return Vec::new() };
    let Some(mut n) = tree.root_node().descendant_for_byte_range(offset, offset) else { return Vec::new() };
    let mut scope = None;
    loop {
        if matches!(n.kind(), "function_definition" | "class_definition") {
            scope = Some(n);
            // A method of a class-based view: the class may hold `template_name`.
            if n.kind() == "class_definition" {
                break;
            }
        }
        match n.parent() {
            Some(p) => n = p,
            None => break,
        }
    }
    let Some(scope) = scope else { return Vec::new() };
    let mut out: Vec<String> = Vec::new();
    ts::walk(scope, &mut |n| {
        if let Some(v) = ts::string_value(n, src).filter(|v| looks_like_template(v)) {
            if !out.contains(&v) {
                out.push(v);
            }
        }
    });
    out
}
