//! Python name resolution within one file, from the syntax tree alone (no type checker):
//! which identifiers refer to the same variable as the one at a position. Scopes are
//! functions, lambdas and classes; a name bound in a function (parameter, assignment, `for`
//! target, `with ... as`, `except ... as`, walrus, nested `def`/`class`) is local to it unless
//! declared `global`/`nonlocal`. Nested scopes that bind the same name shadow it.
//! Comprehension variables count as bindings of the enclosing scope (an approximation).

use crate::{identifier_at_of, Lang, SyntaxTree};
use std::ops::Range;
use tree_sitter::{Node, Tree};

/// The occurrences of the name at a position.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct NameRefs {
    pub name: String,
    /// Byte ranges, sorted.
    pub ranges: Vec<Range<usize>>,
    /// Bound in a function or lambda (a local variable or parameter), not module- or class-level.
    pub local: bool,
}

pub(crate) fn is_scope(k: &str) -> bool {
    matches!(k, "function_definition" | "lambda" | "class_definition")
}

fn same(a: Option<Node>, b: Node) -> bool {
    a.is_some_and(|a| a.id() == b.id())
}

/// The identifier is the attribute part of `x.attr` or the name in `f(name=...)`: not a variable.
pub(crate) fn is_member(n: Node) -> bool {
    let Some(p) = n.parent() else { return false };
    match p.kind() {
        "attribute" => same(p.child_by_field_name("attribute"), n),
        "keyword_argument" => same(p.child_by_field_name("name"), n),
        _ => false,
    }
}

/// The identifier is assigned (bound) here rather than read.
pub(crate) fn is_binding(n: Node) -> bool {
    let mut cur = n;
    while let Some(p) = cur.parent() {
        match p.kind() {
            "pattern_list" | "tuple_pattern" | "list_pattern" | "list_splat_pattern" | "dictionary_splat_pattern" | "tuple" | "list"
            | "parenthesized_expression" | "as_pattern_target" | "expression_list" => cur = p,
            "parameters" | "lambda_parameters" => return true,
            "typed_parameter" => return same(p.named_child(0), cur),
            "default_parameter" | "typed_default_parameter" => return same(p.child_by_field_name("name"), cur),
            "assignment" | "augmented_assignment" | "for_statement" | "for_in_clause" => return same(p.child_by_field_name("left"), cur),
            "named_expression" => return same(p.child_by_field_name("name"), cur),
            "function_definition" | "class_definition" => return same(p.child_by_field_name("name"), cur),
            "aliased_import" => return same(p.child_by_field_name("alias"), cur),
            "as_pattern" => return same(p.child_by_field_name("alias"), cur),
            _ => return false,
        }
    }
    false
}

/// Calls `f` for every identifier that belongs to `scope` itself and `nested` for every
/// directly nested scope (whose own name still belongs to `scope`).
fn walk<'t>(scope: Node<'t>, f: &mut dyn FnMut(Node<'t>), nested: &mut dyn FnMut(Node<'t>)) {
    let mut stack: Vec<Node> = Vec::new();
    let mut c = scope.walk();
    if scope.parent().is_none() || !is_scope(scope.kind()) {
        stack.extend(scope.children(&mut c));
    } else {
        // The scope's own name belongs to the enclosing scope.
        stack.extend(scope.children(&mut c).filter(|k| !same(scope.child_by_field_name("name"), *k)));
    }
    while let Some(n) = stack.pop() {
        if is_scope(n.kind()) {
            if let Some(name) = n.child_by_field_name("name") {
                f(name);
            }
            nested(n);
            continue;
        }
        if n.kind() == "identifier" {
            f(n);
        }
        let mut c = n.walk();
        stack.extend(n.children(&mut c));
    }
}

/// Whether `scope` binds `name` itself (`Some(true)`), or declares it `global`/`nonlocal`
/// (`Some(false)`: it belongs elsewhere), or neither (`None`).
fn binds(scope: Node, text: &str, name: &str) -> Option<bool> {
    let mut bound = false;
    let mut declared = false;
    walk(
        scope,
        &mut |n| {
            if &text[n.byte_range()] == name {
                let p = n.parent().map(|p| p.kind());
                if matches!(p, Some("global_statement" | "nonlocal_statement")) {
                    declared = true;
                } else if is_binding(n) {
                    bound = true;
                }
            }
        },
        &mut |_| {},
    );
    if declared {
        Some(false)
    } else {
        bound.then_some(true)
    }
}

/// Occurrences in `scope` and in the nested scopes that see it. `own`: count the scope's
/// own identifiers (false for a class that binds the name: only its methods may still see
/// the outer one).
fn collect(scope: Node, text: &str, name: &str, own: bool, out: &mut Vec<Range<usize>>) {
    let mut inner = Vec::new();
    walk(
        scope,
        &mut |n| {
            if own && &text[n.byte_range()] == name && !is_member(n) {
                out.push(n.byte_range());
            }
        },
        &mut |s| inner.push(s),
    );
    // A class body is not visible from the functions in it.
    if own && scope.kind() == "class_definition" {
        inner.retain(|s| s.kind() == "class_definition");
    }
    for s in inner {
        match binds(s, text, name) {
            Some(true) if s.kind() == "class_definition" => collect(s, text, name, false, out),
            Some(true) => {}
            _ => collect(s, text, name, true, out),
        }
    }
}

impl SyntaxTree {
    /// Occurrences of the variable at `byte` (Python only). None when there is no variable
    /// there (an attribute, a keyword argument name, a keyword, a string).
    pub fn name_refs(&self, text: &str, byte: usize) -> Option<NameRefs> {
        name_refs_of(self.lang, self.tree.as_ref()?, text, byte)
    }
}

/// [`SyntaxTree::name_refs`] on a copy of the tree (for a background thread).
pub fn name_refs_of(lang: Lang, tree: &Tree, text: &str, byte: usize) -> Option<NameRefs> {
    if lang != Lang::Python {
        return None;
    }
    let mut leaf = tree.root_node().descendant_for_byte_range(byte, byte)?;
    if leaf.kind() != "identifier" && byte > 0 {
        leaf = tree.root_node().descendant_for_byte_range(byte - 1, byte - 1)?;
    }
    if leaf.kind() != "identifier" || is_member(leaf) {
        return None;
    }
    let name = text.get(leaf.byte_range())?.to_string();
    // The scopes around the identifier, innermost first; a def's own name belongs outside.
    let mut scopes = Vec::new();
    let mut n = leaf;
    while let Some(p) = n.parent() {
        if is_scope(p.kind()) && !same(p.child_by_field_name("name"), n) {
            scopes.push(p);
        }
        n = p;
    }
    let mut global = false;
    let mut found = None;
    for (i, s) in scopes.iter().enumerate() {
        // Class bodies are invisible from nested functions: only the innermost may be one.
        if s.kind() == "class_definition" && i > 0 {
            continue;
        }
        match binds(*s, text, &name) {
            Some(true) if !global => {
                found = Some(*s);
                break;
            }
            Some(false) => global = true,
            _ => {}
        }
    }
    let scope = found.unwrap_or_else(|| tree.root_node());
    let local = found.is_some_and(|s| s.kind() != "class_definition");
    let mut ranges = Vec::new();
    collect(scope, text, &name, true, &mut ranges);
    ranges.sort_by_key(|r| r.start);
    ranges.dedup();
    Some(NameRefs { name, ranges, local })
}

/// Byte ranges of the usages of the name under `byte` (also when `byte` is right after it):
/// scope-aware in Python, otherwise every match of the identifier outside strings and
/// comments, searched only in texts up to `limit` bytes.
pub fn occurrences_of(lang: Lang, tree: &Tree, text: &str, byte: usize, limit: usize) -> Vec<Range<usize>> {
    if let Some(refs) = name_refs_of(lang, tree, text, byte) {
        return refs.ranges;
    }
    let word = identifier_at_of(tree, byte).or_else(|| byte.checked_sub(1).and_then(|p| identifier_at_of(tree, p)));
    let Some(word) = word else { return Vec::new() };
    let name = &text[word];
    if name.is_empty() || text.len() > limit {
        return Vec::new();
    }
    let is_word = |c: char| c.is_alphanumeric() || c == '_';
    let mut out = Vec::new();
    let mut from = 0;
    while let Some(i) = text[from..].find(name) {
        let (st, en) = (from + i, from + i + name.len());
        from = en;
        let left_ok = !text[..st].chars().next_back().is_some_and(is_word);
        let right_ok = !text[en..].chars().next().is_some_and(is_word);
        if left_ok && right_ok && identifier_at_of(tree, st).is_some_and(|r| r == (st..en)) {
            out.push(st..en);
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    fn refs(src: &str, at: &str, nth: usize) -> Option<(Vec<usize>, bool)> {
        let mut t = SyntaxTree::new(Lang::Python);
        t.parse(src);
        let byte = src.match_indices(at).nth(nth).unwrap().0;
        t.name_refs(src, byte).map(|r| {
            let lines = r.ranges.iter().map(|x| src[..x.start].matches('\n').count()).collect();
            (lines, r.local)
        })
    }

    const SRC: &str = "\
total = 0
def add(items, total=1):
    for x in items:
        total += x
    obj.total = total
    f(total=total)
    def inner():
        return total
    return total
def other():
    return total
class C:
    total = 5
    def m(self):
        return total
";

    #[test]
    fn local_parameter_and_closure() {
        // `total` in add: the parameter, its uses, the closure in inner; not the attribute,
        // not the keyword name, not the module-level one.
        let (lines, local) = refs(SRC, "total", 1).unwrap();
        assert!(local);
        assert_eq!(lines, vec![1, 3, 4, 5, 7, 8]);
        // From a use inside the nested function: the same set.
        assert_eq!(refs(SRC, "total", 8).unwrap().0, vec![1, 3, 4, 5, 7, 8]);
    }

    #[test]
    fn module_level_skips_shadowing_scopes() {
        // Module `total`: line 0, other(), and the method (class bodies are not visible from
        // methods); not add (parameter), not the class attribute.
        let (lines, local) = refs(SRC, "total", 0).unwrap();
        assert!(!local);
        assert_eq!(lines, vec![0, 10, 14]);
        // The class attribute: only the class body, not the method.
        assert_eq!(refs(SRC, "total = 5", 0).unwrap(), (vec![12], false));
    }

    #[test]
    fn global_declaration_and_members() {
        let src = "n = 0\ndef inc():\n    global n\n    n += 1\nx.n = 2\n";
        let (lines, local) = refs(src, "n", 0).unwrap();
        assert!(!local);
        assert_eq!(lines, vec![0, 2, 3]);
        // The attribute part is not a variable.
        assert!(refs(src, "n = 2", 0).is_none());
        // From inside the function: the same, global variable.
        assert_eq!(refs(src, "n +=", 0).unwrap(), (vec![0, 2, 3], false));
    }

    #[test]
    fn def_name_belongs_to_outer_scope() {
        let src = "def helper(a):\n    return a\nhelper(1)\n";
        let (lines, local) = refs(src, "helper", 0).unwrap();
        assert!(!local);
        assert_eq!(lines, vec![0, 2]);
        assert!(refs(src, "a)", 0).unwrap().1);
    }
}
