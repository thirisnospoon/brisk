//! Alt+Enter intentions from the syntax tree, worded as in PyCharm: Convert to f-string literal,
//! Invert 'if' condition, Split into 2 'if' statements, Merge nested 'if's; and the places the
//! quick fixes write to: a created function, an import, a type hint. Edits are byte ranges.

use crate::refactor::Edit;
use tree_sitter::{Node, Tree};

/// An intention: its label and the edits that apply it (one undo step).
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Intention {
    pub label: String,
    pub edits: Vec<Edit>,
}

/// Where a type hint goes: after a variable's name (`x` → `x: T`) or after a function's
/// parameters (`)` → `) -> T`). `name` is the byte where ty is asked about the type.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum HintTarget {
    Variable { name: String, at: usize, insert: usize },
    /// `returns`: what each `return` gives, for when ty has not inferred the return type.
    Return { name: String, at: usize, insert: usize, returns: Vec<Returned> },
}

/// A `return` value's type: known from the literal, to ask ty about (hover at the byte of
/// the expression's last name), or not known.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Returned {
    Known(String),
    Ask(usize, usize),
    Unknown,
}

fn src<'a>(n: Node, text: &'a str) -> &'a str {
    text.get(n.byte_range()).unwrap_or("")
}

fn node_at<'t>(tree: &'t Tree, text: &str, byte: usize) -> Node<'t> {
    let root = tree.root_node();
    let n = root.descendant_for_byte_range(byte, byte).unwrap_or(root);
    // At the end of a line the caret is past the last token: take the token before it.
    if matches!(n.kind(), "module" | "block") && byte > 0 && !text[..byte].ends_with('\n') {
        return root.descendant_for_byte_range(byte - 1, byte - 1).unwrap_or(n);
    }
    n
}

fn ancestor<'t>(mut n: Node<'t>, kinds: &[&str]) -> Option<Node<'t>> {
    loop {
        if kinds.contains(&n.kind()) {
            return Some(n);
        }
        n = n.parent()?;
    }
}

fn line_start(text: &str, byte: usize) -> usize {
    text[..byte].rfind('\n').map_or(0, |i| i + 1)
}

fn indent_at(text: &str, byte: usize) -> &str {
    let s = line_start(text, byte);
    let line = &text[s..];
    &line[..line.len() - line.trim_start_matches([' ', '\t']).len()]
}

/// The intentions that apply at `byte`, in PyCharm's order.
pub fn intentions_at(tree: &Tree, text: &str, byte: usize) -> Vec<Intention> {
    let n = node_at(tree, text, byte);
    let mut out = Vec::new();
    out.extend(to_fstring(n, text));
    if let Some(i) = if_at(n, byte) {
        out.extend(invert_if(i, text));
        out.extend(split_if(i, text));
        out.extend(merge_ifs(i, text));
    }
    out
}

/// The `if` whose header line (not its body) holds the caret.
fn if_at(n: Node, byte: usize) -> Option<Node> {
    let i = ancestor(n, &["if_statement"])?;
    let body = i.child_by_field_name("consequence")?;
    (byte <= body.start_byte() && i.start_position().row == n.start_position().row).then_some(i)
}

fn alternatives(i: Node) -> Vec<Node> {
    let mut c = i.walk();
    i.children_by_field_name("alternative", &mut c).collect()
}

const ATOMS: &[&str] = &["identifier", "attribute", "call", "subscript", "parenthesized_expression", "string", "integer", "float", "true", "false", "none", "list", "dictionary", "tuple", "set"];

/// `cond` negated the way PyCharm writes it: `a == b` → `a != b`, `not x` → `x`, else `not x`.
pub fn negate(cond: Node, text: &str) -> String {
    let s = src(cond, text);
    match cond.kind() {
        "not_operator" => cond.child_by_field_name("argument").map(|a| src(a, text).to_string()).unwrap_or_else(|| format!("not ({s})")),
        "true" => "False".into(),
        "false" => "True".into(),
        "comparison_operator" if cond.named_child_count() == 2 => {
            let (Some(a), Some(b)) = (cond.named_child(0), cond.named_child(1)) else { return format!("not ({s})") };
            let op = text[a.end_byte()..b.start_byte()].split_whitespace().collect::<Vec<_>>().join(" ");
            let flipped = match op.as_str() {
                "==" => "!=",
                "!=" => "==",
                "<" => ">=",
                ">" => "<=",
                "<=" => ">",
                ">=" => "<",
                "in" => "not in",
                "not in" => "in",
                "is" => "is not",
                "is not" => "is",
                _ => return format!("not ({s})"),
            };
            format!("{} {flipped} {}", src(a, text), src(b, text))
        }
        k if ATOMS.contains(&k) => format!("not {s}"),
        _ => format!("not ({s})"),
    }
}

/// `if c: A else: B` → `if not c: B else: A`.
fn invert_if(i: Node, text: &str) -> Option<Intention> {
    let cond = i.child_by_field_name("condition")?;
    let body = i.child_by_field_name("consequence")?;
    let alts = alternatives(i);
    let [els] = alts.as_slice() else { return None };
    if els.kind() != "else_clause" {
        return None;
    }
    let other = els.child_by_field_name("body")?;
    let edits = vec![
        (cond.byte_range(), negate(cond, text)),
        (body.byte_range(), src(other, text).to_string()),
        (other.byte_range(), src(body, text).to_string()),
    ];
    Some(Intention { label: "Invert 'if' condition".into(), edits })
}

/// The indentation step of a block under the statement at `stmt`.
fn unit(text: &str, stmt: Node, body: Node) -> String {
    let outer = indent_at(text, stmt.start_byte()).len();
    let inner = indent_at(text, body.start_byte());
    inner.get(outer..).filter(|u| !u.is_empty()).unwrap_or("    ").to_string()
}

/// The lines of `body` (from its first line's start), each with `f` applied.
fn body_lines(text: &str, body: Node, f: impl Fn(&str) -> String) -> String {
    text[line_start(text, body.start_byte())..body.end_byte()].split('\n').map(|l| if l.trim().is_empty() { String::new() } else { f(l) }).collect::<Vec<_>>().join("\n")
}

/// `if a and b: X` → `if a:` / `if b: X` nested.
fn split_if(i: Node, text: &str) -> Option<Intention> {
    let cond = i.child_by_field_name("condition")?;
    let body = i.child_by_field_name("consequence")?;
    if cond.kind() != "boolean_operator" || !alternatives(i).is_empty() {
        return None;
    }
    let (l, r) = (cond.child_by_field_name("left")?, cond.child_by_field_name("right")?);
    if text[l.end_byte()..r.start_byte()].trim() != "and" {
        return None;
    }
    let ind = indent_at(text, i.start_byte());
    let u = unit(text, i, body);
    let lines = body_lines(text, body, |l| format!("{u}{l}"));
    let new = format!("if {}:\n{ind}{u}if {}:\n{lines}", src(l, text), src(r, text));
    Some(Intention { label: "Split into 2 'if' statements".into(), edits: vec![(i.byte_range(), new)] })
}

/// `if a:` / `if b: X` (nothing else in the outer `if`) → `if a and b: X`.
fn merge_ifs(i: Node, text: &str) -> Option<Intention> {
    let body = i.child_by_field_name("consequence")?;
    if !alternatives(i).is_empty() || body.named_child_count() != 1 {
        return None;
    }
    let inner = body.named_child(0)?;
    if inner.kind() != "if_statement" || !alternatives(inner).is_empty() {
        return None;
    }
    let wrap = |c: Node| {
        let s = src(c, text);
        if matches!(c.kind(), "boolean_operator" | "conditional_expression" | "lambda" | "named_expression") && !(c.kind() == "boolean_operator" && text[c.child_by_field_name("left").map_or(0, |n| n.end_byte())..c.child_by_field_name("right").map_or(0, |n| n.start_byte())].trim() == "and") {
            format!("({s})")
        } else {
            s.to_string()
        }
    };
    let (a, b) = (i.child_by_field_name("condition")?, inner.child_by_field_name("condition")?);
    let inner_body = inner.child_by_field_name("consequence")?;
    let u = unit(text, i, body);
    let lines = body_lines(text, inner_body, |l| l.strip_prefix(u.as_str()).unwrap_or(l).to_string());
    let new = format!("if {} and {}:\n{lines}", wrap(a), wrap(b));
    Some(Intention { label: "Merge nested 'if's".into(), edits: vec![(i.byte_range(), new)] })
}

/// A plain string literal: its prefix (without `f`/`b`), quote and content; `None` for
/// f-strings, bytes and implicit concatenations.
fn plain_string(n: Node, text: &str) -> Option<(String, String, String)> {
    if n.kind() != "string" {
        return None;
    }
    let s = src(n, text);
    let q = s.find(['"', '\''])?;
    let prefix = &s[..q];
    if prefix.chars().any(|c| matches!(c.to_ascii_lowercase(), 'f' | 'b' | 'u')) {
        return None;
    }
    let quote = if s[q..].starts_with("\"\"\"") || s[q..].starts_with("'''") { &s[q..q + 3] } else { &s[q..q + 1] };
    let body = s.get(q + quote.len()..s.len() - quote.len())?;
    Some((prefix.to_string(), quote.to_string(), body.to_string()))
}

/// An argument that can go inside braces of an f-string with this quote (before 3.12 no
/// backslash and not the same quote).
fn fits(arg: &str, quote: &str) -> bool {
    !arg.contains('\\') && !arg.contains(&quote[..1]) && !arg.contains('\n')
}

/// `"{} {x}".format(a, x=b)` and `"%s %d" % (a, b)` → `f"{a} {b}"`.
fn to_fstring(n: Node, text: &str) -> Option<Intention> {
    let e = ancestor(n, &["call", "binary_operator"]).filter(|e| {
        if e.kind() == "call" {
            e.child_by_field_name("function").is_some_and(|f| f.kind() == "attribute" && f.child_by_field_name("object").is_some_and(|o| o.kind() == "string") && f.child_by_field_name("attribute").is_some_and(|a| src(a, text) == "format"))
        } else {
            e.child_by_field_name("left").is_some_and(|l| l.kind() == "string") && e.child_by_field_name("operator").is_some_and(|o| src(o, text) == "%")
        }
    })?;
    let new = if e.kind() == "call" { from_format(e, text)? } else { from_percent(e, text)? };
    Some(Intention { label: "Convert to f-string literal".into(), edits: vec![(e.byte_range(), new)] })
}

fn from_format(call: Node, text: &str) -> Option<String> {
    let s = call.child_by_field_name("function")?.child_by_field_name("object")?;
    let (prefix, quote, body) = plain_string(s, text)?;
    let args = call.child_by_field_name("arguments")?;
    let (mut pos, mut kw) = (Vec::new(), std::collections::HashMap::new());
    let mut c = args.walk();
    for a in args.named_children(&mut c) {
        match a.kind() {
            "keyword_argument" => {
                kw.insert(src(a.child_by_field_name("name")?, text), src(a.child_by_field_name("value")?, text));
            }
            "list_splat" | "dictionary_splat" | "comment" => return None,
            _ => pos.push(src(a, text)),
        }
    }
    let mut out = String::new();
    let mut auto = 0;
    let mut chars = body.char_indices().peekable();
    while let Some((i, ch)) = chars.next() {
        match ch {
            '{' if body[i + 1..].starts_with('{') => {
                out.push_str("{{");
                chars.next();
            }
            '}' if body[i + 1..].starts_with('}') => {
                out.push_str("}}");
                chars.next();
            }
            '{' => {
                let end = i + body[i..].find('}')?;
                let field = &body[i + 1..end];
                let split = field.find(['!', ':']).unwrap_or(field.len());
                let (name, rest) = field.split_at(split);
                let key_end = name.find(['.', '[']).unwrap_or(name.len());
                let (key, tail) = name.split_at(key_end);
                let value = if key.is_empty() {
                    auto += 1;
                    *pos.get(auto - 1)?
                } else if let Ok(k) = key.parse::<usize>() {
                    *pos.get(k)?
                } else {
                    *kw.get(key)?
                };
                if !fits(value, &quote) || rest.contains('{') {
                    return None;
                }
                let value = if tail.is_empty() || value.chars().all(|c| c.is_alphanumeric() || c == '_' || c == '.') { value.to_string() } else { format!("({value})") };
                out.push_str(&format!("{{{value}{tail}{rest}}}"));
                while chars.peek().is_some_and(|(j, _)| *j <= end) {
                    chars.next();
                }
            }
            _ => out.push(ch),
        }
    }
    Some(format!("f{prefix}{quote}{out}{quote}"))
}

fn from_percent(op: Node, text: &str) -> Option<String> {
    let (prefix, quote, body) = plain_string(op.child_by_field_name("left")?, text)?;
    let right = op.child_by_field_name("right")?;
    let values: Vec<&str> = if right.kind() == "tuple" {
        let mut c = right.walk();
        right.named_children(&mut c).map(|v| src(v, text)).collect()
    } else if right.kind() == "dictionary" {
        return None;
    } else {
        vec![src(right, text)]
    };
    let mut out = String::new();
    let mut k = 0;
    let b = body.as_bytes();
    let mut i = 0;
    while i < b.len() {
        let ch = body[i..].chars().next()?;
        match ch {
            '{' => out.push_str("{{"),
            '}' => out.push_str("}}"),
            '%' if body[i + 1..].starts_with('%') => {
                out.push('%');
                i += 1;
            }
            '%' => {
                // %[flags][width][.precision]type; `%(name)s` is not supported.
                let spec_len = body[i + 1..].find(|c: char| c.is_ascii_alphabetic())?;
                let spec = &body[i + 1..i + 1 + spec_len];
                if spec.contains(['(', '*']) {
                    return None;
                }
                let ty = body[i + 1 + spec_len..].chars().next()?;
                let v = *values.get(k)?;
                k += 1;
                if !fits(v, &quote) {
                    return None;
                }
                let field = match ty {
                    's' if spec.is_empty() => format!("{{{v}}}"),
                    'r' if spec.is_empty() => format!("{{{v}!r}}"),
                    'a' if spec.is_empty() => format!("{{{v}!a}}"),
                    'd' | 'i' | 'u' if spec.is_empty() => format!("{{{v}}}"),
                    's' => format!("{{{v}!s:{}}}", spec.replace('-', "<")),
                    'd' | 'i' | 'u' => format!("{{{v}:{}d}}", spec.replace('-', "<")),
                    'f' | 'F' | 'e' | 'E' | 'g' | 'G' | 'x' | 'X' | 'o' | 'c' => format!("{{{v}:{}{ty}}}", spec.replace('-', "<")),
                    _ => return None,
                };
                out.push_str(&field);
                i += 1 + spec_len;
            }
            _ => out.push(ch),
        }
        i += ch.len_utf8();
    }
    (k == values.len()).then(|| format!("f{prefix}{quote}{out}{quote}"))
}

/// The top-level statement holding `n` (a child of the module).
fn top_statement(n: Node) -> Option<Node> {
    let mut n = n;
    while let Some(p) = n.parent() {
        if p.kind() == "module" {
            return Some(n);
        }
        n = p;
    }
    None
}

/// "Create function 'name'" for a call of an unknown plain name at `byte`: `def name(…):` with
/// parameters named after the arguments, put before the top-level statement using it.
pub fn create_function(tree: &Tree, text: &str, byte: usize) -> Option<(String, Edit)> {
    let n = node_at(tree, text, byte);
    let id = if n.kind() == "identifier" { n } else { return None };
    let call = id.parent().filter(|c| c.kind() == "call" && c.child_by_field_name("function") == Some(id))?;
    let name = src(id, text).to_string();
    let args = call.child_by_field_name("arguments")?;
    let mut params: Vec<String> = Vec::new();
    let mut c = args.walk();
    for a in args.named_children(&mut c) {
        let p = match a.kind() {
            "identifier" => src(a, text).to_string(),
            "keyword_argument" => src(a.child_by_field_name("name")?, text).to_string(),
            "list_splat" => "*args".into(),
            "dictionary_splat" => "**kwargs".into(),
            "comment" => continue,
            _ => "param".into(),
        };
        let mut q = p.clone();
        let mut k = 1;
        while params.contains(&q) {
            q = format!("{p}{k}");
            k += 1;
        }
        params.push(q);
    }
    let top = top_statement(call)?;
    // Decorators and comments right above stay with their statement.
    let top = top.parent().filter(|p| p.kind() == "decorated_definition").unwrap_or(top);
    let at = line_start(text, top.start_byte());
    // Two blank lines around it, as PEP 8 (and PyCharm) puts them.
    let blank = text[..at].split('\n').rev().skip(1).take_while(|l| l.trim().is_empty()).count();
    let gap = if at == 0 { "" } else { &"\n\n"[blank.min(2)..] };
    let def = format!("{gap}def {name}({}):\n    pass\n\n\n", params.join(", "));
    Some((name, (at..at, def)))
}

/// Where `line` (an import statement) goes: after the imports at the top of the module (or
/// after its docstring), as an insertion edit.
pub fn import_edit(tree: &Tree, text: &str, line: &str) -> Edit {
    let (row, after) = crate::refactor::constant_row(tree);
    let at = text.split_inclusive('\n').take(row).map(str::len).sum::<usize>().min(text.len());
    let gap = if after && !text[..at].ends_with('\n') { "\n" } else { "" };
    let tail = if !after && !text[at..].trim().is_empty() { "\n" } else { "" };
    (at..at, format!("{gap}{line}\n{tail}"))
}

/// The values a function's `return` statements give, not looking into nested functions,
/// lambdas or classes.
fn returns_of(n: Node, text: &str, out: &mut Vec<Returned>) {
    let mut c = n.walk();
    for k in n.named_children(&mut c) {
        match k.kind() {
            "function_definition" | "class_definition" | "lambda" | "decorated_definition" => {}
            "return_statement" => out.push(match k.named_child(0) {
                None => Returned::Known("None".into()),
                Some(v) => match v.kind() {
                    "none" => Returned::Known("None".into()),
                    "true" | "false" => Returned::Known("bool".into()),
                    "integer" => Returned::Known("int".into()),
                    "float" => Returned::Known("float".into()),
                    "string" | "concatenated_string" => {
                        let s = src(v, text);
                        let prefix = &s[..s.find(['"', '\'']).unwrap_or(0)];
                        Returned::Known(if prefix.contains(['b', 'B']) { "bytes" } else { "str" }.into())
                    }
                    "identifier" | "call" | "attribute" | "subscript" => Returned::Ask(v.start_byte(), if v.kind() == "call" { v.child_by_field_name("function").map_or(v.start_byte(), |f| f.end_byte() - 1) } else { v.end_byte() - 1 }),
                    _ => Returned::Unknown,
                },
            }),
            _ => returns_of(k, text, out),
        }
    }
}

/// The variable or function a type hint can be added to at `byte`, if it has none yet.
pub fn hint_target(tree: &Tree, text: &str, byte: usize) -> Option<HintTarget> {
    let n = node_at(tree, text, byte);
    if let Some(f) = ancestor(n, &["function_definition"]).filter(|f| f.child_by_field_name("body").is_some_and(|b| byte <= b.start_byte())) {
        if f.child_by_field_name("return_type").is_some() {
            return None;
        }
        let name = f.child_by_field_name("name")?;
        let params = f.child_by_field_name("parameters")?;
        let mut returns = Vec::new();
        returns_of(f.child_by_field_name("body")?, text, &mut returns);
        if returns.is_empty() {
            returns.push(Returned::Known("None".into()));
        }
        return Some(HintTarget::Return { name: src(name, text).into(), at: name.start_byte(), insert: params.end_byte(), returns });
    }
    let a = ancestor(n, &["assignment"])?;
    let left = a.child_by_field_name("left")?;
    if left.kind() != "identifier" || a.child_by_field_name("type").is_some() || a.child_by_field_name("right").is_none() {
        return None;
    }
    Some(HintTarget::Variable { name: src(left, text).into(), at: left.start_byte(), insert: left.end_byte() })
}

#[cfg(test)]
mod tests {
    use super::*;

    fn tree(t: &str) -> Tree {
        crate::parse_full(crate::Lang::Python, t, None).unwrap()
    }

    fn apply(text: &str, mut edits: Vec<Edit>) -> String {
        edits.sort_by_key(|e| std::cmp::Reverse(e.0.start));
        let mut s = text.to_string();
        for (r, t) in edits {
            s.replace_range(r, &t);
        }
        s
    }

    fn run(text: &str, at: &str, label: &str) -> Option<String> {
        let t = tree(text);
        let b = text.find(at).unwrap();
        intentions_at(&t, text, b).into_iter().find(|i| i.label == label).map(|i| apply(text, i.edits))
    }

    #[test]
    fn fstrings() {
        let t = "x = '{} and {name}!'.format(a, name=b.c)\n";
        assert_eq!(run(t, "and", "Convert to f-string literal").unwrap(), "x = f'{a} and {b.c}!'\n");
        let t = "x = \"%s: %5.2f%% %r\" % (k, v * 2, o)\n";
        assert_eq!(run(t, "%s", "Convert to f-string literal").unwrap(), "x = f\"{k}: {v * 2:5.2f}% {o!r}\"\n");
        assert_eq!(run("print('{0} {{x}}'.format(n))\n", "{0}", "Convert to f-string literal").unwrap(), "print(f'{n} {{x}}')\n");
        // The same quote inside an argument cannot go into the braces before Python 3.12.
        assert!(run("x = '{}'.format(d['k'])\n", "{}", "Convert to f-string literal").is_none());
    }

    #[test]
    fn ifs() {
        let t = "def f(a, b):\n    if a == 1:\n        x()\n    else:\n        y()\n";
        assert_eq!(run(t, "if a", "Invert 'if' condition").unwrap(), "def f(a, b):\n    if a != 1:\n        y()\n    else:\n        x()\n");
        let t = "if a and b or c:\n    x()\n";
        assert!(run(t, "if", "Split into 2 'if' statements").is_none());
        let t = "if a and not b:\n    x()\n    y()\n";
        let split = run(t, "if", "Split into 2 'if' statements").unwrap();
        assert_eq!(split, "if a:\n    if not b:\n        x()\n        y()\n");
        assert_eq!(run(&split, "if a", "Merge nested 'if's").unwrap(), "if a and not b:\n    x()\n    y()\n");
        assert_eq!(run("if a or b:\n    if c:\n        x()\n", "if a", "Merge nested 'if's").unwrap(), "if (a or b) and c:\n    x()\n");
    }

    #[test]
    fn create_and_import() {
        let t = "import os\n\n\ndef main():\n    total = compute(os, 3, key=1)\n";
        let tr = tree(t);
        let (name, e) = create_function(&tr, t, t.find("compute").unwrap() + 2).unwrap();
        assert_eq!(name, "compute");
        assert_eq!(apply(t, vec![e]), "import os\n\n\ndef compute(os, param, key):\n    pass\n\n\ndef main():\n    total = compute(os, 3, key=1)\n");
        let t2 = "x = 1\ny = go(x)\n";
        let (_, e) = create_function(&tree(t2), t2, t2.find("go").unwrap()).unwrap();
        assert_eq!(apply(t2, vec![e]), "x = 1\n\n\ndef go(x):\n    pass\n\n\ny = go(x)\n");
        assert_eq!(apply(t, vec![import_edit(&tr, t, "from x import y")]), "import os\nfrom x import y\n\n\ndef main():\n    total = compute(os, 3, key=1)\n");
        let t = "print(1)\n";
        assert_eq!(apply(t, vec![import_edit(&tree(t), t, "import os")]), "import os\n\nprint(1)\n");
    }

    #[test]
    fn hint_targets() {
        let t = "def f(a):\n    return [a]\n\nx = f(1)\ny: int = 2\n";
        let tr = tree(t);
        assert_eq!(hint_target(&tr, t, 4), Some(HintTarget::Return { name: "f".into(), at: 4, insert: 8, returns: vec![Returned::Unknown] }));
        let t2 = "def g(x):\n    if x:\n        return 'a'\n    return name\n";
        let at = t2.find("name").unwrap();
        assert_eq!(hint_target(&tree(t2), t2, 4), Some(HintTarget::Return { name: "g".into(), at: 4, insert: 8, returns: vec![Returned::Known("str".into()), Returned::Ask(at, at + 3)] }));
        assert_eq!(hint_target(&tr, t, t.find("x =").unwrap()), Some(HintTarget::Variable { name: "x".into(), at: t.find("x =").unwrap(), insert: t.find("x =").unwrap() + 1 }));
        assert_eq!(hint_target(&tr, t, t.find("y:").unwrap()), None);
    }
}
