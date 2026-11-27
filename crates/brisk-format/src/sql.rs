//! SQL: every clause (`SELECT`, `FROM`, `WHERE`, joins, `GROUP BY`, …) on its own line, the
//! items of a select list and the `AND` / `OR` conditions of a `WHERE` one per line below it,
//! subqueries and `CREATE TABLE` columns indented in their parentheses, a blank line between
//! statements. Keyword case, strings, quoted names and comments are kept as written.

use crate::Out;

#[derive(Debug, Clone, Copy, PartialEq)]
enum Tok<'a> {
    Word(&'a str),
    /// A string or a quoted name (`'…'`, `"…"`, `` `…` ``, `[…]`).
    Str(&'a str),
    /// `(`, `)`, `,`, `;`, `.`.
    Punct(&'a str),
    Op(&'a str),
    /// A comment and whether a line break came before it.
    Comment(&'a str, bool),
}

/// A token and whether whitespace came before it in the source.
type Spaced<'a> = (Tok<'a>, bool);

const OP_CHARS: &str = "<>=!+-*/%|&^~?:";

fn tokens(text: &str) -> Result<Vec<Spaced<'_>>, String> {
    let b = text.as_bytes();
    let line_of = |i: usize| text[..i].matches('\n').count() + 1;
    let mut out = Vec::new();
    let mut i = 0;
    let mut space = false;
    let mut newline = false;
    while i < b.len() {
        let c = b[i];
        if c.is_ascii_whitespace() {
            space = true;
            newline |= c == b'\n';
            i += 1;
            continue;
        }
        let start = i;
        let tok = if text[i..].starts_with("--") {
            i = text[i..].find('\n').map_or(b.len(), |e| i + e);
            Tok::Comment(text[start..i].trim_end(), newline || out.is_empty())
        } else if text[i..].starts_with("/*") {
            i = text[i + 2..].find("*/").map(|e| i + 2 + e + 2).ok_or_else(|| format!("line {}: unclosed comment", line_of(start)))?;
            Tok::Comment(&text[start..i], newline || out.is_empty())
        } else if matches!(c, b'\'' | b'"' | b'`' | b'[') {
            let close = if c == b'[' { b']' } else { c };
            let mut j = i + 1;
            loop {
                match b.get(j) {
                    None => return Err(format!("line {}: unclosed {}", line_of(start), if c == b'[' { "[" } else { "string" })),
                    // A doubled quote is an escaped one.
                    Some(&q) if q == close && b.get(j + 1) == Some(&close) && c != b'[' => j += 2,
                    Some(&q) if q == close => break,
                    Some(b'\\') if c == b'\'' => j += 2,
                    _ => j += 1,
                }
            }
            i = j + 1;
            Tok::Str(&text[start..i])
        } else if matches!(c, b'(' | b')' | b',' | b';' | b'.') {
            i += 1;
            Tok::Punct(&text[start..i])
        } else if c == b':' && b.get(i + 1).is_some_and(|n| n.is_ascii_alphabetic() || *n == b'_') {
            // A `:name` parameter.
            i += 1 + text[i + 1..].find(|ch: char| !(ch.is_alphanumeric() || ch == '_')).unwrap_or(b.len() - i - 1);
            Tok::Word(&text[start..i])
        } else if OP_CHARS.as_bytes().contains(&c) {
            while i < b.len() && OP_CHARS.as_bytes().contains(&b[i]) && !text[i..].starts_with("--") && !text[i..].starts_with("/*") {
                i += 1;
            }
            Tok::Op(&text[start..i])
        } else {
            i += text[i..].find(|ch: char| ch.is_whitespace() || "()[],;.'\"`".contains(ch) || OP_CHARS.contains(ch)).unwrap_or(b.len() - i).max(1);
            Tok::Word(&text[start..i])
        };
        out.push((tok, space));
        space = false;
        newline = false;
    }
    Ok(out)
}

/// Clauses that start a line, longest first.
const CLAUSES: &[&[&str]] = &[
    &["LEFT", "OUTER", "JOIN"],
    &["RIGHT", "OUTER", "JOIN"],
    &["FULL", "OUTER", "JOIN"],
    &["UNION", "ALL"],
    &["GROUP", "BY"],
    &["ORDER", "BY"],
    &["INSERT", "INTO"],
    &["DELETE", "FROM"],
    &["LEFT", "JOIN"],
    &["RIGHT", "JOIN"],
    &["INNER", "JOIN"],
    &["FULL", "JOIN"],
    &["CROSS", "JOIN"],
    &["JOIN"],
    &["SELECT"],
    &["FROM"],
    &["WHERE"],
    &["HAVING"],
    &["LIMIT"],
    &["OFFSET"],
    &["UNION"],
    &["INTERSECT"],
    &["EXCEPT"],
    &["VALUES"],
    &["SET"],
    &["RETURNING"],
];
/// Clauses that only count at the start of a statement.
const STARTERS: &[&str] = &["CREATE", "ALTER", "DROP", "UPDATE", "WITH", "INSERT", "DELETE", "MERGE"];
/// Clauses whose comma-separated items go one per line.
const LISTS: &[&str] = &["SELECT", "GROUP BY", "ORDER BY", "SET", "VALUES", "RETURNING", "WITH"];
/// Clauses whose `AND` / `OR` conditions go one per line.
const CONDITIONS: &[&str] = &["WHERE", "HAVING", "JOIN"];
/// Words after which `(` keeps a space (a function call has none).
const KEYWORDS: &[&str] = &[
    "AS",
    "IN",
    "VALUES",
    "ON",
    "USING",
    "EXISTS",
    "AND",
    "OR",
    "NOT",
    "FROM",
    "JOIN",
    "INTO",
    "TABLE",
    "WHERE",
    "SELECT",
    "ANY",
    "ALL",
    "SOME",
    "OVER",
    "WITH",
    "UNION",
    "THEN",
    "ELSE",
    "WHEN",
    "IS",
    "BY",
    "SET",
    "RETURNING",
    "KEY",
    "REFERENCES",
    "CHECK",
    "DEFAULT",
];

#[derive(PartialEq, Clone, Copy)]
enum Kind {
    /// A statement or subquery: clauses break.
    Query,
    /// `CREATE TABLE t (…)`: one item per line.
    List,
    /// Anything else: kept on its line.
    Inline,
}

struct Frame {
    kind: Kind,
    /// The indentation of its clause lines (Query) or items (List).
    depth: usize,
    /// The indentation of the line its `(` is on, for the `)`.
    outer: usize,
    clause: &'static str,
}

fn word_up(t: &Tok) -> Option<String> {
    match t {
        Tok::Word(w) => Some(w.to_ascii_uppercase()),
        _ => None,
    }
}

/// The clause starting at `k`, and how many words it takes.
fn clause_at(toks: &[Spaced], k: usize, statement_start: bool) -> Option<(&'static str, usize)> {
    let first = word_up(&toks[k].0)?;
    if statement_start {
        if let Some(s) = STARTERS.iter().find(|s| **s == first) {
            // `INSERT INTO` / `DELETE FROM` read as one.
            return Some(match CLAUSES.iter().find(|c| c.len() == 2 && c[0] == *s && toks.get(k + 1).and_then(|t| word_up(&t.0)).as_deref() == Some(c[1])) {
                Some(_) if *s == "INSERT" => ("INSERT INTO", 2),
                Some(_) if *s == "DELETE" => ("DELETE FROM", 2),
                _ => (*s, 1),
            });
        }
    }
    CLAUSES.iter().find_map(|c| {
        let all = c.iter().enumerate().all(|(n, w)| toks.get(k + n).and_then(|t| word_up(&t.0)).as_deref() == Some(*w));
        // Names that read the same as the joined words, for `LISTS` / `CONDITIONS`.
        let name: &'static str = match c.join(" ").as_str() {
            "GROUP BY" => "GROUP BY",
            "ORDER BY" => "ORDER BY",
            "INSERT INTO" => "INSERT INTO",
            "DELETE FROM" => "DELETE FROM",
            "UNION ALL" => "UNION",
            j if j.ends_with("JOIN") => "JOIN",
            _ => c[0],
        };
        all.then_some((name, c.len()))
    })
}

pub fn format(text: &str, indent: &str) -> Result<String, String> {
    let toks = tokens(text)?;
    let mut level = 0i32;
    for (t, _) in &toks {
        match t {
            Tok::Punct("(") => level += 1,
            Tok::Punct(")") => {
                level -= 1;
                if level < 0 {
                    return Err("unexpected ')'".into());
                }
            }
            _ => {}
        }
    }
    if level != 0 {
        return Err("a '(' is not closed".into());
    }
    let mut o = Out::new(indent);
    let mut stack = vec![Frame { kind: Kind::Query, depth: 0, outer: 0, clause: "" }];
    let mut statement_start = true;
    let mut new_statement = false;
    let mut must_break = false;
    let mut between = false;
    // The last token written, and whether it was a unary sign (glued to what follows).
    let mut prev: Option<Tok> = None;
    let mut glue = false;
    let mut k = 0;
    let line = |o: &mut Out, depth: usize| {
        o.depth = depth;
        o.newline();
    };
    while k < toks.len() {
        let (t, spaced) = toks[k];
        let top = stack.last().expect("root frame");
        let (kind, depth, clause) = (top.kind, top.depth, top.clause);
        if let Tok::Comment(c, before) = t {
            if new_statement && before {
                line(&mut o, 0);
                line(&mut o, 0);
                new_statement = false;
            } else if before || must_break {
                line(
                    &mut o,
                    if new_statement {
                        0
                    } else if kind == Kind::Query {
                        depth + usize::from(!statement_start)
                    } else {
                        depth
                    },
                );
            } else if !o.at_line_start() {
                o.push_str(" ");
            }
            o.push_str(c);
            must_break = c.starts_with("--") || before;
            prev = None;
            k += 1;
            continue;
        }
        if new_statement {
            line(&mut o, 0);
            line(&mut o, 0);
            new_statement = false;
            must_break = false;
        }
        let mut took = 1;
        let mut broke = false;
        // A clause starts its own line.
        if kind == Kind::Query {
            if let Some((name, n)) = clause_at(&toks, k, statement_start) {
                if !statement_start || !o.at_line_start() {
                    line(&mut o, depth);
                }
                stack.last_mut().expect("root frame").clause = name;
                took = n;
                broke = true;
                between = false;
            }
        }
        let up = word_up(&t);
        if !broke {
            let cont = match kind {
                Kind::Query => depth + 1,
                _ => depth,
            };
            if kind == Kind::Query && matches!(up.as_deref(), Some("AND" | "OR")) && CONDITIONS.contains(&clause) && !std::mem::take(&mut between) || must_break
            {
                line(&mut o, cont);
                broke = true;
            }
            if up.as_deref() == Some("BETWEEN") {
                between = true;
            }
        }
        must_break = false;
        statement_start = false;
        // Spacing.
        let no_space = o.at_line_start()
            || glue
            || matches!(t, Tok::Punct("," | ";" | ")" | ".") | Tok::Op("::"))
            || matches!(prev, Some(Tok::Punct("(" | ".") | Tok::Op("::")))
            || (t == Tok::Punct("(") && !spaced && matches!(prev, Some(Tok::Word(w)) if !KEYWORDS.contains(&w.to_ascii_uppercase().as_str())))
            || (matches!(t, Tok::Str(_)) && !spaced && matches!(prev, Some(Tok::Word(_))));
        if !no_space && !broke {
            o.push_str(" ");
        }
        glue = false;
        match t {
            Tok::Punct("(") => {
                o.push_str("(");
                let next = toks[k + 1..].iter().find(|(t, _)| !matches!(t, Tok::Comment(..))).and_then(|(t, _)| word_up(t));
                let inner = if matches!(next.as_deref(), Some("SELECT" | "WITH")) {
                    Kind::Query
                } else if kind == Kind::Query && matches!(clause, "CREATE") && !toks.get(k + 1).is_some_and(|t| t.0 == Tok::Punct(")")) {
                    Kind::List
                } else {
                    Kind::Inline
                };
                let outer = o.depth;
                stack.push(Frame { kind: inner, depth: if inner == Kind::Inline { depth } else { outer + 1 }, outer, clause: "" });
                if inner != Kind::Inline {
                    line(&mut o, outer + 1);
                    statement_start = inner == Kind::Query;
                }
            }
            Tok::Punct(")") => {
                let f = stack.pop().expect("balanced");
                if f.kind != Kind::Inline {
                    line(&mut o, f.outer);
                }
                o.push_str(")");
            }
            Tok::Punct(",") => {
                o.push_str(",");
                match kind {
                    Kind::Query if LISTS.contains(&clause) => line(&mut o, depth + 1),
                    Kind::List => line(&mut o, depth),
                    _ => {}
                }
            }
            Tok::Punct(";") => {
                o.push_str(";");
                if stack.len() == 1 {
                    stack[0].clause = "";
                    statement_start = true;
                    new_statement = true;
                }
            }
            Tok::Op(op) => {
                o.push_str(op);
                // A sign after an operator, `(`, `,` or a keyword is unary.
                glue = matches!(op, "-" | "+")
                    && match prev {
                        None | Some(Tok::Op(_) | Tok::Punct("(" | ",")) => true,
                        Some(Tok::Word(w)) => {
                            KEYWORDS.contains(&w.to_ascii_uppercase().as_str()) || CLAUSES.iter().any(|c| c[c.len() - 1].eq_ignore_ascii_case(w))
                        }
                        _ => false,
                    };
            }
            Tok::Word(_) | Tok::Str(_) | Tok::Punct(_) => {
                for (n, (w, _)) in toks[k..k + took].iter().enumerate() {
                    if n > 0 {
                        o.push_str(" ");
                    }
                    match w {
                        Tok::Word(s) | Tok::Str(s) | Tok::Punct(s) => o.push_str(s),
                        _ => {}
                    }
                }
            }
            Tok::Comment(..) => {}
        }
        prev = Some(toks[k + took - 1].0);
        k += took;
    }
    Ok(o.finish())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn query() {
        let t = "select a.id, count(*) as n, max(b.x) from a left join b on b.a_id = a.id and b.ok=1 where a.x between 1 and 5 and a.name like 'O''Neil%' or a.y in (select y from c where z = -1) group by a.id order by n desc limit 10; insert into t (a, b) values (1, 'x'), (2, 'y');";
        let want = "select a.id,\n    count(*) as n,\n    max(b.x)\nfrom a\nleft join b on b.a_id = a.id\n    and b.ok = 1\nwhere a.x between 1 and 5\n    and a.name like 'O''Neil%'\n    or a.y in (\n        select y\n        from c\n        where z = -1\n    )\ngroup by a.id\norder by n desc\nlimit 10;\n\ninsert into t (a, b)\nvalues (1, 'x'),\n    (2, 'y');\n";
        let got = format(t, "    ").unwrap();
        assert_eq!(got, want);
        assert_eq!(format(&got, "    ").unwrap(), got);
    }

    #[test]
    fn create_and_comments() {
        let t = "-- users\nCREATE TABLE users (id SERIAL PRIMARY KEY, name VARCHAR(100) NOT NULL, price DECIMAL(10,2) DEFAULT 0); /* done */\nSELECT x::int FROM t WHERE ts > now() - interval '1 day' -- recent\nAND id = :id";
        let want = "-- users\nCREATE TABLE users (\n  id SERIAL PRIMARY KEY,\n  name VARCHAR(100) NOT NULL,\n  price DECIMAL(10, 2) DEFAULT 0\n); /* done */\n\nSELECT x::int\nFROM t\nWHERE ts > now() - interval '1 day' -- recent\n  AND id = :id\n";
        let got = format(t, "  ").unwrap();
        assert_eq!(got, want);
        assert_eq!(format(&got, "  ").unwrap(), got);
        assert!(format("select (1", "  ").is_err());
        assert!(format("select 'x", "  ").is_err());
    }
}
