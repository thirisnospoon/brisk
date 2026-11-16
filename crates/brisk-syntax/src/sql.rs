//! SQL highlighting with a small hand-written lexer: keywords, strings, numbers, comments,
//! function calls. Covers PostgreSQL, MySQL and SQLite dialects well enough for consoles.

use crate::{Highlight, Span};
use std::ops::Range;

const KEYWORDS: &[&str] = &[
    "add", "all", "alter", "analyze", "and", "any", "array", "as", "asc", "attach", "auto_increment", "autoincrement",
    "begin", "between", "by", "call", "cascade", "case", "cast", "check", "collate", "column", "comment", "commit",
    "constraint", "create", "cross", "current_date", "current_time", "current_timestamp", "database", "default",
    "deferrable", "delete", "desc", "describe", "detach", "distinct", "do", "drop", "else", "end", "engine", "escape",
    "except", "exclude", "exists", "explain", "extension", "false", "fetch", "filter", "first", "following", "for",
    "foreign", "from", "full", "function", "grant", "group", "having", "if", "ignore", "ilike", "in", "index",
    "inner", "insert", "intersect", "interval", "into", "is", "join", "key", "language", "last", "lateral", "left",
    "like", "limit", "materialized", "merge", "natural", "not", "nothing", "notnull", "null", "nulls", "offset", "on",
    "or", "order", "outer", "over", "partition", "pragma", "preceding", "primary", "procedure", "range", "recursive",
    "references", "rename", "replace", "restrict", "returning", "returns", "revoke", "right", "rollback", "row", "rows",
    "savepoint", "schema", "select", "sequence", "set", "show", "similar", "some", "table", "temp", "temporary", "then",
    "to", "transaction", "trigger", "true", "truncate", "type", "unbounded", "union", "unique", "unsigned", "update",
    "use", "using", "vacuum", "values", "view", "when", "where", "window", "with", "without", "zone",
    // Types are keywords in PyCharm's SQL highlighting.
    "bigint", "bigserial", "binary", "bit", "blob", "bool", "boolean", "bytea", "char", "character", "date",
    "datetime", "decimal", "double", "enum", "float", "inet", "int", "integer", "json", "jsonb", "longtext",
    "mediumint", "mediumtext", "money", "numeric", "precision", "real", "serial", "smallint", "smallserial", "text",
    "time", "timestamp", "timestamptz", "tinyint", "tinytext", "uuid", "varbinary", "varchar", "varying", "year",
];

fn is_keyword(w: &str) -> bool {
    if w.len() > 20 {
        return false;
    }
    let mut buf = [0u8; 20];
    for (i, b) in w.bytes().enumerate() {
        buf[i] = b.to_ascii_lowercase();
    }
    let lw = std::str::from_utf8(&buf[..w.len()]).unwrap_or("");
    static SET: std::sync::OnceLock<std::collections::HashSet<&'static str>> = std::sync::OnceLock::new();
    SET.get_or_init(|| KEYWORDS.iter().copied().collect()).contains(lw)
}

/// Spans intersecting `bytes`. Lexes from the start of `text` so multi-line strings and
/// comments above the visible range are handled; SQL consoles are small.
pub fn highlight_sql(text: &str, bytes: Range<usize>) -> Vec<Span> {
    let b = text.as_bytes();
    let hi = bytes.end.min(b.len());
    let mut out = Vec::new();
    let mut i = 0;
    let push =|r: Range<usize>, k: Highlight, out: &mut Vec<Span>| {
        if r.end > bytes.start && r.start < hi {
            out.push(Span { range: r, kind: k });
        }
    };
    while i < hi {
        let c = b[i];
        let start = i;
        if c == b'-' && b.get(i + 1) == Some(&b'-') {
            while i < b.len() && b[i] != b'\n' {
                i += 1;
            }
            push(start..i, Highlight::Comment, &mut out);
        } else if c == b'/' && b.get(i + 1) == Some(&b'*') {
            i = text[i + 2..].find("*/").map(|p| i + 2 + p + 2).unwrap_or(b.len());
            push(start..i, Highlight::Comment, &mut out);
        } else if c == b'\'' {
            i += 1;
            while i < b.len() {
                if b[i] == b'\'' {
                    if b.get(i + 1) == Some(&b'\'') {
                        i += 2;
                        continue;
                    }
                    i += 1;
                    break;
                }
                if b[i] == b'\\' {
                    i += 1;
                }
                i += 1;
            }
            push(start..i.min(b.len()), Highlight::String, &mut out);
        } else if c == b'"' || c == b'`' {
            i += 1;
            while i < b.len() && b[i] != c {
                i += 1;
            }
            i = (i + 1).min(b.len());
        } else if c == b'$' && b.get(i + 1).is_some_and(|x| *x == b'$' || x.is_ascii_alphabetic()) {
            // Dollar-quoted body: highlight the delimiters only, the body is code.
            let mut j = i + 1;
            while j < b.len() && (b[j].is_ascii_alphanumeric() || b[j] == b'_') {
                j += 1;
            }
            if j < b.len() && b[j] == b'$' {
                push(i..j + 1, Highlight::String, &mut out);
                i = j + 1;
            } else {
                i += 1;
            }
        } else if c.is_ascii_digit() {
            while i < b.len() && (b[i].is_ascii_alphanumeric() || b[i] == b'.') {
                i += 1;
            }
            push(start..i, Highlight::Number, &mut out);
        } else if c.is_ascii_alphabetic() || c == b'_' || c >= 0x80 {
            while i < b.len() && (b[i].is_ascii_alphanumeric() || b[i] == b'_' || b[i] >= 0x80) {
                i += 1;
            }
            let w = &text[start..i];
            let mut j = i;
            while j < b.len() && b[j] == b' ' {
                j += 1;
            }
            let prev_dot = start > 0 && b[start - 1] == b'.';
            if !prev_dot && is_keyword(w) {
                push(start..i, Highlight::Keyword, &mut out);
            } else if b.get(j) == Some(&b'(') {
                push(start..i, Highlight::Function, &mut out);
            }
        } else {
            i += 1;
            // Keep i on a char boundary.
            while i < b.len() && (b[i] & 0xC0) == 0x80 {
                i += 1;
            }
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn lexes() {
        let t = "SELECT count(*), 'it''s' -- note\nFROM \"select\" WHERE x = 42 /* c */";
        let spans = highlight_sql(t, 0..t.len());
        let got: Vec<(&str, Highlight)> = spans.iter().map(|s| (&t[s.range.clone()], s.kind)).collect();
        assert_eq!(
            got,
            [
                ("SELECT", Highlight::Keyword),
                ("count", Highlight::Function),
                ("'it''s'", Highlight::String),
                ("-- note", Highlight::Comment),
                ("FROM", Highlight::Keyword),
                ("WHERE", Highlight::Keyword),
                ("42", Highlight::Number),
                ("/* c */", Highlight::Comment),
            ]
        );
    }
}
