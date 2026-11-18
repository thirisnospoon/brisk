//! Parameter Info without ty: the signature of a `def` (or a class's `__init__`) in the same
//! file, or of a common builtin, with the active parameter from the commas typed so far.

use super::Editor;
use brisk_lsp::Signature;

/// Builtins as `name(params)`, the way typeshed spells them (simplified).
const BUILTINS: &[&str] = &[
    "print(*values, sep=' ', end='\\n', file=None, flush=False)",
    "len(obj, /)",
    "range(start, stop=None, step=1, /)",
    "open(file, mode='r', buffering=-1, encoding=None, errors=None, newline=None, closefd=True, opener=None)",
    "input(prompt='', /)",
    "sorted(iterable, /, *, key=None, reverse=False)",
    "enumerate(iterable, start=0)",
    "zip(*iterables, strict=False)",
    "isinstance(obj, class_or_tuple, /)",
    "issubclass(cls, class_or_tuple, /)",
    "int(x=0, /, base=10)",
    "str(object='', encoding='utf-8', errors='strict')",
    "float(x=0, /)",
    "bool(x=False, /)",
    "list(iterable=(), /)",
    "tuple(iterable=(), /)",
    "dict(**kwargs)",
    "set(iterable=(), /)",
    "max(iterable, *, key=None, default=...)",
    "min(iterable, *, key=None, default=...)",
    "sum(iterable, /, start=0)",
    "abs(x, /)",
    "round(number, ndigits=None)",
    "map(func, *iterables)",
    "filter(function, iterable, /)",
    "any(iterable, /)",
    "all(iterable, /)",
    "getattr(obj, name, default=..., /)",
    "setattr(obj, name, value, /)",
    "hasattr(obj, name, /)",
    "repr(obj, /)",
    "format(value, format_spec='', /)",
    "iter(object, sentinel=..., /)",
    "next(iterator, default=..., /)",
    "type(object, /)",
    "super(type=..., object_or_type=..., /)",
    "divmod(x, y, /)",
    "pow(base, exp, mod=None)",
    "reversed(sequence, /)",
    "chr(i, /)",
    "ord(c, /)",
    "hex(number, /)",
    "id(obj, /)",
    "hash(obj, /)",
    "callable(obj, /)",
];

/// Split `params` at top-level commas.
fn split_params(params: &str) -> Vec<String> {
    let mut out = Vec::new();
    let mut depth = 0i32;
    let mut cur = String::new();
    let mut quote: Option<char> = None;
    for c in params.chars() {
        match quote {
            Some(q) => {
                if c == q {
                    quote = None;
                }
                cur.push(c);
                continue;
            }
            None if c == '\'' || c == '"' => quote = Some(c),
            None => {}
        }
        match c {
            '(' | '[' | '{' => depth += 1,
            ')' | ']' | '}' => depth -= 1,
            ',' if depth == 0 => {
                out.push(cur.trim().to_string());
                cur.clear();
                continue;
            }
            _ => {}
        }
        cur.push(c);
    }
    if !cur.trim().is_empty() {
        out.push(cur.trim().to_string());
    }
    out
}

/// The call around the end of `before` (text up to the caret): (callee name, whether it was
/// called on an object (`x.f(`), arguments typed so far).
pub fn call_at(before: &str) -> Option<(String, bool, Vec<String>)> {
    let chars: Vec<char> = before.chars().collect();
    let mut depth = 0i32;
    let mut i = chars.len();
    let limit = chars.len().saturating_sub(4000);
    let open = loop {
        if i <= limit {
            return None;
        }
        i -= 1;
        match chars[i] {
            ')' | ']' | '}' => depth += 1,
            '[' | '{' if depth > 0 => depth -= 1,
            '[' | '{' => return None,
            '(' if depth > 0 => depth -= 1,
            '(' => break i,
            // A new statement before any open paren.
            ':' | ';' if depth == 0 && chars[..i].iter().rev().take_while(|c| **c != '\n').all(|c| c.is_whitespace()) => return None,
            _ => {}
        }
    };
    let mut s = open;
    while s > 0 && (chars[s - 1].is_alphanumeric() || chars[s - 1] == '_') {
        s -= 1;
    }
    if s == open {
        return None;
    }
    let name: String = chars[s..open].iter().collect();
    if name.starts_with(|c: char| c.is_ascii_digit()) || ["if", "while", "for", "return", "and", "or", "not", "in", "elif", "assert", "yield", "with"].contains(&name.as_str()) {
        return None;
    }
    let method = s > 0 && chars[s - 1] == '.';
    let args: String = chars[open + 1..].iter().collect();
    let mut parts = split_params(&args);
    // A trailing comma means a new, empty argument is being typed.
    if args.trim_end().ends_with(',') || args.trim().is_empty() {
        parts.push(String::new());
    }
    Some((name, method, parts))
}

/// `def name(...)` (or `class name` with an `__init__`) in `text`: its parameters and return
/// annotation.
fn def_in(text: &str, name: &str) -> Option<(Vec<String>, String, bool)> {
    let find = |prefix: &str, from: usize| -> Option<usize> {
        let mut at = from;
        while let Some(off) = text[at..].find(prefix) {
            let p = at + off;
            let line_start = text[..p].rfind('\n').map_or(0, |n| n + 1);
            if text[line_start..p].trim().is_empty() || text[line_start..p].trim() == "async" {
                return Some(p);
            }
            at = p + prefix.len();
        }
        None
    };
    let (start, is_class) = match find(&format!("def {name}("), 0) {
        Some(p) => (p + 4 + name.len(), false),
        None => {
            let c = find(&format!("class {name}"), 0)?;
            let init = text[c..].find("def __init__(")? + c;
            (init + "def __init__".len(), true)
        }
    };
    // `start` is at the `(`; find its match.
    let mut depth = 0;
    let mut end = None;
    for (i, ch) in text[start..].char_indices() {
        match ch {
            '(' | '[' | '{' => depth += 1,
            ')' | ']' | '}' => {
                depth -= 1;
                if depth == 0 {
                    end = Some(start + i);
                    break;
                }
            }
            _ => {}
        }
    }
    let end = end?;
    let params: Vec<String> = split_params(&text[start + 1..end].split_whitespace().collect::<Vec<_>>().join(" "));
    let rest = &text[end + 1..];
    let ret = rest.split(':').next().unwrap_or("").trim().strip_prefix("->").map(|r| format!(" -> {}", r.trim())).unwrap_or_default();
    Some((params, ret, is_class))
}

/// The signature for the call at the end of `before`, looking in `text` then the builtins.
pub fn local_signature(text: &str, before: &str) -> Option<Signature> {
    let (name, method, args) = call_at(before)?;
    let (mut params, ret, is_class) = match def_in(text, &name) {
        Some(d) => d,
        None => {
            let b = BUILTINS.iter().find(|b| b.starts_with(&format!("{name}(")))?;
            (split_params(&b[name.len() + 1..b.len() - 1]), String::new(), false)
        }
    };
    if (method || is_class) && params.first().is_some_and(|p| p == "self" || p == "cls") {
        params.remove(0);
    }
    let shown: Vec<&String> = params.iter().filter(|p| p.as_str() != "/" && p.as_str() != "*").collect();
    let label = format!("({}){ret}", shown.iter().map(|s| s.as_str()).collect::<Vec<_>>().join(", "));
    let mut ranges = Vec::new();
    let mut at = 1;
    for p in &shown {
        let n = p.chars().count();
        ranges.push(at..at + n);
        at += n + 2;
    }
    // The argument being typed: a keyword picks its parameter, else its position
    // (a *args parameter takes all the rest).
    let current = args.last().map(|a| a.trim()).unwrap_or("");
    let pname = |p: &str| p.split([':', '=']).next().unwrap_or("").trim().trim_start_matches('*').to_string();
    let active = match current.split_once('=').filter(|(k, _)| !k.contains(['(', '[', '"', '\''])) {
        Some((k, _)) => shown.iter().position(|p| pname(p) == k.trim()),
        None => {
            let pos = args.len().saturating_sub(1);
            let star = shown.iter().position(|p| p.starts_with('*') && !p.starts_with("**"));
            match star {
                Some(s) if pos >= s => Some(s),
                _ => (pos < shown.len()).then_some(pos),
            }
        }
    };
    Some(Signature { label, params: ranges, active, doc: String::new() })
}

impl Editor {
    /// Parameter Info without ty, for the call around the caret.
    pub fn local_signature(&self) -> Option<Signature> {
        let text = self.text_string();
        let caret = self.buffer.primary().head;
        let before: String = self.buffer.rope().slice(..caret).to_string();
        local_signature(&text, &before)
    }
}

#[cfg(test)]
mod tests {
    use super::local_signature;

    fn sig(text: &str, before: &str) -> Option<(String, Option<usize>)> {
        local_signature(text, before).map(|s| {
            let active = s.active.map(|i| s.label.chars().skip(s.params[i].start).take(s.params[i].len()).collect::<String>());
            (s.label, active.map(|a| a.len()).and(s.active))
        })
    }

    #[test]
    fn signatures() {
        let text = "def area(width: int, height: int = 1) -> int:\n    return width * height\n\nclass Box:\n    def __init__(self, size,\n                 color='red'):\n        pass\n";
        assert_eq!(sig(text, "area("), Some(("(width: int, height: int = 1) -> int".into(), Some(0))));
        assert_eq!(sig(text, "area(3, "), Some(("(width: int, height: int = 1) -> int".into(), Some(1))));
        assert_eq!(sig(text, "area(height="), Some(("(width: int, height: int = 1) -> int".into(), Some(1))));
        assert_eq!(sig(text, "x = Box(1, "), Some(("(size, color='red')".into(), Some(1))));
        assert_eq!(sig(text, "print(a, b, "), Some(("(*values, sep=' ', end='\\n', file=None, flush=False)".into(), Some(0))));
        assert_eq!(sig(text, "len(area(1), "), Some(("(obj)".into(), None)));
        assert_eq!(sig(text, "area(len(x"), Some(("(obj)".into(), Some(0))));
        assert_eq!(sig(text, "unknown("), None);
        assert_eq!(sig(text, "x = (1, "), None);
    }
}
