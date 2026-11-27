//! JavaScript and TypeScript scopes for Find Usages and Rename without the TypeScript server:
//! the occurrences of a variable, parameter, function or class in the function or block that
//! declares it (`let` / `const` / `var`, parameters, `function` / `class` names, imports,
//! `catch (e)`, `for (const x of …)`), skipping nested scopes that declare the same name.

use crate::scope::NameRefs;
use tree_sitter::{Node, Tree};

/// Identifier kinds that name a variable (not `obj.prop` or a type name).
fn is_name(k: &str) -> bool {
    matches!(k, "identifier" | "shorthand_property_identifier" | "shorthand_property_identifier_pattern")
}

fn is_function(k: &str) -> bool {
    matches!(
        k,
        "function_declaration"
            | "function_expression"
            | "function"
            | "arrow_function"
            | "method_definition"
            | "generator_function_declaration"
            | "generator_function"
    )
}

fn is_scope(k: &str) -> bool {
    is_function(k) || matches!(k, "statement_block" | "program" | "for_statement" | "for_in_statement" | "catch_clause" | "class_body")
}

/// Whether a pattern (a parameter, a declarator's name) binds `name`.
fn pattern_binds(n: Node, text: &str, name: &str) -> bool {
    if is_name(n.kind()) {
        return text.get(n.byte_range()) == Some(name);
    }
    match n.kind() {
        // `a = 1`: the left side; `key: value` in a pattern: the value.
        "assignment_pattern" | "object_assignment_pattern" => n.child_by_field_name("left").is_some_and(|l| pattern_binds(l, text, name)),
        "pair_pattern" => n.child_by_field_name("value").is_some_and(|v| pattern_binds(v, text, name)),
        "required_parameter" | "optional_parameter" => n.child_by_field_name("pattern").is_some_and(|p| pattern_binds(p, text, name)),
        "array_pattern" | "object_pattern" | "rest_pattern" | "formal_parameters" => {
            let mut c = n.walk();
            let found = n.named_children(&mut c).any(|ch| pattern_binds(ch, text, name));
            found
        }
        _ => false,
    }
}

/// Whether a declaration statement directly in a block binds `name`.
fn statement_binds(s: Node, text: &str, name: &str) -> bool {
    match s.kind() {
        "lexical_declaration" | "variable_declaration" => {
            let mut c = s.walk();
            let found = s
                .named_children(&mut c)
                .filter(|d| d.kind() == "variable_declarator")
                .any(|d| d.child_by_field_name("name").is_some_and(|p| pattern_binds(p, text, name)));
            found
        }
        "function_declaration" | "generator_function_declaration" | "class_declaration" | "abstract_class_declaration" => {
            s.child_by_field_name("name").is_some_and(|n| text.get(n.byte_range()) == Some(name))
        }
        "export_statement" => s.child_by_field_name("declaration").is_some_and(|d| statement_binds(d, text, name)),
        "import_statement" => {
            // `import a, { b, c as d }, * as e`: the local names.
            let mut out = false;
            walk(s, &mut |n| {
                if n.kind() == "identifier" && text.get(n.byte_range()) == Some(name) {
                    let p = n.parent();
                    let alias = p.filter(|p| p.kind() == "import_specifier").and_then(|p| p.child_by_field_name("alias"));
                    // In `b as d` only `d` is local.
                    if alias.is_none_or(|a| a.id() == n.id()) {
                        out = true;
                    }
                }
            });
            out
        }
        _ => false,
    }
}

fn walk<'a>(n: Node<'a>, f: &mut impl FnMut(Node<'a>)) {
    f(n);
    let mut c = n.walk();
    for ch in n.children(&mut c) {
        walk(ch, f);
    }
}

/// Whether `scope` itself declares `name`.
fn declares(scope: Node, text: &str, name: &str) -> bool {
    let k = scope.kind();
    if is_function(k) {
        let params = scope.child_by_field_name("parameters").or_else(|| scope.child_by_field_name("parameter"));
        return params.is_some_and(|p| pattern_binds(p, text, name));
    }
    match k {
        "statement_block" | "program" => {
            let mut c = scope.walk();
            let found = scope.named_children(&mut c).any(|s| statement_binds(s, text, name));
            found
        }
        "for_statement" => scope.child_by_field_name("initializer").is_some_and(|i| statement_binds(i, text, name)),
        "for_in_statement" => scope.child_by_field_name("left").is_some_and(|l| pattern_binds(l, text, name)),
        "catch_clause" => scope.child_by_field_name("parameter").is_some_and(|p| pattern_binds(p, text, name)),
        _ => false,
    }
}

fn collect(n: Node, text: &str, name: &str, top: bool, out: &mut Vec<std::ops::Range<usize>>) {
    if !top && is_scope(n.kind()) && declares(n, text, name) {
        // A nested function's own name still belongs to the outer scope.
        if let Some(f) = n.child_by_field_name("name").filter(|_| n.kind() == "function_declaration") {
            if text.get(f.byte_range()) == Some(name) {
                out.push(f.byte_range());
            }
        }
        return;
    }
    if is_name(n.kind()) && text.get(n.byte_range()) == Some(name) {
        out.push(n.byte_range());
    }
    let mut c = n.walk();
    for ch in n.children(&mut c) {
        collect(ch, text, name, false, out);
    }
}

pub(crate) fn name_refs(tree: &Tree, text: &str, byte: usize) -> Option<NameRefs> {
    let root = tree.root_node();
    let mut leaf = root.descendant_for_byte_range(byte, byte)?;
    if !is_name(leaf.kind()) && byte > 0 {
        leaf = root.descendant_for_byte_range(byte - 1, byte - 1)?;
    }
    if !is_name(leaf.kind()) {
        return None;
    }
    let name = text.get(leaf.byte_range())?.to_string();
    // The innermost scope declaring it; a function declaration's name belongs outside it.
    let mut n = leaf;
    let mut found = None;
    while let Some(p) = n.parent() {
        let own_name = p.kind() == "function_declaration" && p.child_by_field_name("name").is_some_and(|f| f.id() == n.id());
        if is_scope(p.kind()) && !own_name && declares(p, text, &name) {
            found = Some(p);
            break;
        }
        n = p;
    }
    let scope = found.unwrap_or(root);
    let local = found.is_some_and(|s| s.kind() != "program");
    let mut ranges = Vec::new();
    collect(scope, text, &name, true, &mut ranges);
    ranges.sort_by_key(|r| r.start);
    ranges.dedup();
    Some(NameRefs { name, ranges, local })
}

#[cfg(test)]
mod tests {
    use crate::{Lang, SyntaxTree};

    fn refs(lang: Lang, text: &str, at: &str) -> Option<(Vec<usize>, bool)> {
        let mut t = SyntaxTree::new(lang);
        t.parse(text);
        // At the last character of `at`.
        let r = t.name_refs(text, text.find(at)? + at.len() - 1)?;
        Some((r.ranges.iter().map(|r| r.start).collect(), r.local))
    }

    #[test]
    fn locals_and_parameters() {
        let t = "function outer(count) {\n  const total = count + 1;\n  return total * count;\n}\nouter(2);\n";
        let (r, local) = refs(Lang::JavaScript, t, "(count").unwrap();
        assert_eq!(r.len(), 3);
        assert!(local);
        let (r, local) = refs(Lang::JavaScript, t, " total").unwrap();
        assert_eq!(r.len(), 2);
        assert!(local);
        let (r, local) = refs(Lang::JavaScript, t, "}\nouter").unwrap();
        assert_eq!(r.len(), 2);
        assert!(!local);
    }

    #[test]
    fn shadowing_and_members() {
        let t = "const x = 1;\nfunction f(x) { return x + o.x; }\nconst g = () => { let x = 2; return x; };\nconsole.log(x, { x });\n";
        let (r, _) = refs(Lang::TypeScript, t, "const x").unwrap();
        // The top-level x, the one in console.log and the shorthand `{ x }`.
        assert_eq!(r.len(), 3);
        let (r, local) = refs(Lang::TypeScript, t, "f(x").unwrap();
        assert_eq!(r.len(), 2);
        assert!(local);
        let t = "import { a as b } from './m';\nb();\n";
        assert_eq!(refs(Lang::Tsx, t, "\nb").unwrap().0.len(), 2);
    }
}
