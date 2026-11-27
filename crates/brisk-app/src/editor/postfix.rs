//! Postfix completion, as in PyCharm: after `expr.` the completion list also offers `if`,
//! `not`, `for`, `print`, `return` and `par`, which rewrite the expression in place
//! (`x > 0.if` → `if x > 0:` on the next line's indent).

use super::Editor;
use brisk_lint::{Edit, Pos};
use brisk_lsp::CompletionItem;

/// (key, statement-level: takes the whole line before the dot when that is an expression)
const POSTFIX: &[(&str, bool)] = &[("if", true), ("not", false), ("for", true), ("print", true), ("return", true), ("par", false)];

fn is_ident(c: char) -> bool {
    c.is_alphanumeric() || c == '_'
}

/// Start of the primary expression ending at `end` (exclusive): names, dots, calls,
/// subscripts and string literals, e.g. `obj.items()[0]`.
fn primary_start(line: &[char], end: usize) -> usize {
    let mut i = end;
    while i > 0 {
        let c = line[i - 1];
        if is_ident(c) || c == '.' {
            i -= 1;
        } else if matches!(c, ')' | ']' | '}') {
            let open = match c {
                ')' => '(',
                ']' => '[',
                _ => '{',
            };
            let mut depth = 0;
            let mut j = i;
            let mut found = None;
            while j > 0 {
                j -= 1;
                if line[j] == c {
                    depth += 1;
                } else if line[j] == open {
                    depth -= 1;
                    if depth == 0 {
                        found = Some(j);
                        break;
                    }
                }
            }
            match found {
                Some(j) => i = j,
                None => break,
            }
        } else if c == '"' || c == '\'' {
            match line[..i - 1].iter().rposition(|&q| q == c) {
                Some(j) => i = j,
                None => break,
            }
        } else {
            break;
        }
    }
    i
}

/// The text before `dot` that `key` rewrites, as a char range of the line.
pub fn postfix_range(line: &[char], dot: usize, statement: bool) -> Option<std::ops::Range<usize>> {
    let p = primary_start(line, dot);
    let expr: String = line[p..dot].iter().collect();
    // Nothing before the dot, a number (`1.`), or a dot left over from a name (`.x.`).
    let primary_ok = !expr.is_empty() && !expr.starts_with('.') && !expr.starts_with(|c: char| c.is_ascii_digit());
    let ls = line.iter().position(|c| !c.is_whitespace()).unwrap_or(0);
    let whole: String = line[ls..dot].iter().collect();
    let keyword =
        ["return ", "if ", "elif ", "while ", "for ", "with ", "del ", "assert ", "import ", "from ", "def ", "class "].iter().any(|k| whole.starts_with(k));
    let assigns =
        whole.char_indices().any(|(i, c)| c == '=' && !matches!(whole[..i].chars().last(), Some('=' | '!' | '<' | '>')) && !whole[i + 1..].starts_with('='));
    // A whole-line expression (`x > 1.if`), but not a bare number.
    let whole_ok = statement && !keyword && !assigns && !whole.contains('#') && whole.contains(|c: char| c.is_alphabetic() || c == '_');
    if whole_ok && ls < p {
        return Some(ls..dot);
    }
    primary_ok.then_some(p..dot)
}

impl Editor {
    /// Smoke checks: accept the shown postfix item `key`, as a click on it does.
    #[cfg(feature = "smoke")]
    pub fn accept_postfix(&mut self, key: &str, cx: &mut gpui::Context<Self>) -> bool {
        let Some(c) = self.insight.completion.as_mut() else { return false };
        let Some(k) = c.shown.iter().position(|&i| c.items[i].label == key && c.items[i].kind == 15) else { return false };
        c.selected = k;
        self.accept_completion(false, cx);
        true
    }

    /// Postfix items for the completion list whose word starts at `start`, right after a dot.
    pub fn postfix_items(&self, start: usize) -> Vec<CompletionItem> {
        if !self.is_python() || start == 0 || self.buffer.rope().char(start - 1) != '.' {
            return Vec::new();
        }
        let (line, col) = self.buffer.pos_to_line_col(start);
        let text: Vec<char> = self.buffer.line(line).chars().collect();
        let dot = col - 1;
        let first = text.iter().collect::<String>();
        if matches!(first.split_whitespace().next(), Some("import" | "from")) || text[..dot].contains(&'#') {
            return Vec::new();
        }
        let indent: String = text.iter().take_while(|c| c.is_whitespace()).collect();
        let mut out = Vec::new();
        for &(key, statement) in POSTFIX {
            let Some(r) = postfix_range(&text, dot, statement) else { continue };
            let expr: String = text[r.clone()].iter().collect();
            let insert = match key {
                "if" => format!("if {expr}:\n{indent}    "),
                "not" => format!("not {expr}"),
                "for" => format!("for item in {expr}:\n{indent}    "),
                "print" => format!("print({expr})"),
                "return" => format!("return {expr}"),
                _ => format!("({expr})"),
            };
            let shown = if expr.chars().count() > 24 { format!("{}…", expr.chars().take(23).collect::<String>()) } else { expr.clone() };
            out.push(CompletionItem {
                label: key.to_string(),
                detail: format!("postfix: {}", insert.lines().next().unwrap_or("").replace(&expr, &shown)),
                kind: 15,
                sort: String::new(),
                insert,
                range: None,
                // The expression and its dot go, in the same undo step as the insertion.
                resolve: None,
                extra: vec![Edit { start: Pos { line, col: r.start }, end: Pos { line, col }, content: String::new() }],
                doc: String::new(),
            });
        }
        out
    }
}

#[cfg(test)]
mod tests {
    use super::postfix_range;

    fn range(line: &str, statement: bool) -> Option<String> {
        let chars: Vec<char> = line.chars().collect();
        let dot = line.rfind('.').map(|b| line[..b].chars().count())?;
        postfix_range(&chars, dot, statement).map(|r| chars[r].iter().collect())
    }

    #[test]
    fn expressions() {
        assert_eq!(range("    x > 0.", true).as_deref(), Some("x > 0"));
        assert_eq!(range("value > 1.", true).as_deref(), Some("value > 1"));
        assert_eq!(range("    x > 0.", false), None);
        assert_eq!(range("y = obj.items()[0].", true).as_deref(), Some("obj.items()[0]"));
        assert_eq!(range("print(name.", false).as_deref(), Some("name"));
        assert_eq!(range("f'a{b}'.", false).as_deref(), Some("f'a{b}'"));
        assert_eq!(range("    1.", true), None);
        assert_eq!(range("if a == b.", true).as_deref(), Some("b"));
        assert_eq!(range("a == b.", true).as_deref(), Some("a == b"));
    }
}
