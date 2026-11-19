//! Python refactorings within one file, from the syntax tree: the expressions to pick for
//! Extract Variable / Constant, where a constant goes, Extract Method and Inline. Results are
//! byte-range edits on the text; the caller applies them as one undo step.

use crate::scope::{is_binding, is_member, is_scope};
use std::ops::Range;
use tree_sitter::{Node, Tree};

/// A byte-range edit.
pub type Edit = (Range<usize>, String);

const EXPRS: &[&str] = &[
    "identifier", "attribute", "call", "subscript", "binary_operator", "boolean_operator", "comparison_operator", "unary_operator", "not_operator",
    "conditional_expression", "string", "concatenated_string", "integer", "float", "true", "false", "none", "list", "tuple", "dictionary", "set",
    "list_comprehension", "dictionary_comprehension", "set_comprehension", "generator_expression", "parenthesized_expression", "await", "lambda",
];

/// Kinds that need no parentheses wherever an expression goes.
const ATOMS: &[&str] = &[
    "identifier", "attribute", "call", "subscript", "string", "concatenated_string", "integer", "float", "true", "false", "none", "list", "dictionary", "set",
    "list_comprehension", "dictionary_comprehension", "set_comprehension", "generator_expression", "parenthesized_expression",
];

fn src<'a>(n: Node, text: &'a str) -> &'a str {
    text.get(n.byte_range()).unwrap_or("")
}

fn is_statement(k: &str) -> bool {
    k.ends_with("_statement") || matches!(k, "block" | "module" | "decorated_definition" | "function_definition" | "class_definition")
}

fn leaf_at(tree: &Tree, byte: usize) -> Option<Node<'_>> {
    let n = tree.root_node().descendant_for_byte_range(byte, byte)?;
    if (n.kind() == "identifier" || EXPRS.contains(&n.kind())) || byte == 0 {
        return Some(n);
    }
    // Right after a word the caret is past its node.
    tree.root_node().descendant_for_byte_range(byte - 1, byte - 1)
}

/// Extract Variable / Constant without a selection: the expressions around `byte`, innermost
/// first, up to the statement (PyCharm's "Expressions" choice). Assignment targets, attribute
/// names and keyword names are not expressions.
pub fn expressions_at(tree: &Tree, text: &str, byte: usize) -> Vec<Range<usize>> {
    let mut out: Vec<Range<usize>> = Vec::new();
    let mut n = leaf_at(tree, byte);
    while let Some(cur) = n {
        if is_statement(cur.kind()) || matches!(cur.kind(), "assignment" | "augmented_assignment") && !out.is_empty() {
            break;
        }
        let skip = cur.kind() == "identifier" && (is_member(cur) || is_binding(cur));
        if EXPRS.contains(&cur.kind()) && !skip && out.last() != Some(&cur.byte_range()) && !src(cur, text).trim().is_empty() {
            out.push(cur.byte_range());
        }
        n = cur.parent();
    }
    out
}

/// Extract Constant: the row a new module-level constant goes before (after the docstring,
/// `__future__` and the other imports at the top of the module), and whether a blank line
/// must separate it from what is above.
pub fn constant_row(tree: &Tree) -> (usize, bool) {
    let root = tree.root_node();
    let mut after = None;
    for i in 0..root.named_child_count() {
        let Some(c) = root.named_child(i as _) else { continue };
        let docstring = i == 0 && c.kind() == "expression_statement" && c.named_child(0).is_some_and(|s| s.kind() == "string");
        if docstring || matches!(c.kind(), "import_statement" | "import_from_statement" | "future_import_statement") {
            after = Some(c.end_position().row);
        } else if c.kind() != "comment" {
            break;
        }
    }
    match after {
        Some(r) => (r + 1, true),
        None => (0, false),
    }
}

/// The identifiers of `n` in source order with whether each binds, not entering nested
/// functions, lambdas or classes (their own name counts as a binding here).
fn names<'t>(n: Node<'t>, out: &mut Vec<Node<'t>>) {
    let mut c = n.walk();
    for k in n.children(&mut c) {
        if is_scope(k.kind()) {
            if let Some(name) = k.child_by_field_name("name") {
                out.push(name);
            }
            // Defaults and decorators run here, in the enclosing scope.
            if let Some(p) = k.child_by_field_name("parameters") {
                let mut pc = p.walk();
                for d in p.children(&mut pc) {
                    if let Some(v) = d.child_by_field_name("value") {
                        names(v, out);
                    }
                }
            }
            continue;
        }
        if k.kind() == "identifier" {
            if !is_member(k) {
                out.push(k);
            }
            continue;
        }
        names(k, out);
    }
}

fn binding_of(n: Node) -> bool {
    is_binding(n) || n.parent().is_some_and(|p| p.kind() == "function_definition" || p.kind() == "class_definition")
}

/// The function or module that `n` belongs to.
fn scope_of(n: Node) -> Node {
    let mut cur = n;
    while let Some(p) = cur.parent() {
        if p.kind() == "function_definition" {
            return p;
        }
        cur = p;
    }
    cur
}

fn rows(n: &Node) -> Range<usize> {
    n.start_position().row..n.end_position().row + 1
}

fn indent_of(line: &str) -> &str {
    &line[..line.len() - line.trim_start().len()]
}

/// What Extract Method changes, by rows: `rows` become `call` (one line, indented); `def`
/// (indented, lines) goes before row `def_row`, separated by `blank` empty lines.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct MethodPlan {
    pub rows: Range<usize>,
    /// When the selection is one expression: its byte range, replaced by the call instead.
    pub expr: Option<Range<usize>>,
    pub call: String,
    pub def: Vec<String>,
    pub def_row: usize,
    pub blank: usize,
    pub params: Vec<String>,
    pub returns: Vec<String>,
}

/// Inside `n` (not in nested scopes) a node of one of `kinds`; loops stop break/continue.
fn contains(n: Node, kinds: &[&str], loops_stop: bool) -> bool {
    let mut c = n.walk();
    for k in n.children(&mut c) {
        if is_scope(k.kind()) || loops_stop && matches!(k.kind(), "for_statement" | "while_statement") {
            continue;
        }
        if kinds.contains(&k.kind()) || contains(k, kinds, loops_stop) {
            return true;
        }
    }
    false
}

/// Extract Method on the bytes `sel` (whole statements, or one expression) as `name`.
/// Parameters are the variables read in the selection that the enclosing function set before
/// it; the return value is the variables it sets that are read after it. In a method that
/// uses `self` the result is a method.
pub fn extract_method(tree: &Tree, text: &str, sel: Range<usize>, name: &str) -> Result<MethodPlan, String> {
    let root = tree.root_node();
    let lines: Vec<&str> = text.split('\n').collect();
    // One expression, or the statements of one block covering the selected lines.
    let expr = root.descendant_for_byte_range(sel.start, sel.end).filter(|n| n.byte_range() == sel && EXPRS.contains(&n.kind()) && !is_statement(n.kind()));
    let (stmts, rows_r): (Vec<Node>, Range<usize>) = match expr {
        Some(e) => (vec![e], rows(&e)),
        None => {
            let first = text[..sel.start].matches('\n').count();
            let last = text[..sel.end.max(sel.start + 1).min(text.len())].trim_end_matches('\n').matches('\n').count();
            let (r, _) = crate::statement_move(tree, first, last.max(first), true).ok_or("Select whole statements or an expression")?;
            let mut v = Vec::new();
            collect_statements(root, &r, &mut v);
            if v.is_empty() {
                return Err("Select whole statements or an expression".into());
            }
            (v, r)
        }
    };
    let (start, end) = (stmts[0].start_byte(), stmts[stmts.len() - 1].end_byte());
    let scope = scope_of(stmts[0]);
    if expr.is_none() {
        if stmts.iter().any(|s| contains(*s, &["yield"], false)) {
            return Err("The selection contains 'yield'".into());
        }
        if stmts.iter().any(|s| contains(*s, &["break_statement", "continue_statement"], true) || matches!(s.kind(), "break_statement" | "continue_statement")) {
            return Err("The selection contains 'break' or 'continue' outside a loop".into());
        }
    }
    let returns_in = stmts.iter().filter(|s| s.kind() == "return_statement" || contains(**s, &["return_statement"], false)).count();
    let ends_with_return = stmts.last().is_some_and(|s| s.kind() == "return_statement");
    if returns_in > 1 || returns_in == 1 && !ends_with_return {
        return Err("The selection returns from the middle of the function".into());
    }
    let is_async = stmts.iter().any(|s| s.kind() == "await" || contains(*s, &["await"], false));
    // Names: set before the selection in this scope, used inside, used after.
    let mut all = Vec::new();
    names(if scope.kind() == "function_definition" { scope } else { root }, &mut all);
    if scope.kind() == "function_definition" {
        if let Some(p) = scope.child_by_field_name("parameters") {
            let mut ps = Vec::new();
            names(p, &mut ps);
            all.splice(0..0, ps);
        }
    }
    let name_of = |n: &Node| src(*n, text).to_string();
    let before: std::collections::HashSet<String> = all.iter().filter(|n| n.start_byte() < start && binding_of(**n)).map(name_of).collect();
    let mut inside = Vec::new();
    for s in &stmts {
        if s.kind() == "identifier" {
            inside.push(*s);
        } else {
            names(*s, &mut inside);
        }
    }
    let mut params: Vec<String> = Vec::new();
    let mut bound_inside: Vec<String> = Vec::new();
    for n in &inside {
        let s = name_of(n);
        if binding_of(*n) {
            if !bound_inside.contains(&s) {
                bound_inside.push(s);
            }
        } else if !bound_inside.contains(&s) && !params.contains(&s) && (scope.kind() == "function_definition" && before.contains(&s)) {
            params.push(s);
        }
    }
    let after: std::collections::HashSet<String> = all.iter().filter(|n| n.start_byte() >= end && !binding_of(**n)).map(name_of).collect();
    let returns: Vec<String> = if expr.is_some() { Vec::new() } else { bound_inside.iter().filter(|s| after.contains(*s)).cloned().collect() };
    if ends_with_return && !returns.is_empty() {
        return Err("The selection both returns and sets variables used after it".into());
    }
    // A method using `self` (or `cls`) stays a method.
    let class_method = scope.kind() == "function_definition" && scope.parent().is_some_and(|b| b.kind() == "block" && b.parent().is_some_and(|c| c.kind() == "class_definition"));
    let receiver = class_method.then(|| params.first().cloned()).flatten().filter(|p| p == "self" || p == "cls");
    if let Some(r) = &receiver {
        params.retain(|p| p != r);
    }
    let mut sig = receiver.iter().cloned().collect::<Vec<_>>();
    sig.extend(params.iter().cloned());
    let here = lines.get(rows_r.start).map(|l| indent_of(l)).unwrap_or("").to_string();
    let (def_indent, def_row, blank) = if scope.kind() == "function_definition" {
        // After the enclosing function (or method).
        let outer = if scope.parent().is_some_and(|p| p.kind() == "decorated_definition") { scope.parent().unwrap() } else { scope };
        let ind = lines.get(outer.start_position().row).map(|l| indent_of(l)).unwrap_or("").to_string();
        let b = if ind.is_empty() { 2 } else { 1 };
        (ind, outer.end_position().row + 1, b)
    } else {
        // Module code: the function must exist before the call runs.
        (String::new(), top_level_row(root, start), 2)
    };
    let mut def = vec![format!("{def_indent}{}def {name}({}):", if is_async { "async " } else { "" }, sig.join(", "))];
    match expr {
        Some(e) => def.push(format!("{def_indent}    return {}", src(e, text))),
        None => {
            let own = here.len();
            for l in &lines[rows_r.clone()] {
                if l.trim().is_empty() {
                    def.push(String::new());
                } else {
                    let ws = indent_of(l).len().min(own);
                    def.push(format!("{def_indent}    {}", &l[ws..]));
                }
            }
            match returns.len() {
                0 => {}
                _ => def.push(format!("{def_indent}    return {}", returns.join(", "))),
            }
        }
    }
    let callee = match &receiver {
        Some(r) => format!("{r}.{name}"),
        None => name.to_string(),
    };
    let call = format!("{}{callee}({})", if is_async { "await " } else { "" }, params.join(", "));
    let call = match (expr, ends_with_return, returns.len()) {
        (Some(_), _, _) => call,
        (None, true, _) => format!("{here}return {call}"),
        (None, false, 0) => format!("{here}{call}"),
        (None, false, _) => format!("{here}{} = {call}", returns.join(", ")),
    };
    Ok(MethodPlan { rows: rows_r, expr: expr.map(|e| e.byte_range()), call, def, def_row, blank, params, returns })
}

/// The statements of the block (or module) whose children exactly cover `r`.
fn collect_statements<'t>(n: Node<'t>, r: &Range<usize>, out: &mut Vec<Node<'t>>) {
    let kids: Vec<Node> = (0..n.named_child_count()).filter_map(|i| n.named_child(i as _)).collect();
    let hit: Vec<Node> = kids.iter().filter(|k| rows(k).start >= r.start && rows(k).end <= r.end).cloned().collect();
    if !hit.is_empty() && rows(&hit[0]).start == r.start && rows(hit.last().unwrap()).end == r.end && matches!(n.kind(), "module" | "block") {
        out.extend(hit.into_iter().filter(|k| k.kind() != "comment"));
        return;
    }
    for k in kids {
        if rows(&k).start <= r.start && rows(&k).end >= r.end {
            collect_statements(k, r, out);
            if !out.is_empty() {
                return;
            }
        }
    }
}

/// The row of the top-level statement holding `byte`.
fn top_level_row(root: Node, byte: usize) -> usize {
    (0..root.named_child_count())
        .filter_map(|i| root.named_child(i as _))
        .find(|c| c.start_byte() <= byte && byte <= c.end_byte())
        .map(|c| c.start_position().row)
        .unwrap_or(0)
}

/// Parentheses for `value` (a node's text) where `at` stands.
fn wrap(value: &str, kind: &str, at: Node) -> String {
    let free = at.parent().is_some_and(|p| matches!(p.kind(), "argument_list" | "expression_statement" | "return_statement" | "keyword_argument" | "list" | "set" | "pair" | "assignment"));
    if ATOMS.contains(&kind) || free { value.to_string() } else { format!("({value})") }
}

/// Inline (Ctrl+Alt+N) at `byte`.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Inline {
    /// A variable set once: its uses get the value and the assignment goes.
    Variable { name: String, edits: Vec<Edit>, uses: usize },
    /// A function whose body is one `return`: every call (and the `def`), or only `this`.
    Function { name: String, all: Option<Vec<Edit>>, this: Option<Vec<Edit>>, calls: usize },
}

pub fn inline_at(tree: &Tree, text: &str, byte: usize) -> Result<Inline, String> {
    let leaf = leaf_at(tree, byte).filter(|n| n.kind() == "identifier").ok_or("Put the caret on a variable or function name")?;
    let refs = crate::name_refs_of(crate::Lang::Python, tree, text, leaf.start_byte()).ok_or("Put the caret on a variable or function name")?;
    let root = tree.root_node();
    let nodes: Vec<Node> = refs.ranges.iter().filter_map(|r| root.descendant_for_byte_range(r.start, r.end)).collect();
    // A function: one of the bindings is a `def`.
    if let Some(def) = nodes.iter().find_map(|n| n.parent().filter(|p| p.kind() == "function_definition" && p.child_by_field_name("name").is_some_and(|x| x.id() == n.id()))) {
        return inline_function(def, &nodes, leaf, text, refs.name);
    }
    let binds: Vec<&Node> = nodes.iter().filter(|n| binding_of(**n)).collect();
    let [b] = binds[..] else { return Err(format!("Variable '{}' is assigned more than once", refs.name)) };
    let assign = b.parent().filter(|p| p.kind() == "assignment" && p.child_by_field_name("left").is_some_and(|l| l.id() == b.id())).ok_or("Only a variable set by a simple assignment can be inlined")?;
    let value = assign.child_by_field_name("right").ok_or("The variable has no value")?;
    let stmt = assign.parent().filter(|s| s.kind() == "expression_statement").ok_or("Only a variable set by a simple assignment can be inlined")?;
    let uses: Vec<&Node> = nodes.iter().filter(|n| !binding_of(**n)).collect();
    if uses.is_empty() {
        return Err(format!("Variable '{}' is never used", refs.name));
    }
    if uses.iter().any(|u| u.start_byte() < stmt.start_byte()) {
        return Err(format!("Variable '{}' is used before it is set", refs.name));
    }
    let v = src(value, text);
    let mut edits: Vec<Edit> = uses.iter().map(|u| (u.byte_range(), wrap(v, value.kind(), **u))).collect();
    edits.push((line_span(text, stmt.byte_range()), String::new()));
    Ok(Inline::Variable { name: refs.name, edits, uses: uses.len() })
}

/// Whole lines of `r` with the line break after them.
fn line_span(text: &str, r: Range<usize>) -> Range<usize> {
    let s = text[..r.start].rfind('\n').map(|n| n + 1).unwrap_or(0);
    let e = text[r.end..].find('\n').map(|n| r.end + n + 1).unwrap_or(text.len());
    s..e
}

fn inline_function(def: Node, nodes: &[Node], at: Node, text: &str, name: String) -> Result<Inline, String> {
    if def.parent().is_some_and(|p| p.kind() == "decorated_definition") {
        return Err("A decorated function cannot be inlined".into());
    }
    let body = def.child_by_field_name("body").ok_or("No body")?;
    let stmts: Vec<Node> = (0..body.named_child_count()).filter_map(|i| body.named_child(i as _)).filter(|s| s.kind() != "comment").collect();
    let docstring = |s: &Node| s.kind() == "expression_statement" && s.named_child(0).is_some_and(|x| x.kind() == "string");
    let ret = match &stmts[..] {
        [r] => *r,
        [d, r] if docstring(d) => *r,
        _ => return Err(format!("Function '{name}' has more than a single 'return'")),
    };
    let value = (ret.kind() == "return_statement").then(|| ret.named_child(0)).flatten().ok_or(format!("Function '{name}' has more than a single 'return'"))?;
    // Parameters: plain names with optional defaults.
    let mut params: Vec<(String, Option<String>)> = Vec::new();
    if let Some(ps) = def.child_by_field_name("parameters") {
        for i in 0..ps.named_child_count() {
            let Some(p) = ps.named_child(i as _) else { continue };
            match p.kind() {
                "identifier" => params.push((src(p, text).into(), None)),
                "typed_parameter" => params.push((p.named_child(0).map(|x| src(x, text)).unwrap_or("").into(), None)),
                "default_parameter" | "typed_default_parameter" => {
                    params.push((p.child_by_field_name("name").map(|x| src(x, text)).unwrap_or("").into(), p.child_by_field_name("value").map(|x| src(x, text).to_string())))
                }
                _ => return Err(format!("Function '{name}' takes *args, **kwargs or special parameters")),
            }
        }
    }
    // Identifiers of the returned value that name a parameter.
    let mut ids = Vec::new();
    names(value, &mut ids);
    if value.kind() == "identifier" {
        ids.push(value);
    }
    let substitute = |call: Node| -> Option<String> {
        let args = call.child_by_field_name("arguments")?;
        let mut given: Vec<Option<(String, &str)>> = vec![None; params.len()];
        let mut pos = 0;
        for i in 0..args.named_child_count() {
            let a = args.named_child(i as _)?;
            match a.kind() {
                "keyword_argument" => {
                    let k = src(a.child_by_field_name("name")?, text);
                    let v = a.child_by_field_name("value")?;
                    let idx = params.iter().position(|(p, _)| p == k)?;
                    given[idx] = Some((src(v, text).into(), v.kind()));
                }
                "list_splat" | "dictionary_splat" => return None,
                "comment" => {}
                _ => {
                    *given.get_mut(pos)? = Some((src(a, text).into(), a.kind()));
                    pos += 1;
                }
            }
        }
        let mut out = src(value, text).to_string();
        let base = value.start_byte();
        let mut reps: Vec<(Range<usize>, String)> = Vec::new();
        for id in &ids {
            let Some(i) = params.iter().position(|(p, _)| p == src(*id, text)) else { continue };
            let (v, kind) = match &given[i] {
                Some((v, k)) => (v.clone(), *k),
                None => (params[i].1.clone()?, "parenthesized_expression"),
            };
            reps.push((id.start_byte() - base..id.end_byte() - base, if ATOMS.contains(&kind) { v } else { format!("({v})") }));
        }
        reps.sort_by_key(|r| std::cmp::Reverse(r.0.start));
        for (r, v) in reps {
            out.replace_range(r, &v);
        }
        Some(wrap(&out, value.kind(), call))
    };
    fn call_of<'t>(n: &Node<'t>) -> Option<Node<'t>> {
        n.parent().filter(|p| p.kind() == "call" && p.child_by_field_name("function").is_some_and(|f| f.id() == n.id()))
    }
    let uses: Vec<&Node> = nodes.iter().filter(|n| n.id() != def.child_by_field_name("name").map(|x| x.id()).unwrap_or(0)).collect();
    let calls: Vec<Node> = uses.iter().filter_map(|n| call_of(n)).collect();
    let this = call_of(&at).and_then(|c| Some(vec![(c.byte_range(), substitute(c)?)]));
    let all = (calls.len() == uses.len() && !calls.is_empty())
        .then(|| {
            let mut e: Vec<Edit> = calls.iter().map(|c| Some((c.byte_range(), substitute(*c)?))).collect::<Option<Vec<_>>>()?;
            // The `def` and the blank lines after it.
            let span = line_span(text, def.byte_range());
            let tail = text[span.end..].len() - text[span.end..].trim_start_matches('\n').len();
            e.push((span.start..span.end + tail, String::new()));
            Some(e)
        })
        .flatten();
    if all.is_none() && this.is_none() {
        return Err(format!("Function '{name}' is used other than in plain calls"));
    }
    Ok(Inline::Function { name, all, this, calls: calls.len() })
}

#[cfg(test)]
mod tests {
    use super::*;

    fn tree(s: &str) -> Tree {
        crate::parse_full(crate::Lang::Python, s, None).unwrap()
    }

    fn apply(text: &str, mut edits: Vec<Edit>) -> String {
        edits.sort_by_key(|e| std::cmp::Reverse(e.0.start));
        let mut s = text.to_string();
        for (r, v) in edits {
            s.replace_range(r, &v);
        }
        s
    }

    #[test]
    fn expressions_around_the_caret() {
        let s = "total = price(a + b) * 2\n";
        let e: Vec<&str> = expressions_at(&tree(s), s, s.find("a +").unwrap()).into_iter().map(|r| &s[r]).collect();
        assert_eq!(e, ["a", "a + b", "price(a + b)", "price(a + b) * 2"]);
        let s = "import os\nimport sys\n\nX = 1\n";
        assert_eq!(constant_row(&tree(s)), (2, true));
    }

    #[test]
    fn extract_method_params_and_returns() {
        let s = "def f(a, b):\n    c = a + 1\n    d = c * b\n    print(d)\n    return d\n";
        let sel = s.find("c = a").unwrap()..s.find("    print").unwrap();
        let p = extract_method(&tree(s), s, sel, "calc").unwrap();
        assert_eq!((p.params.clone(), p.returns.clone(), p.rows.clone()), (vec!["a".to_string(), "b".to_string()], vec!["d".to_string()], 1..3));
        assert_eq!(p.call, "    d = calc(a, b)");
        assert_eq!(p.def, ["def calc(a, b):", "    c = a + 1", "    d = c * b", "    return d"]);
        assert_eq!((p.def_row, p.blank), (5, 2));
        // In a method that uses self: a method, called through self.
        let s = "class A:\n    def f(self, x):\n        y = self.k + x\n        return y\n";
        let sel = s.find("self.k + x").unwrap()..s.find(" + x").unwrap() + 4;
        let p = extract_method(&tree(s), s, sel, "part").unwrap();
        assert_eq!((p.call.as_str(), p.def[0].as_str(), p.def[1].as_str()), ("self.part(x)", "    def part(self, x):", "        return self.k + x"));
    }

    #[test]
    fn inline_variable_and_function() {
        let s = "def f(a):\n    t = a + 1\n    return t * t\n";
        let Ok(Inline::Variable { edits, uses, .. }) = inline_at(&tree(s), s, s.find("t =").unwrap()) else { panic!() };
        assert_eq!((uses, apply(s, edits).as_str()), (2, "def f(a):\n    return (a + 1) * (a + 1)\n"));
        let s = "def sq(x, k=2):\n    return x ** k\n\n\nprint(sq(a + 1), sq(3, k=4))\n";
        let Ok(Inline::Function { all: Some(all), calls, .. }) = inline_at(&tree(s), s, 4) else { panic!() };
        assert_eq!((calls, apply(s, all).as_str()), (2, "print((a + 1) ** 2, 3 ** 4)\n"));
    }
}
