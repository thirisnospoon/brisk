//! Django / Jinja2 template tags over HTML highlighting: `{% tag args %}`, `{{ value|filter }}`
//! and `{# comment #}`. Inside a tag the HTML colors (an attribute value is a string to
//! tree-sitter-html) give way to the tag's own: delimiters and the tag name as keywords,
//! strings, numbers, filters as functions, comments as comments.

use crate::Highlight;
use std::ops::Range;

/// How far before the painted range a tag may start, and how long one may be.
const REACH: usize = 4096;

const WORDS: &[&str] = &["in", "not", "and", "or", "is", "as", "with", "only", "by", "if", "else", "true", "false", "none", "True", "False", "None"];

pub(crate) fn paint(text: &str, lo: usize, paint: &mut [Option<Highlight>]) {
    let hi = (lo + paint.len()).min(text.len());
    let mut i = lo.saturating_sub(REACH);
    while !text.is_char_boundary(i) {
        i -= 1;
    }
    let bytes = text.as_bytes();
    let mut set = |r: Range<usize>, k: Option<Highlight>| {
        let (s, e) = (r.start.max(lo), r.end.min(hi));
        if s < e {
            for p in &mut paint[s - lo..e - lo] {
                *p = k;
            }
        }
    };
    while i + 1 < hi {
        if bytes[i] != b'{' || !matches!(bytes[i + 1], b'%' | b'{' | b'#') {
            i += 1;
            continue;
        }
        let close = match bytes[i + 1] {
            b'%' => "%}",
            b'{' => "}}",
            _ => "#}",
        };
        let limit = (i + 2 + REACH).min(text.len());
        let Some(body_len) = text.get(i + 2..limit).and_then(|t| t.find(close)) else {
            i += 2;
            continue;
        };
        let (open, end) = (i, i + 2 + body_len + 2);
        i = end;
        if end <= lo {
            continue;
        }
        if close == "#}" {
            set(open..end, Some(Highlight::Comment));
            continue;
        }
        set(open..end, None);
        // Delimiters with Jinja's whitespace control (`{%-`, `-%}`).
        let mut b = open + 2;
        if matches!(bytes.get(b), Some(b'-' | b'+')) {
            b += 1;
        }
        let mut e = end - 2;
        if e > b && matches!(bytes[e - 1], b'-' | b'+') {
            e -= 1;
        }
        set(open..b, Some(Highlight::Keyword));
        set(e..end, Some(Highlight::Keyword));
        let mut j = b;
        let mut first_word = close == "%}";
        let mut after_pipe = false;
        while j < e {
            let c = bytes[j];
            if c == b'"' || c == b'\'' {
                let s = j;
                j += 1;
                while j < e && bytes[j] != c {
                    j += 1;
                }
                j = (j + 1).min(e);
                set(s..j, Some(Highlight::String));
                first_word = false;
                after_pipe = false;
            } else if c.is_ascii_digit() {
                let s = j;
                while j < e && (bytes[j].is_ascii_digit() || bytes[j] == b'.') {
                    j += 1;
                }
                set(s..j, Some(Highlight::Number));
                first_word = false;
            } else if c.is_ascii_alphabetic() || c == b'_' {
                let s = j;
                while j < e && (bytes[j].is_ascii_alphanumeric() || bytes[j] == b'_') {
                    j += 1;
                }
                let word = &text[s..j];
                if first_word || WORDS.contains(&word) {
                    set(s..j, Some(Highlight::Keyword));
                } else if after_pipe {
                    set(s..j, Some(Highlight::Function));
                }
                first_word = false;
                after_pipe = false;
            } else {
                if c == b'|' {
                    after_pipe = true;
                } else if !c.is_ascii_whitespace() {
                    after_pipe = false;
                }
                j += 1;
            }
        }
    }
}
