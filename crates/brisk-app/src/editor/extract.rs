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
    if snake.is_empty() {
        "value".into()
    } else {
        snake
    }
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

    /// Without a selection: the expressions around the caret (char ranges), innermost first.
    pub fn expression_choices(&mut self) -> Vec<Range<usize>> {
        let byte = self.buffer.rope().char_to_byte(self.buffer.primary().head);
        let text = self.text().to_string();
        let Some(tree) = self.python_tree() else { return Vec::new() };
        let found = brisk_syntax::expressions_at(tree, &text, byte);
        let rope = self.buffer.rope();
        found.into_iter().map(|r| rope.byte_to_char(r.start)..rope.byte_to_char(r.end)).collect()
    }

    /// How many times the selection occurs in the whole file (Extract Constant).
    pub fn constant_count(&self) -> usize {
        let Some(r) = self.extract_target() else { return 0 };
        let text: Vec<char> = self.text_string().chars().collect();
        occurrences(&text, &text[r.clone()], 0..text.len()).len().max(1)
    }

    /// Extract the selected expression into a variable; `all`: replace every occurrence in
    /// the function. False without a usable selection.
    pub fn extract_variable(&mut self, all: bool, cx: &mut Context<Self>) -> bool {
        self.extract_into(all, false, cx)
    }

    /// Extract Constant (Ctrl+Alt+C): the selection becomes an UPPER_CASE module constant
    /// after the imports; `all`: every occurrence in the file.
    pub fn extract_constant(&mut self, all: bool, cx: &mut Context<Self>) -> bool {
        self.extract_into(all, true, cx)
    }

    fn extract_into(&mut self, all: bool, constant: bool, cx: &mut Context<Self>) -> bool {
        let Some(r) = self.extract_target() else { return false };
        let text: Vec<char> = self.text_string().chars().collect();
        let expr: String = text[r.clone()].iter().collect();
        if expr.contains('\n') && !expr.starts_with(['(', '[', '{']) {
            return false;
        }
        let scope = if constant { 0..text.len() } else { self.extract_scope(r.start) };
        let mut targets = if all { occurrences(&text, &text[r.clone()], scope) } else { vec![r.clone()] };
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
        let base = if constant { suggest_name(&expr).to_uppercase() } else { suggest_name(&expr) };
        let mut name = base.clone();
        let mut k = 1;
        while words.contains(&name) {
            name = format!("{base}{k}");
            k += 1;
        }
        // A variable goes on the line above its statement; a constant after the imports.
        let (at, indent, lead, decl) = if constant {
            let (row, gap) = self.python_tree().map(brisk_syntax::constant_row).unwrap_or((0, false));
            let row = row.min(self.buffer.len_lines().saturating_sub(1));
            let lead = if gap { "\n" } else { "" };
            let tail = if gap { "\n" } else { "\n\n" };
            (self.buffer.line_start(row), String::new(), lead, format!("{lead}{name} = {expr}{tail}"))
        } else {
            let first_line = self.buffer.pos_to_line_col(targets[0].start).0;
            let line_text = self.buffer.line(first_line);
            let indent: String = line_text.chars().take_while(|c| *c == ' ' || *c == '\t').collect();
            let decl = format!("{indent}{name} = {expr}\n");
            (self.buffer.line_start(first_line), indent, "", decl)
        };
        if targets.iter().any(|t| t.start < at) {
            return false;
        }
        let mut edits: Vec<(Range<usize>, String)> = vec![(at..at, decl.clone())];
        edits.extend(targets.iter().map(|t| (t.clone(), name.clone())));
        self.buffer.replace_ranges(edits);
        // Select the name in the declaration and at every use: typing renames them together.
        let n = name.chars().count();
        let shift = decl.chars().count();
        let start = at + lead.len() + indent.chars().count();
        let mut sels = vec![Selection { anchor: start, head: start + n, goal_col: None }];
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

impl Editor {
    /// Byte-range edits on the current text as one undo step.
    pub fn apply_byte_edits(&mut self, edits: Vec<(Range<usize>, String)>, cx: &mut Context<Self>) {
        let rope = self.buffer.rope();
        // Edits computed on an older text (a stale popup) are dropped, not applied.
        if edits
            .iter()
            .any(|(r, _)| r.start > r.end || r.end > rope.len_bytes() || rope.try_byte_to_char(r.start).is_err() || rope.try_byte_to_char(r.end).is_err())
        {
            return;
        }
        let edits: Vec<(Range<usize>, String)> = edits.into_iter().map(|(r, s)| (rope.byte_to_char(r.start)..rope.byte_to_char(r.end), s)).collect();
        if let Some(first) = edits.iter().map(|e| e.0.start).min() {
            self.buffer.replace_ranges(edits);
            self.buffer.set_caret(first.min(self.buffer.len_chars()), false);
            self.ensure_caret_visible();
            cx.notify();
        }
    }

    /// Extract Method (Ctrl+Alt+M) on the selection (or the statement at the caret): a new
    /// function or method named `extracted` (made unique) and its call, as one undo step;
    /// both names are then selected, so typing renames them together.
    pub fn extract_method(&mut self, cx: &mut Context<Self>) -> Result<String, String> {
        let r = match self.extract_target() {
            Some(r) => r,
            None => {
                let (l, _) = self.buffer.pos_to_line_col(self.buffer.primary().head);
                self.buffer.line_start(l)..self.buffer.line_end(l)
            }
        };
        let text = self.text().to_string();
        let sel = self.buffer.rope().char_to_byte(r.start)..self.buffer.rope().char_to_byte(r.end);
        let mut name = "extracted".to_string();
        let mut k = 1;
        while occurrences(&text.chars().collect::<Vec<_>>(), &name.chars().collect::<Vec<_>>(), 0..text.chars().count()).len() > 0 {
            name = format!("extracted{k}");
            k += 1;
        }
        let tree = self.python_tree().ok_or("Extract Method works in Python files")?;
        let plan = brisk_syntax::extract_method(tree, &text, sel, &name)?;
        let b = &self.buffer;
        let rope = b.rope();
        let call = match &plan.expr {
            Some(e) => (rope.byte_to_char(e.start)..rope.byte_to_char(e.end), plan.call.clone()),
            None => (b.line_start(plan.rows.start)..b.line_end(plan.rows.end - 1), plan.call.clone()),
        };
        let def = plan.def.join("\n");
        let blank = "\n".repeat(plan.blank);
        let insert = if plan.def_row <= plan.rows.start {
            let at = b.line_start(plan.def_row);
            (at..at, format!("{def}\n{blank}"))
        } else if plan.def_row < b.len_lines() {
            let at = b.line_start(plan.def_row);
            (at..at, format!("{blank}{def}\n"))
        } else {
            (b.len_chars()..b.len_chars(), format!("\n{blank}{def}"))
        };
        self.buffer.replace_ranges(vec![call, insert]);
        // Select the new name at the definition and the call.
        let chars: Vec<char> = self.text_string().chars().collect();
        let n: Vec<char> = name.chars().collect();
        let sels: Vec<Selection> =
            occurrences(&chars, &n, 0..chars.len()).into_iter().map(|o| Selection { anchor: o.start, head: o.end, goal_col: None }).collect();
        if !sels.is_empty() {
            self.buffer.selections = sels;
        }
        self.ensure_caret_visible();
        cx.notify();
        let what = if plan.call.contains(&format!(".{name}(")) { "method" } else { "function" };
        Ok(format!(
            "Extracted {what} '{name}' ({}{}): type to rename",
            if plan.params.is_empty() { "no parameters".to_string() } else { format!("parameters {}", plan.params.join(", ")) },
            if plan.returns.is_empty() { String::new() } else { format!(", returns {}", plan.returns.join(", ")) }
        ))
    }

    /// Inline (Ctrl+Alt+N) at the caret: what can be inlined there.
    pub fn inline_at_caret(&mut self) -> Result<brisk_syntax::Inline, String> {
        let byte = self.buffer.rope().char_to_byte(self.buffer.primary().head);
        let text = self.text().to_string();
        let tree = self.python_tree().ok_or("Inline works in Python files")?;
        brisk_syntax::inline_at(tree, &text, byte)
    }

    /// Alt+Enter: the intentions at the caret (f-string, 'if' ones), the function a call of an
    /// unknown name could create, and the variable or function a type hint can go to.
    pub fn intentions_at_caret(&mut self) -> (Vec<brisk_syntax::Intention>, Option<(String, (Range<usize>, String))>, Option<brisk_syntax::HintTarget>) {
        let byte = self.buffer.rope().char_to_byte(self.buffer.primary().head);
        let text = self.text().to_string();
        let Some(tree) = self.python_tree() else { return (Vec::new(), None, None) };
        (brisk_syntax::intentions_at(tree, &text, byte), brisk_syntax::create_function(tree, &text, byte), brisk_syntax::hint_target(tree, &text, byte))
    }

    /// Where an import line goes in this file.
    pub fn import_edit(&mut self, line: &str) -> Option<(Range<usize>, String)> {
        let text = self.text().to_string();
        let tree = self.python_tree()?;
        Some(brisk_syntax::import_edit(tree, &text, line))
    }

    /// A byte offset of the text as a line and character column.
    pub fn byte_pos(&self, byte: usize) -> brisk_lint::Pos {
        let rope = self.buffer.rope();
        let ch = rope.byte_to_char(byte.min(rope.len_bytes()));
        let line = rope.char_to_line(ch);
        brisk_lint::Pos { line, col: ch - rope.line_to_char(line) }
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
