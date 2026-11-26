//! Reading the Python source around the stop: hover expressions, inline values and the
//! targets of Smart Step Into.

use crate::Variable;

const KEYWORDS: &[&str] = &[
    "False", "None", "True", "and", "as", "assert", "async", "await", "break", "class", "continue", "def", "del", "elif", "else", "except", "finally", "for",
    "from", "global", "if", "import", "in", "is", "lambda", "nonlocal", "not", "or", "pass", "raise", "return", "try", "while", "with", "yield",
];

/// The expression under the mouse for a value hover: the name at `col` with the dotted
/// names before it (`obj.attr`), not inside a string or comment, not a keyword.
pub fn hover_expression(line: &str, col: usize) -> Option<String> {
    let c: Vec<char> = line.chars().collect();
    let ident = |ch: char| ch.is_alphanumeric() || ch == '_';
    if !c.get(col).copied().is_some_and(ident) {
        return None;
    }
    // Inside a comment or a string: count quotes before the point.
    let before: String = c[..col].iter().collect();
    let (mut quote, mut i) = (None::<char>, 0);
    let bc: Vec<char> = before.chars().collect();
    while i < bc.len() {
        match (quote, bc[i]) {
            (None, '#') => return None,
            (None, q @ ('"' | '\'')) => quote = Some(q),
            (Some(_), '\\') => i += 1,
            (Some(q), ch) if ch == q => quote = None,
            _ => {}
        }
        i += 1;
    }
    if quote.is_some() {
        return None;
    }
    let mut start = col;
    while start > 0 && ident(c[start - 1]) {
        start -= 1;
    }
    let mut end = col;
    while end < c.len() && ident(c[end]) {
        end += 1;
    }
    let word: String = c[start..end].iter().collect();
    if KEYWORDS.contains(&word.as_str()) || word.chars().next().is_some_and(|ch| ch.is_ascii_digit()) {
        return None;
    }
    while start >= 2 && c[start - 1] == '.' && ident(c[start - 2]) {
        let mut s = start - 1;
        while s > 0 && ident(c[s - 1]) {
            s -= 1;
        }
        start = s;
    }
    Some(c[start..end].iter().collect())
}

/// Names used on a line of Python: identifiers outside strings and comments, not after a dot.
fn line_names(line: &str) -> Vec<&str> {
    let b = line.as_bytes();
    let mut out: Vec<&str> = Vec::new();
    let mut i = 0;
    while i < b.len() {
        let c = b[i];
        if c == b'#' {
            break;
        }
        if c == b'"' || c == b'\'' {
            // Skip the string (prefixes like f"" leave their letter as a word: harmless).
            let q = c;
            i += 1;
            while i < b.len() && b[i] != q {
                i += if b[i] == b'\\' { 2 } else { 1 };
            }
            i += 1;
            continue;
        }
        if c.is_ascii_alphabetic() || c == b'_' {
            let s = i;
            while i < b.len() && (b[i].is_ascii_alphanumeric() || b[i] == b'_') {
                i += 1;
            }
            let after_dot = line[..s].trim_end().ends_with('.');
            let w = &line[s..i];
            if !after_dot && !out.contains(&w) {
                out.push(w);
            }
            continue;
        }
        i += 1;
    }
    out
}

/// PyCharm's inline values: for the lines of the current function up to the stop line,
/// `name: value` of the frame's variables used there. `lines` are the file's lines.
pub fn inline_values(lines: &[String], stop: usize, vars: &[Variable]) -> std::collections::HashMap<usize, String> {
    let mut out = std::collections::HashMap::new();
    let Some(cur) = lines.get(stop) else { return out };
    let indent = |l: &str| l.len() - l.trim_start().len();
    let cur_indent = indent(cur);
    // The function's `def` above the stop line (module level: up to 50 lines back).
    let start = (0..=stop)
        .rev()
        .find(|&i| {
            let t = lines[i].trim_start();
            (t.starts_with("def ") || t.starts_with("async def ")) && indent(&lines[i]) < cur_indent.max(1)
        })
        .unwrap_or(stop.saturating_sub(50))
        .max(stop.saturating_sub(300));
    for (i, l) in lines.iter().enumerate().take(stop + 1).skip(start) {
        let shown: Vec<String> = line_names(l)
            .into_iter()
            .filter_map(|n| vars.iter().find(|v| v.name == n))
            .map(|v| {
                // One line: a DataFrame's or a multi-line string's repr is flattened.
                let flat = v.value.split_whitespace().collect::<Vec<_>>().join(" ");
                let mut val: String = flat.chars().take(60).collect();
                if flat.chars().count() > 60 {
                    val.push('…');
                }
                format!("{}: {val}", v.name)
            })
            .collect();
        if !shown.is_empty() {
            out.insert(i, shown.join(", "));
        }
    }
    out
}

/// The statement starting at `line`: following lines join while brackets are open or
/// the line ends with a backslash (at most 30 lines).
pub fn statement_text(lines: &[String], line: usize) -> String {
    let mut out = String::new();
    let mut depth = 0i32;
    for l in lines.iter().skip(line).take(30) {
        let code = l.split('#').next().unwrap_or("");
        out.push_str(code);
        out.push('\n');
        depth += code
            .chars()
            .map(|c| match c {
                '(' | '[' | '{' => 1,
                ')' | ']' | '}' => -1,
                _ => 0,
            })
            .sum::<i32>();
        if depth <= 0 && !code.trim_end().ends_with('\\') {
            break;
        }
    }
    out
}

/// Smart Step Into targets of this statement: debugpy lists every call of the frame's code
/// not run yet (later lines too), as `a(b(3))` or `f (call 2)`. A target stays when its
/// callee is called in `stmt`; when none would, all stay.
pub fn targets_on_statement(stmt: &str, targets: Vec<(i64, String)>) -> Vec<(i64, String)> {
    let squeezed: String = stmt.chars().filter(|c| !c.is_whitespace()).collect();
    let callee = |label: &str| -> String {
        let l = label.split(" (call ").next().unwrap_or(label);
        l.split('(').next().unwrap_or(l).chars().filter(|c| !c.is_whitespace()).collect()
    };
    let on = |label: &str| {
        let c = callee(label);
        if c.is_empty() {
            return true;
        }
        let pat = format!("{c}(");
        // The callee must not be the tail of a longer name (`xa(` is not `a(`).
        squeezed.match_indices(&pat).any(|(i, _)| {
            let before = squeezed[..i].chars().next_back();
            !before.is_some_and(|ch| ch.is_alphanumeric() || ch == '_' || (ch == '.' && !c.contains('.')))
        })
    };
    let kept: Vec<(i64, String)> = targets.iter().filter(|t| on(&t.1)).cloned().collect();
    if kept.is_empty() {
        targets
    } else {
        kept
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn inline_values_of_a_function() {
        let lines: Vec<String> =
            ["x = 1", "def main(n):", "    items = [n, 2]  # n", "    s = \"items\"", "    total = add(items[0], obj.items)"].map(String::from).to_vec();
        let var = |n: &str, v: &str| Variable { name: n.into(), value: v.into(), type_name: String::new(), reference: 0, evaluate_name: String::new() };
        let vars = [var("n", "5"), var("items", "[5, 2]"), var("x", "1")];
        let v = inline_values(&lines, 4, &vars);
        // From the def line on; not inside strings, comments, or after a dot.
        assert_eq!(v.get(&1).map(String::as_str), Some("n: 5"));
        assert_eq!(v.get(&2).map(String::as_str), Some("items: [5, 2], n: 5"));
        assert_eq!(v.get(&3), None);
        assert_eq!(v.get(&4).map(String::as_str), Some("items: [5, 2]"));
        assert_eq!(v.get(&0), None);
    }

    #[test]
    fn smart_step_targets_of_the_line() {
        let t = |v: &[&str]| v.iter().enumerate().map(|(i, s)| (i as i64, s.to_string())).collect::<Vec<_>>();
        let names = |v: Vec<(i64, String)>| v.into_iter().map(|x| x.1).collect::<Vec<_>>();
        let lines: Vec<String> = ["r = a(b(3))", "print(r)"].map(String::from).to_vec();
        let st = statement_text(&lines, 0);
        assert_eq!(names(targets_on_statement(&st, t(&["b(3)", "a(b(3))", "print(r)"]))), ["b(3)", "a(b(3))"]);
        // A call spread over lines; `(call 2)`; a method; a longer name is not a match.
        let lines: Vec<String> = ["x = f(", "    g(1),", "    f(2))  # h(3)", "h(4)"].map(String::from).to_vec();
        let st = statement_text(&lines, 0);
        assert_eq!(names(targets_on_statement(&st, t(&["g(1)", "f(2)", "f (call 2)", "h(4)"]))), ["g(1)", "f(2)", "f (call 2)"]);
        let st = statement_text(&["y = obj.run(xa(1))".to_string()], 0);
        assert_eq!(names(targets_on_statement(&st, t(&["obj.run(xa(1))", "a(2)", "xa(1)"]))), ["obj.run(xa(1))", "xa(1)"]);
        // Nothing matches (labels debugpy shortened): keep all.
        assert_eq!(targets_on_statement("z = 1\n", t(&["q()"])).len(), 1);
    }

    #[test]
    fn hover_expressions() {
        let l = "    total = obj.items[0] + add(x)  # items here";
        let at = |s: &str| l.find(s).unwrap();
        assert_eq!(hover_expression(l, at("total") + 2).as_deref(), Some("total"));
        assert_eq!(hover_expression(l, at("items") + 1).as_deref(), Some("obj.items"));
        assert_eq!(hover_expression(l, at("add")).as_deref(), Some("add"));
        assert_eq!(hover_expression(l, at("here")), None);
        assert_eq!(hover_expression("s = \"x y\" if a else b", 5), None);
        assert_eq!(hover_expression("s = \"x y\" if a else b", 10), None);
        assert_eq!(hover_expression("s = \"x y\" if a else b", 13).as_deref(), Some("a"));
    }
}
