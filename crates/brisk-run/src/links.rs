//! Clickable file locations in console output: Python tracebacks
//! (`File "C:\x\app.py", line 12, in f`) and `path.py:12` (pytest, ruff, warnings).

use std::ops::Range;

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Link {
    /// Character columns in the line (not bytes), for painting on a terminal grid.
    pub cols: Range<usize>,
    pub path: String,
    /// 1-based.
    pub line: usize,
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
                out.push(Link { cols: char_col(line, s - 1)..char_col(line, end), path: line[s..e].to_string(), line: n });
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
                    out.push(Link { cols: char_col(line, start)..char_col(line, i + 1 + len), path: path.to_string(), line: n.max(1) });
                    i += 1 + len;
                    continue;
                }
            }
        }
        i += 1;
    }
    out
}
