//! Extract Variable (Ctrl+Alt+V), as in PyCharm: the selected expression goes into a new
//! variable on the line above its statement, optionally replacing every identical occurrence
//! in the same function. The new name is then selected everywhere (multiple carets), so
//! typing renames it in place.

use super::Editor;
use brisk_text::Selection;
use gpui::Context;
use std::ops::Range;

fn is_ident(c: char) -> bool {
    c.is_alphanumeric() || c == '_'
}

fn indent_of(l: &str) -> usize {
    l.chars().take_while(|c| *c == ' ' || *c == '\t').count()
}

/// A variable name for `expr`: the callee or attribute name (`get_` dropped), else `value`.
pub fn suggest_name(expr: &str) -> String {
    let e = expr.trim();
    let head = e.split(['(', '[']).next().unwrap_or("");
    let last = head.rsplit('.').next().unwrap_or("").trim();
    let base = if !last.is_empty() && last.chars().all(is_ident) && !last.starts_with(|c: char| c.is_ascii_digit()) && last != e {
        last.strip_prefix("get_").unwrap_or(last).to_string()
    } else if e.starts_with(['"', '\'']) || e.starts_with("f\"") || e.starts_with("f'") {
        "text".into()
    } else if e.starts_with('[') {
        "items".into()
    } else if e.starts_with('{') {
        "mapping".into()
    } else {
        "value".into()
    };
    let mut snake = String::new();
    for (i, c) in base.chars().enumerate() {
        if c.is_uppercase() {
            if i > 0 {
                snake.push('_');
            }
            snake.extend(c.to_lowercase());
        } else {
            snake.push(c);
        }
    }
    if snake.is_empty() { "value".into() } else { snake }
}

/// Char ranges of `expr` in `text` between chars `scope`, not inside a longer name.
fn occurrences(text: &[char], expr: &[char], scope: Range<usize>) -> Vec<Range<usize>> {
    let mut out = Vec::new();
    let n = expr.len();
    if n == 0 {
        return out;
    }
    let mut i = scope.start;
    while i + n <= scope.end {
        if text[i..i + n] == *expr {
            let before = i > 0 && is_ident(text[i - 1]) && is_ident(expr[0]);
            let after = i + n < text.len() && is_ident(text[i + n]) && is_ident(expr[n - 1]);
            if !before && !after {
                out.push(i..i + n);
                i += n;
                continue;
            }
        }
        i += 1;
    }
    out
}

impl Editor {
    /// The selection to extract (trimmed), if it is on non-blank text.
    fn extract_target(&self) -> Option<Range<usize>> {
        if self.buffer.selections.len() != 1 {
            return None;
        }
        let r = self.buffer.primary().range();
        let s: String = self.buffer.rope().slice(r.clone()).to_string();
        let lead = s.chars().take_while(|c| c.is_whitespace()).count();
        let trail = s.chars().rev().take_while(|c| c.is_whitespace()).count();
        (lead + trail < s.chars().count()).then(|| r.start + lead..r.end - trail)
    }

    /// Chars of the function (or module) around `pos`: where identical expressions count.
    fn extract_scope(&self, pos: usize) -> Range<usize> {
        let (line, _) = self.buffer.pos_to_line_col(pos);
        let here = indent_of(&self.buffer.line(line));
        let mut start_line = 0;
        let mut def_indent = None;
        // The nearest `def` above that encloses the line (less indented than it).
        for l in (0..line).rev() {
            let t = self.buffer.line(l);
            let tt = t.trim_start();
            if (tt.starts_with("def ") || tt.starts_with("async def ")) && indent_of(&t) < here {
                start_line = l;
                def_indent = Some(indent_of(&t));
                break;
            }
        }
        let Some(di) = def_indent else { return 0..self.buffer.len_chars() };
        let mut end_line = self.buffer.len_lines();
        for l in start_line + 1..self.buffer.len_lines() {
            let t = self.buffer.line(l);
            if !t.trim().is_empty() && indent_of(&t) <= di {
                end_line = l;
                break;
            }
        }
        let end = if end_line >= self.buffer.len_lines() { self.buffer.len_chars() } else { self.buffer.line_start(end_line) };
        self.buffer.line_start(start_line)..end
    }

    /// How many times the selected expression occurs in its function (0 without a selection).
    pub fn extract_count(&self) -> usize {
        let Some(r) = self.extract_target() else { return 0 };
        let text: Vec<char> = self.text_string().chars().collect();
        occurrences(&text, &text[r.clone()], self.extract_scope(r.start)).len().max(1)
    }

    /// Extract the selected expression into a variable; `all`: replace every occurrence in
    /// the function. False without a usable selection.
    pub fn extract_variable(&mut self, all: bool, cx: &mut Context<Self>) -> bool {
        let Some(r) = self.extract_target() else { return false };
        let text: Vec<char> = self.text_string().chars().collect();
        let expr: String = text[r.clone()].iter().collect();
        if expr.contains('\n') && !expr.starts_with(['(', '[', '{']) {
            return false;
        }
        let mut targets = if all { occurrences(&text, &text[r.clone()], self.extract_scope(r.start)) } else { vec![r.clone()] };
        if !targets.contains(&r) {
            targets = vec![r.clone()];
        }
        // A free name: `value`, else `value1`, `value2`... Attributes (`order.total_price`)
        // do not take a name.
        let mut words = std::collections::HashSet::new();
        let mut i = 0;
        while i < text.len() {
            if is_ident(text[i]) && (i == 0 || !is_ident(text[i - 1])) {
                let s = i;
                while i < text.len() && is_ident(text[i]) {
                    i += 1;
                }
                let dotted = text[..s].iter().rev().find(|c| **c != ' ') == Some(&'.');
                if !dotted {
                    words.insert(text[s..i].iter().collect::<String>());
                }
            } else {
                i += 1;
            }
        }
        let base = suggest_name(&expr);
        let mut name = base.clone();
        let mut k = 1;
        while words.contains(&name) {
            name = format!("{base}{k}");
            k += 1;
        }
        let first_line = self.buffer.pos_to_line_col(targets[0].start).0;
        let line_text = self.buffer.line(first_line);
        let indent: String = line_text.chars().take_while(|c| *c == ' ' || *c == '\t').collect();
        let at = self.buffer.line_start(first_line);
        let decl = format!("{indent}{name} = {expr}\n");
        let mut edits: Vec<(Range<usize>, String)> = vec![(at..at, decl.clone())];
        edits.extend(targets.iter().map(|t| (t.clone(), name.clone())));
        self.buffer.replace_ranges(edits);
        // Select the name in the declaration and at every use: typing renames them together.
        let n = name.chars().count();
        let shift = decl.chars().count();
        let mut sels = vec![Selection { anchor: at + indent.chars().count(), head: at + indent.chars().count() + n, goal_col: None }];
        let mut delta = 0isize;
        for t in &targets {
            let s = (t.start as isize + shift as isize + delta) as usize;
            sels.push(Selection { anchor: s, head: s + n, goal_col: None });
            delta += n as isize - t.len() as isize;
        }
        self.buffer.selections = sels;
        self.ensure_caret_visible();
        cx.notify();
        true
    }
}

#[cfg(test)]
mod tests {
    use super::suggest_name;

    #[test]
    fn names() {
        assert_eq!(suggest_name("self.get_user(1)"), "user");
        assert_eq!(suggest_name("order.totalPrice"), "total_price");
        assert_eq!(suggest_name("a + b"), "value");
        assert_eq!(suggest_name("'x'"), "text");
        assert_eq!(suggest_name("[1, 2]"), "items");
        assert_eq!(suggest_name("len(xs)"), "len");
    }
}
