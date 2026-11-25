//! Structural edits on Python statements: Move Statement Up / Down (whole statements past
//! their neighbours in the same block) and Unwrap / Remove (Ctrl+Shift+Delete). Rows are
//! 0-based line numbers; `Range<usize>` of rows is end-exclusive.

use std::ops::Range;
use tree_sitter::{Node, Tree};

fn rows(n: &Node) -> Range<usize> {
    n.start_position().row..n.end_position().row + 1
}

fn named_children<'t>(n: Node<'t>) -> Vec<Node<'t>> {
    (0..n.named_child_count()).filter_map(|i| n.named_child(i as _)).collect()
}

/// The block (statement list) inside compound statement `n` that holds rows `first..=last`,
/// looking through clauses (`elif`, `else`, `except`, `finally`, `case`).
fn block_holding<'t>(n: Node<'t>, first: usize, last: usize) -> Option<Node<'t>> {
    for c in named_children(n) {
        let r = rows(&c);
        if r.start > first || r.end <= last {
            continue;
        }
        if c.kind() == "block" {
            return Some(c);
        }
        if c.kind().ends_with("_clause") || c.kind() == "case_clause" {
            return block_holding(c, first, last);
        }
    }
    None
}

/// Move Statement: the statements covering rows `first..=last` (whole statements of one
/// block) and the neighbouring statement they swap with (`None` at the block's edge).
pub fn statement_move(tree: &Tree, first: usize, last: usize, up: bool) -> Option<(Range<usize>, Option<Range<usize>>)> {
    let mut node = tree.root_node();
    loop {
        let kids = named_children(node);
        let hit: Vec<usize> = kids.iter().enumerate().filter(|(_, k)| rows(k).start <= last && rows(k).end > first).map(|(i, _)| i).collect();
        let (&a, &z) = (hit.first()?, hit.last()?);
        // Lines inside one compound statement's body: move within that body.
        if a == z && rows(&kids[a]).start < first {
            if let Some(inner) = block_holding(kids[a], first, last) {
                node = inner;
                continue;
            }
        }
        let target = rows(&kids[a]).start..rows(&kids[z]).end;
        let other = if up { a.checked_sub(1).map(|i| rows(&kids[i])) } else { kids.get(z + 1).map(rows) };
        // Siblings sharing a row (`a = 1; b = 2`) cannot be swapped by lines.
        let other = other.filter(|o| o.end <= target.start || o.start >= target.end);
        return Some((target, other));
    }
}

/// One Unwrap / Remove choice: replace `rows` with `text` (`None`: delete the rows).
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Unwrap {
    pub label: String,
    pub rows: Range<usize>,
    pub text: Option<String>,
}

fn body<'t>(n: Node<'t>) -> Option<Node<'t>> {
    n.child_by_field_name("consequence").or_else(|| n.child_by_field_name("body")).or_else(|| named_children(n).into_iter().find(|c| c.kind() == "block"))
}

fn indent_of(line: &str) -> &str {
    &line[..line.len() - line.trim_start().len()]
}

/// `block`'s lines moved to `indent` (the statement's own), as PyCharm unwraps.
fn dedented(lines: &[&str], text: &str, block: Node, indent: &str) -> String {
    let r = rows(&block);
    // `if x: y`: the body shares the header's row.
    if lines.get(r.start).is_some_and(|l| indent_of(l).len() < block.start_position().column) {
        let first = text.get(block.start_byte()..block.end_byte()).unwrap_or("").lines().next().unwrap_or("");
        return format!("{indent}{first}");
    }
    let own = lines.get(r.start).map(|l| indent_of(l).len()).unwrap_or(0);
    lines[r.start..r.end.min(lines.len())]
        .iter()
        .map(|l| {
            let ws = indent_of(l).len().min(own);
            if l.trim().is_empty() {
                String::new()
            } else {
                format!("{indent}{}", &l[ws..])
            }
        })
        .collect::<Vec<_>>()
        .join("\n")
}

fn head_word(n: &Node) -> &'static str {
    match n.kind() {
        "if_statement" => "if",
        "elif_clause" => "elif",
        "else_clause" => "else",
        "for_statement" => "for",
        "while_statement" => "while",
        "with_statement" => "with",
        "try_statement" => "try",
        "except_clause" | "except_group_clause" => "except",
        "finally_clause" => "finally",
        _ => "",
    }
}

/// Unwrap / Remove choices for the statements around `byte`, innermost first: "Unwrap 'if...'"
/// keeps the body, "Remove 'else...'" drops the clause, "Unwrap 'else...'" keeps only it.
pub fn unwrap_options(tree: &Tree, text: &str, byte: usize) -> Vec<Unwrap> {
    let v = unwrap_from(tree, text, byte);
    // At the end of a line the caret is just past the statement's node.
    if v.is_empty() && byte > 0 {
        unwrap_from(tree, text, byte - 1)
    } else {
        v
    }
}

fn unwrap_from(tree: &Tree, text: &str, byte: usize) -> Vec<Unwrap> {
    let lines: Vec<&str> = text.split('\n').collect();
    let mut out = Vec::new();
    let mut n = tree.root_node().descendant_for_byte_range(byte, byte);
    while let Some(cur) = n {
        n = cur.parent();
        let word = head_word(&cur);
        if word.is_empty() {
            continue;
        }
        let indent = |node: &Node| lines.get(node.start_position().row).map(|l| indent_of(l).to_string()).unwrap_or_default();
        match cur.kind() {
            "if_statement" | "for_statement" | "while_statement" | "with_statement" | "try_statement" => {
                if let Some(b) = body(cur) {
                    out.push(Unwrap { label: format!("Unwrap '{word}...'"), rows: rows(&cur), text: Some(dedented(&lines, text, b, &indent(&cur))) });
                }
            }
            "else_clause" | "elif_clause" | "except_clause" | "except_group_clause" | "finally_clause" => {
                let Some(parent) = cur.parent() else { continue };
                if word == "else" && parent.kind() == "if_statement" {
                    if let Some(b) = body(cur) {
                        out.push(Unwrap { label: "Unwrap 'else...'".into(), rows: rows(&parent), text: Some(dedented(&lines, text, b, &indent(&parent))) });
                    }
                }
                // A try needs one except or finally left.
                let handlers = named_children(parent).iter().filter(|c| matches!(c.kind(), "except_clause" | "except_group_clause" | "finally_clause")).count();
                let removable = !matches!(word, "except" | "finally") || handlers > 1;
                if removable && rows(&cur).start > parent.start_position().row {
                    out.push(Unwrap { label: format!("Remove '{word}...'"), rows: rows(&cur), text: None });
                }
            }
            _ => {}
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    fn parse(src: &str) -> Tree {
        crate::parse_full(crate::Lang::Python, src, None).unwrap()
    }

    #[test]
    fn moves_whole_statements_within_their_block() {
        let src = "def a():\n    pass\n\n\ndef b():\n    x = 1\n    y = 2\n";
        let t = parse(src);
        // The caret on `def b` moves the whole function above `def a`.
        assert_eq!(statement_move(&t, 4, 4, true), Some((4..7, Some(0..2))));
        // `y = 2` inside b swaps with `x = 1`, and stops at the block's end.
        assert_eq!(statement_move(&t, 6, 6, true), Some((6..7, Some(5..6))));
        assert_eq!(statement_move(&t, 6, 6, false), Some((6..7, None)));
    }

    #[test]
    fn unwrap_and_remove() {
        let src = "def f(x):\n    if x:\n        a()\n        b()\n    else:\n        c()\n";
        let t = parse(src);
        let at = src.find("c()").unwrap();
        let o = unwrap_options(&t, src, at);
        let labels: Vec<_> = o.iter().map(|u| u.label.as_str()).collect();
        assert_eq!(labels, ["Unwrap 'else...'", "Remove 'else...'", "Unwrap 'if...'"]);
        assert_eq!(o[0], Unwrap { label: "Unwrap 'else...'".into(), rows: 1..6, text: Some("    c()".into()) });
        assert_eq!(o[1].rows, 4..6);
        assert_eq!(o[2].text.as_deref(), Some("    a()\n    b()"));
        let src = "try:\n    run()\nexcept E:\n    pass\n";
        let o = unwrap_options(&parse(src), src, src.find("pass").unwrap());
        // The only handler of a try is not offered for removal.
        assert_eq!(o.iter().map(|u| u.label.as_str()).collect::<Vec<_>>(), ["Unwrap 'try...'"]);
        assert_eq!(o[0].text.as_deref(), Some("run()"));
    }
}
