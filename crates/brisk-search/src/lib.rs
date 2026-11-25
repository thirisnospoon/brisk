//! Find / Replace in Files. UI-free.
//!
//! Searches the project's file list (already filtered by .gitignore and excluded folders) on all
//! cores. No persistent index: a parallel scan of a typical Python project takes milliseconds and
//! costs zero memory while idle. A trigram index can be layered on later for huge monorepos.

use globset::{Glob, GlobSet, GlobSetBuilder};
use regex::{Regex, RegexBuilder};
use std::ops::Range;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};
use std::sync::Mutex;

const MAX_FILE: u64 = 10 * 1024 * 1024;
const MAX_LINE_SHOWN: usize = 400;

#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Options {
    pub query: String,
    pub case_sensitive: bool,
    pub whole_words: bool,
    pub regex: bool,
    /// PyCharm "File mask": `*.py, *.pyi`; `!*.txt` excludes.
    pub file_mask: String,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Match {
    /// Index into the searched file list.
    pub file: usize,
    /// 0-based line.
    pub line: usize,
    /// Char columns of the match within the line (for placing the caret).
    pub col: Range<usize>,
    /// The line (trimmed to a window around the match for very long lines).
    pub text: String,
    /// Byte range of the match within `text`.
    pub hl: Range<usize>,
}

#[derive(Clone, Debug, Default)]
pub struct Results {
    pub matches: Vec<Match>,
    pub files_with_matches: usize,
    pub files_searched: usize,
    pub truncated: bool,
}

pub struct Query {
    re: Regex,
    include: Option<GlobSet>,
    exclude: Option<GlobSet>,
    regex_mode: bool,
}

impl Query {
    pub fn new(o: &Options) -> Result<Query, String> {
        if o.query.is_empty() {
            return Err("empty query".into());
        }
        let mut pat = if o.regex { o.query.clone() } else { regex::escape(&o.query) };
        if o.whole_words {
            pat = format!(r"\b(?:{pat})\b");
        }
        let re = RegexBuilder::new(&pat).case_insensitive(!o.case_sensitive).multi_line(true).size_limit(1 << 22).build().map_err(|e| match e {
            regex::Error::Syntax(s) => s.lines().last().unwrap_or("bad regex").trim().to_string(),
            e => e.to_string(),
        })?;
        let (include, exclude) = parse_mask(&o.file_mask)?;
        Ok(Query { re, include, exclude, regex_mode: o.regex })
    }

    pub fn matches_path(&self, rel: &str) -> bool {
        let name = rel.rsplit('/').next().unwrap_or(rel);
        let hit = |g: &GlobSet| g.is_match(name) || g.is_match(rel);
        self.include.as_ref().is_none_or(hit) && !self.exclude.as_ref().is_some_and(hit)
    }

    /// All matches in one file's text.
    pub fn find_in(&self, file: usize, text: &str, out: &mut Vec<Match>, limit: usize) {
        let bytes = text.as_bytes();
        let mut line = 0usize;
        let mut scanned = 0usize; // byte up to which newlines are counted
                                  // The last match's line (start, and end at its `\n`) and a known char column in it:
                                  // many matches on one long line must not rescan it from its start each time.
        let (mut ls, mut le): (usize, Option<usize>) = (0, None);
        let (mut col_at, mut col) = (0usize, 0usize);
        for m in self.re.find_iter(text) {
            if m.start() == m.end() {
                continue; // `^`, `\b` and friends: nothing to show
            }
            line += memchr::memchr_iter(b'\n', &bytes[scanned..m.start()]).count();
            scanned = m.start();
            if le.is_none_or(|e| m.start() > e) {
                // A new line: it starts after the last one's end, so look back only to there.
                let from = le.map_or(0, |e| e + 1);
                ls = memchr::memrchr(b'\n', &bytes[from..m.start()]).map_or(from, |i| from + i + 1);
                le = Some(memchr::memchr(b'\n', &bytes[m.start()..]).map_or(text.len(), |i| m.start() + i));
                (col_at, col) = (ls, 0);
            }
            let le = le.unwrap_or(text.len());
            let le_match = m.end().min(le); // multi-line regex matches are shown on their first line
            let full = text[ls..le].trim_end_matches('\r');
            let c0 = col + text[col_at..m.start()].chars().count();
            (col_at, col) = (m.start(), c0);
            let c1 = c0 + text[m.start()..le_match].chars().count();
            // A match starting on the CR or LF itself (regex `\n` on a CRLF file) lies past
            // the trimmed line: clamp so the highlight is never reversed.
            let s = (m.start() - ls).min(full.len());
            let e = (le_match.min(ls + full.len()) - ls).max(s);
            let (shown, hl) = window(full, s, e);
            out.push(Match { file, line, col: c0..c1, text: shown, hl });
            if out.len() >= limit {
                return;
            }
        }
    }

    /// Byte ranges of all non-empty matches in `text`, at most `limit`.
    pub fn ranges(&self, text: &str, limit: usize) -> Vec<Range<usize>> {
        self.re.find_iter(text).filter(|m| m.start() != m.end()).take(limit).map(|m| m.range()).collect()
    }

    /// [`Query::ranges`] wholly inside the byte range `within` (Find "In Selection"); the
    /// limit counts only those.
    pub fn ranges_in(&self, text: &str, within: Range<usize>, limit: usize) -> Vec<Range<usize>> {
        let all = self.re.find_iter(text).filter(|m| m.start() != m.end()).map(|m| m.range());
        // Matches come in order: skip the ones before, stop at the first one past the end.
        all.skip_while(|r| r.start < within.start).take_while(|r| r.end <= within.end).take(limit).collect()
    }

    /// The replacement for the match at `range` (found by [`Query::ranges`]): `$1`/`${name}`
    /// expand in regex mode only.
    pub fn expand(&self, text: &str, range: Range<usize>, with: &str) -> String {
        if !self.regex_mode {
            return with.to_string();
        }
        match self.re.captures_at(text, range.start).filter(|c| c.get(0).is_some_and(|m| m.range() == range)) {
            Some(caps) => {
                let mut out = String::new();
                caps.expand(with, &mut out);
                out
            }
            None => with.to_string(),
        }
    }

    /// Replace every match; `$1`/`${name}` expand in regex mode only. Returns (text, count).
    pub fn replace(&self, text: &str, with: &str) -> (String, usize) {
        let n = self.re.find_iter(text).filter(|m| m.start() != m.end()).count();
        if n == 0 {
            return (text.to_string(), 0);
        }
        let out = if self.regex_mode { self.re.replace_all(text, with).into_owned() } else { self.re.replace_all(text, regex::NoExpand(with)).into_owned() };
        (out, n)
    }
}

/// Keep long lines readable: a window around the match, on char boundaries.
fn window(line: &str, s: usize, e: usize) -> (String, Range<usize>) {
    if line.len() <= MAX_LINE_SHOWN {
        return (line.to_string(), s..e);
    }
    let mut a = s.saturating_sub(MAX_LINE_SHOWN / 3);
    while !line.is_char_boundary(a) {
        a -= 1;
    }
    let mut b = (e.max(a + MAX_LINE_SHOWN)).min(line.len());
    while !line.is_char_boundary(b) {
        b += 1;
    }
    let prefix = if a > 0 { "…" } else { "" };
    let shown = format!("{prefix}{}", &line[a..b]);
    let off = prefix.len();
    (shown, s - a + off..e - a + off)
}

fn parse_mask(mask: &str) -> Result<(Option<GlobSet>, Option<GlobSet>), String> {
    let mut inc = GlobSetBuilder::new();
    let mut exc = GlobSetBuilder::new();
    let (mut ni, mut ne) = (0, 0);
    for part in mask.split([',', ';']).map(str::trim).filter(|s| !s.is_empty()) {
        let (neg, pat) = match part.strip_prefix('!') {
            Some(p) => (true, p.trim()),
            None => (false, part),
        };
        let g = Glob::new(pat).map_err(|e| format!("file mask: {e}"))?;
        if neg {
            exc.add(g);
            ne += 1;
        } else {
            inc.add(g);
            ni += 1;
        }
    }
    let build = |b: GlobSetBuilder, n: usize| -> Result<Option<GlobSet>, String> {
        if n == 0 {
            Ok(None)
        } else {
            b.build().map(Some).map_err(|e| e.to_string())
        }
    };
    Ok((build(inc, ni)?, build(exc, ne)?))
}

/// Read a file as text; None for binary, huge or non-UTF-8 files.
pub fn read_text(path: &Path) -> Option<String> {
    let meta = std::fs::metadata(path).ok()?;
    if meta.len() > MAX_FILE {
        return None;
    }
    let bytes = std::fs::read(path).ok()?;
    if memchr::memchr(0, &bytes[..bytes.len().min(8192)]).is_some() {
        return None;
    }
    String::from_utf8(bytes).ok()
}

/// Search `files` (relative to `root`) on all cores. Stops early when `cancel` is set
/// (a newer query superseded this one) or `limit` matches are found.
pub fn search(root: &Path, files: &[String], q: &Query, limit: usize, cancel: &AtomicBool) -> Results {
    let next = AtomicUsize::new(0);
    let found = AtomicUsize::new(0);
    let searched = AtomicUsize::new(0);
    let per_file: Mutex<Vec<(usize, Vec<Match>)>> = Mutex::new(Vec::new());
    let threads = std::thread::available_parallelism().map(|n| n.get()).unwrap_or(4).min(16);
    std::thread::scope(|s| {
        for _ in 0..threads {
            s.spawn(|| {
                let mut local: Vec<(usize, Vec<Match>)> = Vec::new();
                loop {
                    if cancel.load(Ordering::Relaxed) || found.load(Ordering::Relaxed) >= limit {
                        break;
                    }
                    let i = next.fetch_add(1, Ordering::Relaxed);
                    let Some(rel) = files.get(i) else { break };
                    if !q.matches_path(rel) {
                        continue;
                    }
                    let Some(text) = read_text(&root.join(rel)) else { continue };
                    searched.fetch_add(1, Ordering::Relaxed);
                    let mut v = Vec::new();
                    q.find_in(i, &text, &mut v, limit);
                    if !v.is_empty() {
                        found.fetch_add(v.len(), Ordering::Relaxed);
                        local.push((i, v));
                    }
                }
                per_file.lock().unwrap().extend(local);
            });
        }
    });
    let mut per_file = per_file.into_inner().unwrap();
    per_file.sort_by_key(|(i, _)| *i);
    let files_with_matches = per_file.len();
    let mut matches: Vec<Match> = per_file.into_iter().flat_map(|(_, v)| v).collect();
    let truncated = matches.len() >= limit;
    matches.truncate(limit);
    Results { matches, files_with_matches, files_searched: searched.into_inner(), truncated }
}

pub fn abs(root: &Path, rel: &str) -> PathBuf {
    root.join(rel)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn opts(q: &str) -> Options {
        Options { query: q.into(), ..Default::default() }
    }

    #[test]
    fn literal_case_words_regex() {
        let text = "def find_user(name):\n    User = name.upper()\n    return user_cache[name]\n";
        let mut v = Vec::new();
        Query::new(&opts("user")).unwrap().find_in(0, text, &mut v, 100);
        assert_eq!(v.len(), 3, "case-insensitive by default");
        assert_eq!((v[0].line, v[0].col.clone()), (0, 9..13));
        assert_eq!(v[1].line, 1);
        assert_eq!(&v[1].text[v[1].hl.clone()], "User");

        let mut v = Vec::new();
        Query::new(&Options { case_sensitive: true, ..opts("User") }).unwrap().find_in(0, text, &mut v, 100);
        assert_eq!(v.len(), 1);

        let mut v = Vec::new();
        Query::new(&Options { whole_words: true, ..opts("name") }).unwrap().find_in(0, text, &mut v, 100);
        assert_eq!(v.len(), 3);
        assert!(v.iter().all(|m| &m.text[m.hl.clone()] == "name"));

        let mut v = Vec::new();
        Query::new(&Options { regex: true, ..opts(r"def \w+\(") }).unwrap().find_in(0, text, &mut v, 100);
        assert_eq!(v.len(), 1);
        assert!(Query::new(&Options { regex: true, ..opts("(") }).is_err());
        // Literal mode escapes regex syntax.
        let mut v = Vec::new();
        Query::new(&opts("upper()")).unwrap().find_in(0, text, &mut v, 100);
        assert_eq!(v.len(), 1);
    }

    #[test]
    fn unicode_columns_and_long_lines() {
        let text = "x = 'привет мир'\n";
        let mut v = Vec::new();
        Query::new(&opts("мир")).unwrap().find_in(0, text, &mut v, 10);
        assert_eq!(v[0].col, 12..15);
        let long = format!("{}needle{}", "ж".repeat(500), "a".repeat(500));
        let mut v = Vec::new();
        Query::new(&opts("needle")).unwrap().find_in(0, &long, &mut v, 10);
        assert!(v[0].text.len() <= MAX_LINE_SHOWN + 10);
        assert_eq!(&v[0].text[v[0].hl.clone()], "needle");
        assert_eq!(v[0].col.start, 500);
    }

    #[test]
    fn many_matches_on_long_lines() {
        // Columns and lines stay right while the line is tracked incrementally.
        let text = format!("{}\nжa a\n\nxa{}", "ba".repeat(50_000), "a".repeat(3));
        let mut v = Vec::new();
        Query::new(&opts("a")).unwrap().find_in(0, &text, &mut v, usize::MAX);
        assert_eq!(v.len(), 50_000 + 2 + 4);
        assert_eq!((v[0].line, v[0].col.clone()), (0, 1..2));
        assert_eq!((v[49_999].line, v[49_999].col.clone()), (0, 99_999..100_000));
        assert_eq!(v[50_000..].iter().map(|m| (m.line, m.col.start)).collect::<Vec<_>>(), [(1, 1), (1, 3), (3, 1), (3, 2), (3, 3), (3, 4)]);
        // A multi-line match, then one on the line it ends on.
        let mut v = Vec::new();
        Query::new(&Options { regex: true, ..opts(r"b\nc|d") }).unwrap().find_in(0, "ab\ncd\n", &mut v, 10);
        assert_eq!(v.iter().map(|m| (m.line, m.col.clone())).collect::<Vec<_>>(), [(0, 1..2), (1, 1..2)]);
    }

    #[test]
    fn ranges_inside_a_scope() {
        let q = Query::new(&opts("ab")).unwrap();
        let text = "ab ab ab ab";
        assert_eq!(q.ranges_in(text, 2..7, 10), [Range { start: 3, end: 5 }]);
        assert_eq!(q.ranges_in(text, 3..11, 2), [3..5, 6..8]);
        assert_eq!(q.ranges_in(text, 0..text.len(), usize::MAX), q.ranges(text, usize::MAX));
    }

    #[test]
    fn regex_newline_on_crlf_text() {
        let text = "a = 1\r\n\r\nb = 2\r\n";
        for q in [r"\n", r"\r", r"\n\s*\n", r"1\r\n"] {
            let mut v = Vec::new();
            Query::new(&Options { regex: true, ..opts(q) }).unwrap().find_in(0, text, &mut v, 10);
            for m in &v {
                assert!(m.hl.start <= m.hl.end && m.hl.end <= m.text.len(), "{q}: {m:?}");
            }
        }
    }

    #[test]
    fn masks_and_replace() {
        let q = Query::new(&Options { file_mask: "*.py, !test_*".into(), ..opts("x") }).unwrap();
        assert!(q.matches_path("app/main.py"));
        assert!(!q.matches_path("tests/test_main.py"));
        assert!(!q.matches_path("README.md"));

        let q = Query::new(&opts("a.b")).unwrap();
        assert_eq!(q.replace("a.b axb a.b", "$0"), ("$0 axb $0".to_string(), 2));
        let q = Query::new(&Options { regex: true, ..opts(r"(\w+)@(\w+)") }).unwrap();
        assert_eq!(q.replace("me@host", "$2:$1").0, "host:me");
    }

    #[test]
    fn parallel_search_on_disk() {
        let d = std::env::temp_dir().join(format!("brisk-search-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&d);
        std::fs::create_dir_all(&d).unwrap();
        let mut files = Vec::new();
        for i in 0..200 {
            let name = format!("f{i:03}.py");
            std::fs::write(d.join(&name), if i % 10 == 0 { "import os\nTOKEN = 1\n" } else { "pass\n" }).unwrap();
            files.push(name);
        }
        std::fs::write(d.join("bin.dat"), b"TOKEN\0\0").unwrap();
        files.push("bin.dat".into());
        let r = search(&d, &files, &Query::new(&opts("token")).unwrap(), 1000, &AtomicBool::new(false));
        assert_eq!(r.matches.len(), 20);
        assert_eq!(r.files_with_matches, 20);
        assert!(r.matches.windows(2).all(|w| w[0].file < w[1].file), "sorted by file order");
        assert_eq!(r.matches[0].line, 1);
        let r = search(&d, &files, &Query::new(&opts("token")).unwrap(), 5, &AtomicBool::new(false));
        assert!(r.truncated && r.matches.len() == 5);
        let _ = std::fs::remove_dir_all(&d);
    }
}
