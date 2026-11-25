//! Statement splitting for consoles: `;` outside strings, quoted identifiers, comments and
//! PostgreSQL dollar-quoted bodies.

use crate::Driver;
use std::ops::Range;

/// What the scanner met.
enum Piece {
    /// A string literal or quoted identifier, a comment, or a dollar-quoted body.
    Opaque,
    Semicolon,
    Code,
}

/// Walks `sql` piece by piece: `f(range, piece)` for each.
fn scan(sql: &str, driver: Driver, mut f: impl FnMut(Range<usize>, Piece)) {
    let b = sql.as_bytes();
    let mut i = 0;
    while i < b.len() {
        let end = match b[i] {
            b'\'' | b'"' | b'`' => skip_quoted(b, i, backslash_escapes(b, i, driver)),
            b'-' if b.get(i + 1) == Some(&b'-') => skip_line(b, i),
            b'/' if b.get(i + 1) == Some(&b'*') => match sql[i + 2..].find("*/") {
                Some(p) => i + 2 + p + 2,
                None => b.len(),
            },
            b'$' if driver == Driver::Postgres => skip_dollar(sql, i),
            b';' => {
                f(i..i + 1, Piece::Semicolon);
                i += 1;
                continue;
            }
            _ => {
                let n = sql[i..].chars().next().map_or(1, char::len_utf8);
                f(i..i + n, Piece::Code);
                i += n;
                continue;
            }
        };
        if end == i + 1 && b[i] == b'$' {
            f(i..end, Piece::Code);
        } else {
            f(i..end, Piece::Opaque);
        }
        i = end;
    }
}

/// Does a backslash escape the next character in the string starting at `i`? Always in
/// MySQL strings; in PostgreSQL only in `E'…'` (standard strings take `\` literally).
fn backslash_escapes(b: &[u8], i: usize, driver: Driver) -> bool {
    match driver {
        Driver::Mysql => b[i] != b'`',
        Driver::Postgres => {
            let word = |c: u8| c.is_ascii_alphanumeric() || c == b'_';
            b[i] == b'\'' && i >= 1 && matches!(b[i - 1], b'e' | b'E') && (i < 2 || !word(b[i - 2]))
        }
        Driver::Sqlite => false,
    }
}

/// Byte ranges of the statements in `sql`, trimmed, without the trailing `;`.
/// Comment-only fragments are dropped.
pub fn split_statements(sql: &str, driver: Driver) -> Vec<Range<usize>> {
    let mut out = Vec::new();
    let mut start = 0;
    scan(sql, driver, |r, p| {
        if let Piece::Semicolon = p {
            push(sql, start..r.start, &mut out);
            start = r.end;
        }
    });
    push(sql, start..sql.len(), &mut out);
    out
}

/// `sql` with string literals, quoted identifiers and comments blanked out (same length),
/// for keyword checks that must not match inside them.
pub(crate) fn code_only(sql: &str, driver: Driver) -> String {
    let mut out = String::with_capacity(sql.len());
    scan(sql, driver, |r, p| match p {
        Piece::Opaque => out.extend(std::iter::repeat_n(' ', r.len())),
        _ => out.push_str(&sql[r]),
    });
    out
}

fn push(sql: &str, r: Range<usize>, out: &mut Vec<Range<usize>>) {
    let s = &sql[r.clone()];
    let lead = s.len() - s.trim_start().len();
    let trail = s.len() - s.trim_end().len();
    let r = r.start + lead..r.end - trail;
    if r.start < r.end && has_code(&sql[r.clone()]) {
        out.push(r);
    }
}

/// Anything besides comments and whitespace?
fn has_code(s: &str) -> bool {
    let mut t = s.trim_start();
    loop {
        if t.is_empty() {
            return false;
        }
        if let Some(r) = t.strip_prefix("--") {
            t = r.split_once('\n').map(|x| x.1).unwrap_or("").trim_start();
        } else if let Some(r) = t.strip_prefix("/*") {
            t = r.split_once("*/").map(|x| x.1).unwrap_or("").trim_start();
        } else {
            return true;
        }
    }
}

fn skip_quoted(b: &[u8], i: usize, backslash: bool) -> usize {
    let q = b[i];
    let mut j = i + 1;
    while j < b.len() {
        if backslash && b[j] == b'\\' && j + 1 < b.len() {
            j += 2;
            continue;
        }
        if b[j] == q {
            if b.get(j + 1) == Some(&q) {
                j += 2;
                continue;
            }
            return j + 1;
        }
        j += 1;
    }
    b.len()
}

fn skip_line(b: &[u8], i: usize) -> usize {
    match b[i..].iter().position(|&c| c == b'\n') {
        Some(p) => i + p + 1,
        None => b.len(),
    }
}

/// `$$ ... $$` or `$tag$ ... $tag$`; a lone `$1` parameter is just skipped.
fn skip_dollar(sql: &str, i: usize) -> usize {
    let b = sql.as_bytes();
    let mut j = i + 1;
    while j < b.len() && (b[j].is_ascii_alphanumeric() || b[j] == b'_') {
        j += 1;
    }
    if j < b.len() && b[j] == b'$' && !(j > i + 1 && b[i + 1].is_ascii_digit()) {
        let tag = &sql[i..=j];
        return match sql[j + 1..].find(tag) {
            Some(p) => j + 1 + p + tag.len(),
            None => b.len(),
        };
    }
    i + 1
}

/// The statement the caret is in, PyCharm-style: the caret may also sit right after the `;`
/// or on blank space after a statement on the same line.
pub fn statement_at(sql: &str, offset: usize, driver: Driver) -> Option<Range<usize>> {
    let stmts = split_statements(sql, driver);
    if let Some(r) = stmts.iter().find(|r| r.start <= offset && offset <= r.end) {
        return Some(r.clone());
    }
    // After a statement: up to the end of its line (past `;` and spaces).
    let mut best = None;
    for r in &stmts {
        if r.end <= offset {
            let between = &sql[r.end..offset.min(sql.len())];
            if !between.contains('\n') {
                best = Some(r.clone());
            }
        }
    }
    best.or_else(|| {
        // On an empty line: the next statement below, then the one above.
        stmts.iter().find(|r| r.start >= offset).cloned().or_else(|| stmts.last().cloned())
    })
}
