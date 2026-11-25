//! Completion without ty (not installed, or still starting): words of the open file, Python
//! keywords and builtins, matched like ty's list (prefix, then camel humps). After a dot only
//! names the file itself uses after a dot are offered. No auto-import.

use super::Editor;
use brisk_lsp::CompletionItem;
use gpui::Context;
use std::collections::HashSet;

const KEYWORDS: &[&str] = &[
    "False", "None", "True", "and", "as", "assert", "async", "await", "break", "case", "class", "continue", "def", "del", "elif", "else", "except", "finally",
    "for", "from", "global", "if", "import", "in", "is", "lambda", "match", "nonlocal", "not", "or", "pass", "raise", "return", "self", "try", "while", "with",
    "yield",
];

/// Builtin functions (LSP kind 3: accepted with `()`).
const BUILTIN_FUNCTIONS: &[&str] = &[
    "abs",
    "aiter",
    "all",
    "anext",
    "any",
    "ascii",
    "bin",
    "breakpoint",
    "callable",
    "chr",
    "classmethod",
    "compile",
    "delattr",
    "dir",
    "divmod",
    "enumerate",
    "eval",
    "exec",
    "filter",
    "format",
    "getattr",
    "globals",
    "hasattr",
    "hash",
    "help",
    "hex",
    "id",
    "input",
    "isinstance",
    "issubclass",
    "iter",
    "len",
    "locals",
    "map",
    "max",
    "min",
    "next",
    "oct",
    "open",
    "ord",
    "pow",
    "print",
    "property",
    "repr",
    "reversed",
    "round",
    "setattr",
    "sorted",
    "staticmethod",
    "sum",
    "super",
    "vars",
    "zip",
    "__import__",
];

/// Builtin types and exceptions (LSP kind 7).
const BUILTIN_CLASSES: &[&str] = &[
    "bool",
    "bytearray",
    "bytes",
    "complex",
    "dict",
    "float",
    "frozenset",
    "int",
    "list",
    "memoryview",
    "object",
    "range",
    "set",
    "slice",
    "str",
    "tuple",
    "type",
    "BaseException",
    "Exception",
    "ArithmeticError",
    "AssertionError",
    "AttributeError",
    "EOFError",
    "FileExistsError",
    "FileNotFoundError",
    "ImportError",
    "IndexError",
    "KeyError",
    "KeyboardInterrupt",
    "LookupError",
    "ModuleNotFoundError",
    "NameError",
    "NotImplementedError",
    "OSError",
    "OverflowError",
    "PermissionError",
    "RecursionError",
    "RuntimeError",
    "StopIteration",
    "SyntaxError",
    "SystemExit",
    "TimeoutError",
    "TypeError",
    "UnicodeDecodeError",
    "UnicodeEncodeError",
    "ValueError",
    "ZeroDivisionError",
    "NotImplemented",
    "Ellipsis",
];

fn item(label: &str, kind: u32, detail: &str) -> CompletionItem {
    CompletionItem {
        label: label.to_string(),
        detail: detail.to_string(),
        kind,
        sort: String::new(),
        insert: label.to_string(),
        range: None,
        extra: Vec::new(),
        doc: String::new(),
    }
}

fn is_ident(c: char) -> bool {
    c.is_alphanumeric() || c == '_'
}

/// Candidates for the word starting at char `start` (the caret is at `caret`): words of
/// `text` nearest to the caret first, then keywords, then builtins.
pub fn word_items(text: &str, start: usize, caret: usize, after_dot: bool) -> Vec<CompletionItem> {
    // (word, distance from the caret, follows a dot)
    let mut words: Vec<(String, usize, bool)> = Vec::new();
    let mut cur = String::new();
    let mut cur_start = 0;
    let mut prev = ' ';
    let mut before_word = ' ';
    for (i, c) in text.chars().chain(std::iter::once(' ')).enumerate() {
        if is_ident(c) {
            if cur.is_empty() {
                cur_start = i;
                before_word = prev;
            }
            cur.push(c);
        } else if !cur.is_empty() {
            let word = std::mem::take(&mut cur);
            // Skip the word being typed and numbers.
            if cur_start != start && !word.starts_with(|c: char| c.is_ascii_digit()) {
                words.push((word, cur_start.abs_diff(caret), before_word == '.'));
            }
        }
        prev = c;
    }
    words.sort_by_key(|w| w.1);
    let mut seen: HashSet<String> = HashSet::new();
    let mut out = Vec::new();
    for (w, _, dotted) in words {
        if after_dot && !dotted {
            continue;
        }
        if !after_dot && (KEYWORDS.contains(&w.as_str()) || BUILTIN_FUNCTIONS.contains(&w.as_str()) || BUILTIN_CLASSES.contains(&w.as_str())) {
            continue;
        }
        if seen.insert(w.clone()) {
            out.push(item(&w, 1, ""));
        }
    }
    if !after_dot {
        out.extend(KEYWORDS.iter().map(|k| item(k, 14, "keyword")));
        out.extend(BUILTIN_FUNCTIONS.iter().map(|f| item(f, 3, "builtin")));
        out.extend(BUILTIN_CLASSES.iter().map(|c| item(c, 7, "builtin")));
    }
    out
}

impl Editor {
    /// Show the word list at the caret; false when nothing matches what is typed.
    pub fn show_word_completion(&mut self, cx: &mut Context<Self>) -> bool {
        let start = self.word_start_before_caret();
        let caret = self.buffer.primary().head;
        let after_dot = start > 0 && self.buffer.rope().char(start - 1) == '.';
        // In a large file only the lines around the caret (scanning 50k lines per key is slow).
        let items = if self.is_big() {
            let line = self.buffer.pos_to_line_col(caret).0;
            let first = self.buffer.line_start(line.saturating_sub(super::big::WORD_WINDOW));
            let last_line = (line + super::big::WORD_WINDOW).min(self.buffer.len_lines() - 1);
            let end = self.buffer.line_start(last_line);
            let text = self.buffer.rope().slice(first..end.max(caret)).to_string();
            word_items(&text, start - first, caret - first, after_dot)
        } else {
            word_items(&self.buffer.text(), start, caret, after_dot)
        };
        // After a dot the list may still hold postfix items (`.if`, `.not`, ...).
        if items.is_empty() && !after_dot {
            return false;
        }
        self.show_completion_list(items, start, false, cx);
        self.completion_open()
    }
}

#[cfg(test)]
mod tests {
    use super::word_items;

    #[test]
    fn words_keywords_builtins() {
        let text = "user_name = 1\nresult = compute(user_name)\nobj.attr_one = 2\nus";
        let start = text.chars().count() - 2;
        let labels: Vec<String> = word_items(text, start, start + 2, false).into_iter().map(|i| i.label).collect();
        // Nearest words first, the typed word itself left out, then keywords and builtins.
        assert_eq!(&labels[..3], ["attr_one", "obj", "user_name"]);
        assert!(!labels.iter().any(|l| l == "us"));
        assert!(labels.contains(&"return".to_string()) && labels.contains(&"print".to_string()) && labels.contains(&"ValueError".to_string()));
        // After a dot: only names used after a dot.
        let text = "obj.attr_one = 2\nobj.";
        let n = text.chars().count();
        let labels: Vec<String> = word_items(text, n, n, true).into_iter().map(|i| i.label).collect();
        assert_eq!(labels, ["attr_one"]);
    }
}
