//! Clickable file locations in console output: Python tracebacks
//! (`File "C:\x\app.py", line 12, in f`) and `path.py:12` / `path.py:12:5` (pytest, ruff,
//! mypy, warnings).

use std::ops::Range;

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Link {
    /// Character columns in the line (not bytes), for painting on a terminal grid.
    pub cols: Range<usize>,
    pub path: String,
    /// 1-based.
    pub line: usize,
    /// 1-based column of `path.py:12:5`; 0 when the link has none.
    pub col: usize,
}

fn char_col(s: &str, byte: usize) -> usize {
    s[..byte].chars().count()
}

const EXTS: &[&str] = &[".py", ".pyi", ".pyw", ".js", ".ts", ".tsx", ".jsx", ".toml", ".cfg", ".txt", ".html", ".sql"];

fn digits(s: &str) -> (usize, usize) {
    let n = s.bytes().take_while(u8::is_ascii_digit).count();
    (s[..n].parse().unwrap_or(0), n)
}

pub fn find_links(line: &str) -> Vec<Link> {
    let mut out = Vec::new();
    // Traceback frames.
    let mut from = 0;
    while let Some(i) = line[from..].find("File \"") {
        let s = from + i + 6;
        let Some(e) = line[s..].find('"').map(|e| s + e) else { break };
        let rest = &line[e + 1..];
        if let Some(r) = rest.strip_prefix(", line ") {
            let (n, len) = digits(r);
            if n > 0 && !line[s..e].starts_with('<') {
                let end = e + 1 + ", line ".len() + len;
                out.push(Link { cols: char_col(line, s - 1)..char_col(line, end), path: line[s..e].to_string(), line: n, col: 0 });
            }
        }
        from = e + 1;
    }
    if !out.is_empty() {
        return out;
    }
    // `path.ext:line` — the path is the run of non-space characters before ":<digits>".
    let b = line.as_bytes();
    let mut i = 0;
    while i < b.len() {
        if b[i] == b':' && b.get(i + 1).is_some_and(u8::is_ascii_digit) {
            let before = &line[..i];
            if let Some(ext) = EXTS.iter().find(|x| before.ends_with(*x)) {
                let start = before.rfind(|c: char| c.is_whitespace() || c == '"' || c == '\'' || c == '(').map(|p| p + 1).unwrap_or(0);
                // A Windows drive letter "C:" is part of the path, not a separator.
                let path = &line[start..i];
                if path.len() > ext.len() {
                    let (n, len) = digits(&line[i + 1..]);
                    let mut end = i + 1 + len;
                    // `:12:5`: a column too.
                    let (col, clen) = match line[end..].strip_prefix(':') {
                        Some(r) => digits(r),
                        None => (0, 0),
                    };
                    if clen > 0 {
                        end += 1 + clen;
                    }
                    out.push(Link { cols: char_col(line, start)..char_col(line, end), path: path.to_string(), line: n.max(1), col });
                    i = end;
                    continue;
                }
            }
        }
        i += 1;
    }
    out
}

/// `http://` and `https://` links in a line: character columns and the URL (trailing
/// punctuation and closing brackets left out).
pub fn find_urls(line: &str) -> Vec<(Range<usize>, String)> {
    let mut out = Vec::new();
    let mut from = 0;
    while let Some(i) = ["http://", "https://"].iter().filter_map(|p| line[from..].find(p)).min() {
        let s = from + i;
        let len = line[s..].find(|c: char| c.is_whitespace() || matches!(c, '"' | '\'' | '<' | '>' | '`')).unwrap_or(line.len() - s);
        let mut url = &line[s..s + len];
        url = url.trim_end_matches(['.', ',', ';', ':', '!', '?']);
        if url.ends_with(')') && !url.contains('(') {
            url = &url[..url.len() - 1];
        }
        if url.len() > "https://".len() {
            out.push((char_col(line, s)..char_col(line, s + url.len()), url.to_string()));
        }
        from = s + len.max(1);
    }
    out
}
