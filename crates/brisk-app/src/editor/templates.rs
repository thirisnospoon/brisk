//! Live templates (an abbreviation then Tab, or Ctrl+J), Surround With (Ctrl+Alt+T) and block
//! comments (Ctrl+Shift+/), the PyCharm way. Bodies use 4-space indents (rewritten to the
//! file's indent unit) and one `${...}` placeholder that ends up selected (`${}`: the caret).

use super::Editor;
use brisk_text::Selection;
use gpui::Context;
use std::ops::Range;

pub struct Template {
    pub key: &'static str,
    pub desc: &'static str,
    pub body: &'static str,
}

/// Python live templates (PyCharm's names where it has them).
pub const TEMPLATES: &[Template] = &[
    Template { key: "main", desc: "if __name__ == '__main__'", body: "if __name__ == \"__main__\":\n    ${main()}" },
    Template { key: "ifmain", desc: "if __name__ == '__main__'", body: "if __name__ == \"__main__\":\n    ${main()}" },
    Template { key: "def", desc: "Function", body: "def ${name}():\n    pass" },
    Template { key: "class", desc: "Class with __init__", body: "class ${Name}:\n    def __init__(self):\n        pass" },
    Template { key: "for", desc: "for loop", body: "for ${item} in items:\n    pass" },
    Template { key: "iter", desc: "for item in iterable", body: "for item in ${iterable}:\n    pass" },
    Template { key: "while", desc: "while loop", body: "while ${condition}:\n    pass" },
    Template { key: "try", desc: "try / except", body: "try:\n    ${}\nexcept Exception as e:\n    raise" },
    Template { key: "with", desc: "with statement", body: "with ${open(path)} as f:\n    pass" },
    Template { key: "prop", desc: "Property", body: "@property\ndef ${name}(self):\n    return self._name" },
    Template { key: "super", desc: "super() call", body: "super().${__init__()}" },
];

/// Surround With: (name, header, footer). The selected lines go between, one level deeper.
pub const SURROUNDERS: &[(&str, &str, &str)] = &[
    ("if", "if ${condition}:", ""),
    ("if / else", "if ${condition}:", "else:\n    pass"),
    ("while", "while ${condition}:", ""),
    ("for", "for ${item} in items:", ""),
    ("try / except", "try:", "except ${Exception} as e:\n    raise"),
    ("try / finally", "try:", "finally:\n    ${pass}"),
    ("with", "with ${open(path)} as f:", ""),
];

/// Text with `${...}` removed, and the char range of the placeholder's text.
fn take_placeholder(s: &str) -> (String, Range<usize>) {
    let Some(a) = s.find("${") else {
        let n = s.chars().count();
        return (s.to_string(), n..n);
    };
    let Some(len) = s[a..].find('}') else {
        let n = s.chars().count();
        return (s.to_string(), n..n);
    };
    let inner = &s[a + 2..a + len];
    let out = format!("{}{inner}{}", &s[..a], &s[a + len + 1..]);
    let start = s[..a].chars().count();
    (out, start..start + inner.chars().count())
}

/// Template lines re-indented: 4-space steps become `unit`, lines after the first get `base`.
fn reindent(body: &str, base: &str, unit: &str) -> String {
    body.lines()
        .enumerate()
        .map(|(i, l)| {
            let spaces = l.chars().take_while(|&c| c == ' ').count();
            let rest = &l[spaces..];
            let lead = format!("{}{}", unit.repeat(spaces / 4), " ".repeat(spaces % 4));
            if i == 0 {
                format!("{lead}{rest}")
            } else {
                format!("{base}{lead}{rest}")
            }
        })
        .collect::<Vec<_>>()
        .join("\n")
}

/// Expanded template text for a caret on a line indented by `base`, and its selection.
pub fn render_template(body: &str, base: &str, unit: &str) -> (String, Range<usize>) {
    take_placeholder(&reindent(body, base, unit))
}

/// The lines wrapped by a surrounder, and the placeholder range.
pub fn render_surround(lines: &[String], header: &str, footer: &str, unit: &str) -> (String, Range<usize>) {
    let base = lines
        .iter()
        .filter(|l| !l.trim().is_empty())
        .map(|l| l.chars().take_while(|c| c.is_whitespace()).collect::<String>())
        .min_by_key(|s| s.chars().count())
        .unwrap_or_default();
    let mut out = format!("{base}{}", reindent(header, &base, unit));
    for l in lines {
        out.push('\n');
        if !l.trim().is_empty() {
            out.push_str(&format!("{base}{unit}{}", &l[base.len().min(l.len())..]));
        }
    }
    if !footer.is_empty() {
        out.push('\n');
        out.push_str(&format!("{base}{}", reindent(footer, &base, unit)));
    }
    take_placeholder(&out)
}

/// Block comment markers by file type; `None` where the language has only line comments.
fn block_markers(path: Option<&std::path::Path>) -> Option<(&'static str, &'static str)> {
    let ext = path?.extension()?.to_str()?.to_ascii_lowercase();
    match ext.as_str() {
        "css" | "scss" | "less" | "js" | "jsx" | "ts" | "tsx" | "c" | "cpp" | "h" | "hpp" | "rs" | "java" | "go" | "sql" | "kt" | "json5" | "jsonc" => Some(("/*", "*/")),
        "html" | "htm" | "xml" | "md" | "vue" | "svg" => Some(("<!--", "-->")),
        _ => None,
    }
}

impl Editor {
    /// The live template whose abbreviation is right before the caret, at a statement start.
    pub(super) fn template_before_caret(&self) -> Option<(&'static Template, usize)> {
        if !self.is_python() || self.buffer.selections.len() != 1 || !self.buffer.primary().is_empty() {
            return None;
        }
        let start = self.word_start_before_caret();
        let caret = self.buffer.primary().head;
        let word: String = self.buffer.rope().slice(start..caret).to_string();
        let (line, col) = self.buffer.pos_to_line_col(start);
        let before: String = self.buffer.line(line).chars().take(col).collect();
        let at_statement = before.trim().is_empty() || (word == "super" && !before.trim_end().ends_with('.'));
        let t = TEMPLATES.iter().find(|t| t.key == word)?;
        at_statement.then_some((t, start))
    }

    /// Tab after an abbreviation: expand it. False when there is none.
    pub fn expand_live_template(&mut self, cx: &mut Context<Self>) -> bool {
        let Some((t, start)) = self.template_before_caret() else { return false };
        let caret = self.buffer.primary().head;
        self.insert_template_at(t, start..caret, cx);
        true
    }

    /// Ctrl+J's list: insert the template at the caret (replacing an abbreviation typed there).
    pub fn insert_template(&mut self, key: &str, cx: &mut Context<Self>) {
        let Some(t) = TEMPLATES.iter().find(|t| t.key == key) else { return };
        let caret = self.buffer.primary().head;
        let start = match self.template_before_caret() {
            Some((_, s)) => s,
            None => self.word_start_before_caret(),
        };
        let word: String = self.buffer.rope().slice(start..caret).to_string();
        let from = if t.key.starts_with(&word) && !word.is_empty() { start } else { caret };
        self.insert_template_at(t, from..caret, cx);
    }

    fn insert_template_at(&mut self, t: &Template, range: Range<usize>, cx: &mut Context<Self>) {
        let (line, _) = self.buffer.pos_to_line_col(range.start);
        let base: String = self.buffer.line(line).chars().take_while(|c| c.is_whitespace()).collect();
        let (text, sel) = render_template(t.body, &base, &self.buffer.indent_unit.clone());
        let at = range.start;
        self.buffer.replace_ranges(vec![(range, text)]);
        self.buffer.selections = vec![Selection { anchor: at + sel.start, head: at + sel.end, goal_col: None }];
        self.ensure_caret_visible();
        cx.notify();
    }

    /// Surround the selected lines (or the caret line) with `SURROUNDERS[i]`.
    pub fn surround_with(&mut self, i: usize, cx: &mut Context<Self>) {
        let Some(&(_, header, footer)) = SURROUNDERS.get(i) else { return };
        let r = self.buffer.primary().range();
        let (first, _) = self.buffer.pos_to_line_col(r.start);
        let (mut last, last_col) = self.buffer.pos_to_line_col(r.end);
        if last > first && last_col == 0 {
            last -= 1;
        }
        let lines: Vec<String> = (first..=last).map(|l| self.buffer.line(l)).collect();
        let (text, sel) = render_surround(&lines, header, footer, &self.buffer.indent_unit.clone());
        let start = self.buffer.line_start(first);
        let end = self.buffer.line_start(last) + self.buffer.line_len(last);
        self.buffer.replace_ranges(vec![(start..end, text)]);
        self.buffer.selections = vec![Selection { anchor: start + sel.start, head: start + sel.end, goal_col: None }];
        self.ensure_caret_visible();
        cx.notify();
    }

    /// Ctrl+Shift+/: wrap the selection in a block comment or unwrap it; with no selection,
    /// insert an empty one at the caret. Languages without block comments (Python, TOML,
    /// YAML) comment the lines instead.
    pub fn toggle_block_comment(&mut self, cx: &mut Context<Self>) {
        let Some((open, close)) = block_markers(self.path.as_deref()) else {
            self.buffer.toggle_line_comment();
            cx.notify();
            return;
        };
        let r = self.buffer.primary().range();
        if r.is_empty() {
            let at = r.start;
            self.buffer.replace_ranges(vec![(at..at, format!("{open}{close}"))]);
            let p = at + open.chars().count();
            self.buffer.set_caret(p, false);
        } else {
            let sel: String = self.buffer.rope().slice(r.clone()).to_string();
            let t = sel.trim();
            let new = if t.starts_with(open) && t.ends_with(close) && t.len() >= open.len() + close.len() {
                let inner = &t[open.len()..t.len() - close.len()];
                let lead = &sel[..sel.find(t).unwrap_or(0)];
                format!("{lead}{}", inner.strip_prefix(' ').and_then(|s| s.strip_suffix(' ')).unwrap_or(inner))
            } else {
                format!("{open}{sel}{close}")
            };
            let n = new.chars().count();
            self.buffer.replace_ranges(vec![(r.clone(), new)]);
            self.buffer.selections = vec![Selection { anchor: r.start, head: r.start + n, goal_col: None }];
        }
        cx.notify();
    }

    /// Ctrl+/ in files without line comments (CSS, HTML, Markdown): each line in a block comment.
    pub fn toggle_line_block_comment(&mut self, cx: &mut Context<Self>) -> bool {
        if self.buffer.line_comment.is_some() {
            return false;
        }
        let Some((open, close)) = block_markers(self.path.as_deref()) else { return false };
        let r = self.buffer.primary().range();
        let (first, _) = self.buffer.pos_to_line_col(r.start);
        let (mut last, lc) = self.buffer.pos_to_line_col(r.end);
        if last > first && lc == 0 {
            last -= 1;
        }
        let lines: Vec<usize> = (first..=last).filter(|&l| !self.buffer.line(l).trim().is_empty()).collect();
        let all = !lines.is_empty() && lines.iter().all(|&l| {
            let t = self.buffer.line(l);
            let t = t.trim();
            t.starts_with(open) && t.ends_with(close)
        });
        let mut edits = Vec::new();
        for &l in &lines {
            let text = self.buffer.line(l);
            let ws = text.chars().take_while(|c| c.is_whitespace()).count();
            let start = self.buffer.line_start(l);
            let body: String = text.chars().skip(ws).collect();
            let body = body.trim_end();
            let new = if all {
                let inner = &body[open.len()..body.len() - close.len()];
                inner.strip_prefix(' ').and_then(|s| s.strip_suffix(' ')).unwrap_or(inner).to_string()
            } else {
                format!("{open} {body} {close}")
            };
            edits.push((start + ws..start + ws + body.chars().count(), new));
        }
        self.buffer.replace_ranges(edits);
        cx.notify();
        true
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn templates_render() {
        let (t, sel) = render_template(TEMPLATES[0].body, "    ", "    ");
        assert_eq!(t, "if __name__ == \"__main__\":\n        main()");
        assert_eq!(&t.chars().skip(sel.start).take(sel.len()).collect::<String>(), "main()");
        let (t, sel) = render_template("try:\n    ${}\nexcept Exception as e:\n    raise", "", "\t");
        assert_eq!(t, "try:\n\t\nexcept Exception as e:\n\traise");
        assert_eq!(sel, 6..6);
    }

    #[test]
    fn surround_lines() {
        let lines = vec!["    x = 1".to_string(), "".to_string(), "    print(x)".to_string()];
        let (t, sel) = render_surround(&lines, "if ${condition}:", "else:\n    pass", "    ");
        assert_eq!(t, "    if condition:\n        x = 1\n\n        print(x)\n    else:\n        pass");
        assert_eq!(&t.chars().skip(sel.start).take(sel.len()).collect::<String>(), "condition");
        let (t, _) = render_surround(&["a()".to_string()], "try:", "except ${Exception} as e:\n    raise", "    ");
        assert_eq!(t, "try:\n    a()\nexcept Exception as e:\n    raise");
    }
}
