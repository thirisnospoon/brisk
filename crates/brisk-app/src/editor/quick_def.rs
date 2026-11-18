//! Quick Definition (Ctrl+Shift+I): the definition's source in a popup at the caret, as in
//! PyCharm, without leaving the file.

use super::{Anchor, Editor};
use gpui::Context;

/// Longest definition shown; the rest is cut with an ellipsis.
const MAX_LINES: usize = 40;

/// The source of the definition at 0-based `line` of `text`: a `def` or `class` with its
/// decorators and indented body, else just that line. Returns (first line, source).
pub fn definition_source(text: &str, line: usize) -> Option<(usize, String)> {
    let lines: Vec<&str> = text.lines().collect();
    let head = *lines.get(line)?;
    let indent = |l: &str| l.len() - l.trim_start().len();
    let t = head.trim_start();
    let is_block = ["def ", "async def ", "class "].iter().any(|k| t.starts_with(k));
    if !is_block {
        return Some((line, head.trim_end().to_string()));
    }
    let base = indent(head);
    let mut first = line;
    while first > 0 && lines[first - 1].trim_start().starts_with('@') && indent(lines[first - 1]) == base {
        first -= 1;
    }
    let mut last = line;
    for (i, l) in lines.iter().enumerate().skip(line + 1) {
        if l.trim().is_empty() {
            continue;
        }
        if indent(l) <= base {
            break;
        }
        last = i;
    }
    let shown = &lines[first..=last.min(first + MAX_LINES - 1)];
    let mut src: Vec<String> = shown.iter().map(|l| l.get(base.min(indent(l))..).unwrap_or(l).trim_end().to_string()).collect();
    if last >= first + MAX_LINES {
        src.push("…".into());
    }
    Some((first, src.join("\n")))
}

impl Editor {
    /// Show `source` from `place` (e.g. `pkg/util.py:12`) in the documentation popup.
    pub fn show_definition(&mut self, place: &str, source: &str, cx: &mut Context<Self>) {
        let md = format!("{place}\n\n```python\n{source}\n```");
        self.show_doc(&md, Anchor::Caret, cx);
    }
}

#[cfg(test)]
mod tests {
    use super::definition_source;

    #[test]
    fn blocks() {
        let text = "import x\n\n@dec\ndef f(a):\n    if a:\n\n        return 1\n    return 2\n\ny = f(1)\nclass C:\n    def m(self):\n        pass\n";
        assert_eq!(definition_source(text, 3), Some((2, "@dec\ndef f(a):\n    if a:\n\n        return 1\n    return 2".into())));
        assert_eq!(definition_source(text, 9), Some((9, "y = f(1)".into())));
        assert_eq!(definition_source(text, 11), Some((11, "def m(self):\n    pass".into())));
    }
}
