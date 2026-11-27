//! Adding type hints to Python variables without typing them out.
//!
//! - Type `:` after a variable that has a value (`total = 0` → `total: = 0`) and the type ty
//!   inferred appears in grey after the colon, as PyCharm's inline completion does; Tab
//!   writes it, typing its first letters keeps it, anything else drops it.
//! - Quick Documentation on such a variable offers "Add type hint" with the type.
//!
//! Both offer only simple types: the builtins (`int`, `str`, …), `None`, and the builtin
//! containers of them (`list[int]`, `dict[str, float]`, `int | None`). Those never need an
//! import and read the same everywhere; a class or anything ty only half knows (`Unknown`)
//! is left to Alt+Enter › Add type hint. Literal types are widened: `x = 0` gets `int`, not
//! `Literal[0]`.

use super::{char_to_byte, Editor};

/// The builtin types offered on their own.
const SCALARS: &[&str] = &["int", "float", "str", "bool", "bytes", "complex", "None", "bytearray", "object"];
/// The builtin containers, with their type arguments.
const CONTAINERS: &[&str] = &["list", "set", "frozenset", "dict", "tuple", "type"];

/// The type to write for ty's inferred `ty`, when it is a simple one (see the module docs).
pub fn simple_type(ty: &str) -> Option<String> {
    let ty = ty.trim().trim_start_matches(':').trim();
    let out = widen(ty)?;
    (!out.is_empty()).then_some(out)
}

/// `ty` with literal types widened, or None when it is not simple.
fn widen(ty: &str) -> Option<String> {
    let ty = ty.trim();
    // A union: each part simple; equal parts once (`Literal[1] | Literal[2]` is `int`).
    let parts = split_top(ty, '|');
    if parts.len() > 1 {
        let mut out: Vec<String> = Vec::new();
        for p in parts {
            let w = widen(p)?;
            if !out.contains(&w) {
                out.push(w);
            }
        }
        return Some(out.join(" | "));
    }
    if ty == "LiteralString" {
        return Some("str".into());
    }
    if let Some(inner) = ty.strip_prefix("Literal[").and_then(|r| r.strip_suffix(']')) {
        let mut kinds: Vec<&str> = Vec::new();
        for v in split_top(inner, ',') {
            let v = v.trim();
            let k = if v == "True" || v == "False" {
                "bool"
            } else if v.starts_with("b'") || v.starts_with("b\"") {
                "bytes"
            } else if v.starts_with('\'') || v.starts_with('"') {
                "str"
            } else if v.trim_start_matches('-').chars().all(|c| c.is_ascii_digit()) && !v.is_empty() {
                "int"
            } else {
                // An enum member: not a simple type.
                return None;
            };
            if !kinds.contains(&k) {
                kinds.push(k);
            }
        }
        return Some(kinds.join(" | "));
    }
    if SCALARS.contains(&ty) {
        return Some(ty.to_string());
    }
    let (base, args) = match ty.split_once('[') {
        Some((b, rest)) => (b, Some(rest.strip_suffix(']')?)),
        None => (ty, None),
    };
    if !CONTAINERS.contains(&base) {
        return None;
    }
    let Some(args) = args else { return Some(base.to_string()) };
    let mut out = Vec::new();
    for a in split_top(args, ',') {
        let a = a.trim();
        // `tuple[int, ...]`.
        out.push(if a == "..." { a.to_string() } else { widen(a)? });
    }
    Some(format!("{base}[{}]", out.join(", ")))
}

/// The type of a literal value, for when ty gives no hint there (it skips the obvious ones):
/// `0` is `int`, `"x"` is `str`, `[1, 2]` is `list[int]`. Containers need items, all of one
/// type; anything else is None.
pub fn literal_type(value: &str) -> Option<String> {
    let v = value.trim();
    // A trailing comment.
    let v = match split_top(v, '#').first() {
        Some(code) => code.trim(),
        None => v,
    };
    if v.is_empty() {
        return None;
    }
    if v == "True" || v == "False" {
        return Some("bool".into());
    }
    let first = v.chars().next()?;
    let prefix_len = v.find(['"', '\'']).filter(|&i| i <= 3 && v[..i].chars().all(|c| "rbfuRBFU".contains(c)));
    if let Some(i) = prefix_len {
        let closes = v.ends_with(&v[i..i + 1]) && v.len() > i + 1;
        if !closes {
            return None;
        }
        return Some(if v[..i].to_ascii_lowercase().contains('b') { "bytes" } else { "str" }.into());
    }
    if first.is_ascii_digit() || (first == '-' && v.len() > 1) || first == '.' {
        let n = v.trim_start_matches('-').replace('_', "");
        if n.ends_with(['j', 'J']) && n[..n.len() - 1].parse::<f64>().is_ok() {
            return Some("complex".into());
        }
        if n.parse::<i128>().is_ok() || ["0x", "0o", "0b"].iter().any(|p| n.starts_with(p) && i128::from_str_radix(&n[2..], 16).is_ok()) {
            return Some("int".into());
        }
        return n.parse::<f64>().is_ok().then(|| "float".into());
    }
    let items = |inner: &str| -> Option<String> {
        let parts: Vec<&str> = split_top(inner, ',').into_iter().map(str::trim).filter(|p| !p.is_empty()).collect();
        let mut ty: Option<String> = None;
        for p in &parts {
            let t = literal_type(p)?;
            match &ty {
                Some(prev) if *prev != t => return None,
                _ => ty = Some(t),
            }
        }
        ty
    };
    let inner = |open: char, close: char| v.strip_prefix(open).and_then(|r| r.strip_suffix(close));
    if let Some(i) = inner('[', ']') {
        return items(i).map(|t| format!("list[{t}]"));
    }
    if let Some(i) = inner('(', ')').filter(|i| split_top(i, ',').len() > 1) {
        return items(i).map(|t| format!("tuple[{t}, ...]"));
    }
    if let Some(i) = inner('{', '}') {
        let pairs: Vec<&str> = split_top(i, ',').into_iter().map(str::trim).filter(|p| !p.is_empty()).collect();
        if pairs.is_empty() {
            return None;
        }
        if pairs.iter().all(|p| split_top(p, ':').len() == 2) {
            let keys: Vec<&str> = pairs.iter().map(|p| split_top(p, ':')[0]).collect();
            let vals: Vec<&str> = pairs.iter().map(|p| split_top(p, ':')[1]).collect();
            return Some(format!("dict[{}, {}]", items(&keys.join(","))?, items(&vals.join(","))?));
        }
        return items(i).map(|t| format!("set[{t}]"));
    }
    None
}

/// `s` split at `sep` outside brackets and quotes.
fn split_top(s: &str, sep: char) -> Vec<&str> {
    let (mut depth, mut quote, mut start) = (0i32, None::<char>, 0);
    let mut out = Vec::new();
    for (i, c) in s.char_indices() {
        match (quote, c) {
            (Some(q), c) if c == q => quote = None,
            (Some(_), _) => {}
            (None, '\'' | '"') => quote = Some(c),
            (None, '[' | '(') => depth += 1,
            (None, ']' | ')') => depth -= 1,
            (None, c) if c == sep && depth == 0 => {
                out.push(&s[start..i]);
                start = i + c.len_utf8();
            }
            _ => {}
        }
    }
    out.push(&s[start..]);
    out
}

/// The grey type after a typed colon: where it stands and what Tab writes.
#[derive(Clone, Debug, PartialEq)]
pub struct TypeGhost {
    pub line: usize,
    /// Byte in the line where the ghost text stands (the caret).
    pub byte: usize,
    /// The line's text the ghost belongs to; another text drops it.
    pub text: String,
    /// What Tab inserts (and what is drawn).
    pub insert: String,
}

impl Editor {
    /// Before a typed `:`: the simple type ty inferred for the variable right before the
    /// caret, when the line is `name = value` and the caret is just after `name`.
    pub(super) fn type_before_colon(&self) -> Option<String> {
        if self.buffer.selections.len() != 1 || !self.buffer.primary().is_empty() || !self.is_python() {
            return None;
        }
        let (line, col) = self.buffer.pos_to_line_col(self.buffer.primary().head);
        let text = self.buffer.line(line);
        let byte = char_to_byte(&text, col);
        let (before, after) = text.split_at(byte);
        let after = after.trim_start();
        if !before.ends_with(|c: char| c.is_alphanumeric() || c == '_') || !after.starts_with('=') || after.starts_with("==") {
            return None;
        }
        // ty's type, else the literal's own (ty gives no hint for `total = 0`).
        match self.inlays.raw_type(line, &text, byte) {
            Some(label) => simple_type(label),
            None => literal_type(&after[1..]),
        }
    }

    /// After a typed char: start the ghost after a `:` that had a type waiting, keep it while
    /// the typed chars are its own, drop it otherwise.
    pub(super) fn update_type_ghost(&mut self, typed: char, seed: Option<String>) {
        let ghost = self.type_ghost.take();
        let (line, col) = self.buffer.pos_to_line_col(self.buffer.primary().head);
        let text = self.buffer.line(line);
        let byte = char_to_byte(&text, col);
        if let Some(ty) = seed.filter(|_| typed == ':') {
            // After `:` the drawn text follows the colon: ` int`.
            self.type_ghost = Some(TypeGhost { line, byte, text, insert: format!(" {ty}") });
            return;
        }
        if let Some(g) = ghost {
            if let Some(rest) = g.insert.strip_prefix(typed).filter(|r| !r.is_empty() && line == g.line && byte == g.byte + typed.len_utf8()) {
                self.type_ghost = Some(TypeGhost { line, byte, text, insert: rest.to_string() });
            }
        }
    }

    /// The ghost, when it still belongs to the caret's line as it is now.
    pub(super) fn live_type_ghost(&self) -> Option<&TypeGhost> {
        let g = self.type_ghost.as_ref()?;
        (g.line < self.buffer.len_lines() && self.buffer.line(g.line) == g.text).then_some(g)
    }

    /// Tab on a ghost: writes it. False when there is none.
    pub(super) fn accept_type_ghost(&mut self) -> bool {
        let Some(g) = self.live_type_ghost().cloned() else {
            self.type_ghost = None;
            return false;
        };
        self.type_ghost = None;
        self.buffer.insert(&g.insert);
        true
    }

    /// Quick Documentation of `pos`: where `: type` goes when `pos` is on a variable that has
    /// a value and no type hint (the char position after its name).
    pub fn type_hint_spot(&mut self, pos: usize) -> Option<usize> {
        let byte = self.buffer.rope().char_to_byte(pos.min(self.buffer.len_chars()));
        let text = self.text().to_string();
        let tree = self.python_tree()?;
        match brisk_syntax::hint_target(tree, &text, byte)? {
            brisk_syntax::HintTarget::Variable { at, insert, .. } if (at..=insert).contains(&byte) => Some(self.buffer.rope().byte_to_char(insert)),
            _ => None,
        }
    }

    /// Writes `: ty` at char position `at` (the Quick Documentation button).
    pub fn add_type_hint(&mut self, at: usize, ty: &str, cx: &mut gpui::Context<Self>) {
        self.buffer.replace_ranges(vec![(at..at, format!(": {ty}"))]);
        self.insight.doc = None;
        cx.notify();
    }
}

#[cfg(test)]
mod tests {
    use super::{literal_type, simple_type};

    #[test]
    fn literal_types() {
        let t = |v: &str| literal_type(v);
        assert_eq!(t(" 0").as_deref(), Some("int"));
        assert_eq!(t("-12_000").as_deref(), Some("int"));
        assert_eq!(t("0xff").as_deref(), Some("int"));
        assert_eq!(t("1.5").as_deref(), Some("float"));
        assert_eq!(t("1e3").as_deref(), Some("float"));
        assert_eq!(t("2j").as_deref(), Some("complex"));
        assert_eq!(t("\"x\"").as_deref(), Some("str"));
        assert_eq!(t("f'{a}'").as_deref(), Some("str"));
        assert_eq!(t("b'x'").as_deref(), Some("bytes"));
        assert_eq!(t("True").as_deref(), Some("bool"));
        assert_eq!(t("0  # count").as_deref(), Some("int"));
        assert_eq!(t("[1, 2]").as_deref(), Some("list[int]"));
        assert_eq!(t("{'a', 'b'}").as_deref(), Some("set[str]"));
        assert_eq!(t("{'a': 1, 'b': 2}").as_deref(), Some("dict[str, int]"));
        assert_eq!(t("(1, 2)").as_deref(), Some("tuple[int, ...]"));
        // Unknown or mixed: nothing.
        assert_eq!(t("[]"), None);
        assert_eq!(t("{}"), None);
        assert_eq!(t("[1, 'a']"), None);
        assert_eq!(t("None"), None);
        assert_eq!(t("set()"), None);
        assert_eq!(t("foo()"), None);
        assert_eq!(t("x + 1"), None);
        assert_eq!(t("(1)"), None);
    }

    #[test]
    fn simple_types_only() {
        assert_eq!(simple_type(": int").as_deref(), Some("int"));
        assert_eq!(simple_type("Literal[0]").as_deref(), Some("int"));
        assert_eq!(simple_type("Literal[-3]").as_deref(), Some("int"));
        assert_eq!(simple_type("Literal['a', \"b\"]").as_deref(), Some("str"));
        assert_eq!(simple_type("Literal[True]").as_deref(), Some("bool"));
        assert_eq!(simple_type("Literal[b'x']").as_deref(), Some("bytes"));
        assert_eq!(simple_type("LiteralString").as_deref(), Some("str"));
        assert_eq!(simple_type("Literal[1] | Literal[2]").as_deref(), Some("int"));
        assert_eq!(simple_type("int | None").as_deref(), Some("int | None"));
        assert_eq!(simple_type("list[int]").as_deref(), Some("list[int]"));
        assert_eq!(simple_type("dict[str, list[Literal[1]]]").as_deref(), Some("dict[str, list[int]]"));
        assert_eq!(simple_type("tuple[int, ...]").as_deref(), Some("tuple[int, ...]"));
        assert_eq!(simple_type("set[str]").as_deref(), Some("set[str]"));
        // Not simple: classes, unknowns, enum members.
        assert_eq!(simple_type("set[Unknown]"), None);
        assert_eq!(simple_type("Unknown"), None);
        assert_eq!(simple_type("CallRecordProcessService"), None);
        assert_eq!(simple_type("list[User]"), None);
        assert_eq!(simple_type("Literal[Color.RED]"), None);
        assert_eq!(simple_type("Any"), None);
        assert_eq!(simple_type("@Todo"), None);
    }
}
