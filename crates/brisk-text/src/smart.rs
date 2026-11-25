//! Python-aware typing, as PyCharm does it: Enter in docstrings, strings, comments and open
//! brackets, dedent after `return` / `pass` / …, `"""` closed by itself, and `else:` /
//! `except:` moved back to their block's indent when the colon is typed.

use super::{Buffer, EditKind, Selection};

/// What a Python line holds up to some column (a light scan, no parser).
#[derive(Debug, Default, PartialEq)]
pub(crate) struct Scan {
    /// Inside a one-line string: its quote, the column of the opening quote, its prefix (`f`, `rb`…).
    pub string: Option<(char, usize, String)>,
    /// Inside a triple-quoted string that opened on this line.
    pub triple: bool,
    /// The column of a `#` that starts a comment.
    pub comment: Option<usize>,
    /// Columns of brackets still open.
    pub open: Vec<usize>,
}

/// Join Lines: `left` with `right` (its indent dropped), and the column of the joint.
pub(crate) fn join_pair(left: &str, right: &str, python: bool, comment: Option<&str>) -> (String, usize) {
    let mut l = left.trim_end().to_string();
    let mut r = right.trim_start();
    if r.is_empty() {
        let at = l.chars().count();
        return (l, at);
    }
    if python && l.ends_with('\\') && !scan(&l).string.is_some() {
        l.pop();
        l.truncate(l.trim_end().len());
    }
    // Two comment lines: one comment.
    if let Some(c) = comment {
        if r.starts_with(c) && scan_comment(&l, python, c) {
            r = r[c.len()..].trim_start();
            if r.is_empty() {
                let at = l.chars().count();
                return (l, at);
            }
        }
    }
    // 'a' 'b' -> 'ab': the left line ends with a one-line literal that the right one continues.
    if python {
        if let Some(q) = l.chars().last().filter(|c| *c == '\'' || *c == '"') {
            let before = &l[..l.len() - 1];
            let s = scan(before);
            let closes = s.string.as_ref().is_some_and(|(sq, _, prefix)| *sq == q && !prefix.to_lowercase().contains('f') && !s.triple);
            let triple_right = r.starts_with(&q.to_string().repeat(3));
            if closes && r.starts_with(q) && !triple_right && !before.ends_with(&q.to_string().repeat(2)) {
                l.pop();
                let at = l.chars().count();
                l.push_str(&r[1..]);
                return (l, at);
            }
        }
    }
    let tight = l.ends_with(['(', '[', '{']) || r.starts_with([')', ']', '}', ',', '.', ':', ';']);
    // The caret goes to the joint, before the space.
    let at = l.chars().count();
    if !tight {
        l.push(' ');
    }
    l.push_str(r);
    (l, at)
}

/// Whether `line` is a comment line (only a comment) for Join Lines.
fn scan_comment(line: &str, python: bool, mark: &str) -> bool {
    let t = line.trim_start();
    if python {
        return t.starts_with(mark) || scan(line).comment.is_some_and(|c| c == line.len() - t.len());
    }
    t.starts_with(mark)
}

pub(crate) fn scan(before: &str) -> Scan {
    let cs: Vec<char> = before.chars().collect();
    let mut s = Scan::default();
    let mut i = 0;
    while i < cs.len() {
        let c = cs[i];
        if let Some((q, _, _)) = &s.string {
            if c == '\\' {
                i += 2;
                continue;
            }
            if c == *q {
                s.string = None;
            }
            i += 1;
            continue;
        }
        if s.triple {
            if (c == '"' || c == '\'') && cs[i..].starts_with(&[c, c, c]) {
                s.triple = false;
                i += 3;
                continue;
            }
            i += 1;
            continue;
        }
        match c {
            '#' => {
                s.comment = Some(i);
                break;
            }
            '"' | '\'' => {
                if cs[i..].starts_with(&[c, c, c]) {
                    s.triple = true;
                    i += 3;
                    continue;
                }
                let mut p = i;
                while p > 0 && "rRbBfFuU".contains(cs[p - 1]) && (p - 1 == 0 || !cs[p - 2].is_alphanumeric()) {
                    p -= 1;
                }
                let prefix: String = cs[p..i].iter().collect();
                s.string = Some((c, i, prefix));
            }
            '(' | '[' | '{' => s.open.push(i),
            ')' | ']' | '}' => {
                s.open.pop();
            }
            _ => {}
        }
        i += 1;
    }
    s
}

/// The keywords after which PyCharm puts the next line one level back.
fn ends_block(line: &str) -> bool {
    let t = line.trim();
    ["return", "pass", "break", "continue", "raise"].iter().any(|k| t == *k || t.strip_prefix(k).is_some_and(|r| r.starts_with([' ', '(', '\t'])))
}

/// The block keywords a `else` / `elif` / `except` / `finally` line pairs with.
fn partners(kw: &str) -> &'static [&'static str] {
    match kw {
        "elif" => &["if", "elif"],
        "else" => &["if", "elif", "for", "while", "try", "except"],
        "except" => &["try", "except"],
        "finally" => &["try", "except", "else"],
        _ => &[],
    }
}

fn first_word(t: &str) -> &str {
    let t = t.trim_start();
    let end = t.find(|c: char| !(c.is_alphanumeric() || c == '_')).unwrap_or(t.len());
    &t[..end]
}

impl Buffer {
    /// Enter at `start..end` in Python: the text to insert and where the caret goes in it, or
    /// None for the plain smart Enter.
    pub(crate) fn py_enter(&self, start: usize, end: usize) -> Option<(String, usize)> {
        let (line, col) = self.pos_to_line_col(start);
        let text = self.line(line);
        let before: String = text.chars().take(col).collect();
        let (eline, ecol) = self.pos_to_line_col(end);
        let after: String = self.line(eline).chars().skip(ecol).collect();
        let base = self.leading_ws(line);
        let unit = self.indent_unit.clone();
        let sc = scan(&before);
        let trimmed = before.trim_end();

        // `"""|"""`: the docstring's own line between the quotes.
        for q in ["\"\"\"", "'''"] {
            if trimmed.ends_with(q) && after.starts_with(q) && sc.triple {
                let ins = format!("\n{base}\n{base}");
                return Some((ins, base.chars().count() + 1));
            }
        }
        // `"""|` opening a docstring right under `def` / `class`: close it, like PyCharm's stub.
        for q in ["\"\"\"", "'''"] {
            if sc.triple && trimmed.trim_start() == q && after.trim().is_empty() && !self.closed_below(line, q, base.chars().count()) {
                let ins = format!("\n{base}\n{base}{q}");
                return Some((ins, base.chars().count() + 1));
            }
        }
        // Inside a one-line string: split it into two literals.
        if let Some((q, _, prefix)) = &sc.string {
            if !after.starts_with(*q) && !after.is_empty() {
                let (indent, cont) = match sc.open.last() {
                    Some(&c) => (" ".repeat(c + 1), ""),
                    None => (format!("{base}{unit}"), " \\"),
                };
                let ins = format!("{q}{cont}\n{indent}{prefix}{q}");
                let caret = ins.chars().count();
                return Some((ins, caret));
            }
        }
        // In the middle of a whole-line comment: the new line is a comment too.
        if let Some(c) = sc.comment {
            if before[..].trim_start().starts_with('#') && !after.trim().is_empty() {
                let pad: String = before.chars().take(c).collect();
                let ins = format!("\n{pad}# ");
                let n = ins.chars().count();
                return Some((ins, n));
            }
        }
        // An open bracket with text after it: line up with that text.
        if let Some(&c) = sc.open.last() {
            if !trimmed.ends_with(['(', '[', '{']) {
                let ins = format!("\n{}", " ".repeat(c + 1));
                let n = ins.chars().count();
                return Some((ins, n));
            }
        }
        // `return x` / `pass` …: back one level.
        if after.trim().is_empty() && ends_block(&before) && base.ends_with(&unit) && !unit.is_empty() {
            let indent = &base[..base.len() - unit.len()];
            let ins = format!("\n{indent}");
            return Some((ins.clone(), ins.chars().count()));
        }
        None
    }

    /// Whether a triple quote `q` turns up below `line` before the block (lines indented at
    /// least `indent`) ends: then the docstring is already closed.
    fn closed_below(&self, line: usize, q: &str, indent: usize) -> bool {
        for l in line + 1..self.len_lines() {
            let t = self.line(l);
            if t.trim().is_empty() {
                continue;
            }
            let ws = t.chars().take_while(|c| c.is_whitespace()).count();
            if t.contains(q) {
                return true;
            }
            if ws < indent {
                return false;
            }
        }
        false
    }

    /// Typing `c` in Python where PyCharm does more than insert it. True if handled.
    pub(crate) fn py_type(&mut self, c: char) -> bool {
        if self.selections.len() != 1 || !self.primary().is_empty() {
            return false;
        }
        let p = self.primary().head;
        let (line, col) = self.pos_to_line_col(p);
        let text = self.line(line);
        let before: String = text.chars().take(col).collect();
        let after: String = text.chars().skip(col).collect();
        // The third quote of `"""`: close it as `"""|"""`.
        if (c == '"' || c == '\'')
            && self.auto_pairs
            && before.ends_with(&c.to_string().repeat(2))
            && !before.ends_with(&c.to_string().repeat(3))
            && !after.starts_with(c)
        {
            let sc = scan(&before[..before.len() - 2]);
            if sc.string.is_none() && !sc.triple && sc.comment.is_none() {
                let ins: String = std::iter::repeat_n(c, 4).collect();
                self.apply(vec![(p..p, ins)], EditKind::Typing, |map, _| vec![Selection::caret(map(p, false) + 1)]);
                return true;
            }
        }
        // `else:` / `elif x:` / `except E:` / `finally:` go back to the indent of their `if` / `try`.
        if c == ':' && after.trim().is_empty() {
            let kw = first_word(&before);
            let pk = partners(kw);
            let ws: usize = before.chars().take_while(|c| *c == ' ' || *c == '\t').count();
            if !pk.is_empty() && scan(&before).string.is_none() {
                // The nearest line above, less indented, that opens a partner block; a `def` /
                // `class` on the way ends the search.
                let mut target = None;
                for t in (0..line).rev().map(|l| self.line(l)).filter(|t| !t.trim().is_empty()) {
                    let w = t.chars().take_while(|c| *c == ' ' || *c == '\t').count();
                    if w >= ws {
                        continue;
                    }
                    let word = first_word(&t);
                    if pk.contains(&word) {
                        target = Some(t.chars().take(w).collect::<String>());
                        break;
                    }
                    if matches!(word, "def" | "class" | "async") || w == 0 {
                        break;
                    }
                }
                if let Some(indent) = target {
                    let ls = self.line_start(line);
                    let edits = vec![(ls..ls + ws, indent.clone()), (p..p, ":".to_string())];
                    let shift = indent.chars().count() as isize - ws as isize;
                    self.apply(edits, EditKind::Typing, move |_, _| vec![Selection::caret((p as isize + shift) as usize + 1)]);
                    return true;
                }
            }
        }
        false
    }
}

fn indent_width(l: &str, tab: usize) -> usize {
    l.chars().take_while(|c| *c == ' ' || *c == '\t').map(|c| if c == '\t' { tab } else { 1 }).sum()
}

impl Buffer {
    /// Paste in Python, PyCharm's "Indent block on paste": a block of several lines pasted where
    /// only whitespace is before the caret keeps its inner structure and gets the caret's
    /// indent. The first line may have been copied without its indent (from its first char): it
    /// is then taken as one level above the next line if it ends with `:`, else level with it.
    pub fn paste_python(&mut self, text: &str) {
        let adjusted = (self.python && self.selections.len() == 1 && text.contains('\n')).then(|| self.reindent_paste(text)).flatten();
        self.insert(adjusted.as_deref().unwrap_or(text));
    }

    fn reindent_paste(&self, text: &str) -> Option<String> {
        let s = self.primary();
        let start = s.range().start;
        let (line, col) = self.pos_to_line_col(start);
        let before: String = self.line(line).chars().take(col).collect();
        if !before.chars().all(|c| c == ' ' || c == '\t') {
            return None;
        }
        let tab = self.indent_unit.chars().count().max(1);
        let unit = if self.indent_unit.is_empty() { "    ".to_string() } else { self.indent_unit.clone() };
        let unit_w = unit.chars().count().max(1);
        let lines: Vec<&str> = text.split('\n').collect();
        let rest_min = lines[1..].iter().filter(|l| !l.trim().is_empty()).map(|l| indent_width(l, tab)).min();
        let first = lines[0];
        let base = if first.starts_with([' ', '\t']) || first.trim().is_empty() {
            indent_width(first, tab)
        } else {
            match rest_min {
                Some(m) if first.trim_end().ends_with(':') => m.saturating_sub(unit_w),
                Some(m) => m,
                None => 0,
            }
        };
        let target = indent_width(&before, tab);
        let pad = |w: usize| -> String {
            if unit.starts_with('\t') {
                "\t".repeat(w / tab) + &" ".repeat(w % tab)
            } else {
                " ".repeat(w)
            }
        };
        let mut out = String::with_capacity(text.len());
        out.push_str(first.trim_start_matches([' ', '\t']));
        for l in &lines[1..] {
            out.push('\n');
            if l.trim().is_empty() {
                continue;
            }
            let w = (indent_width(l, tab) + target).saturating_sub(base);
            out.push_str(&pad(w));
            out.push_str(l.trim_start_matches([' ', '\t']));
        }
        (out != text).then_some(out)
    }
}
