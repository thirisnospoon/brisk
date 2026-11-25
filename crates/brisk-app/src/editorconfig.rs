//! `.editorconfig` support (https://editorconfig.org): indent style and size, line endings,
//! trailing whitespace and the final newline. Files are read from the file's folder upwards
//! until one says `root = true`; nearer files and later sections win.

use std::path::Path;

#[derive(Clone, Debug, Default, PartialEq)]
pub struct EditorConfig {
    /// Some(true) = tabs, Some(false) = spaces.
    pub indent_tabs: Option<bool>,
    pub indent_size: Option<usize>,
    pub tab_width: Option<usize>,
    /// Some(true) = CRLF, Some(false) = LF.
    pub crlf: Option<bool>,
    pub trim_trailing_whitespace: Option<bool>,
    pub insert_final_newline: Option<bool>,
    pub charset: Option<String>,
    /// `max_line_length`: where the right margin is drawn (`off`: none).
    pub max_line_length: Option<Option<usize>>,
}

impl EditorConfig {
    /// The text one indent level inserts.
    pub fn indent_unit(&self) -> Option<String> {
        if self.indent_tabs == Some(true) {
            return Some("\t".into());
        }
        match self.indent_size.or(self.tab_width) {
            Some(n) => Some(" ".repeat(n.clamp(1, 16))),
            None => (self.indent_tabs == Some(false)).then(|| "    ".into()),
        }
    }

    /// What the save does to `text` (lines joined by '\n').
    pub fn fix_on_save(&self, text: &str, keep_line: Option<usize>) -> String {
        let mut out = text.to_string();
        if self.trim_trailing_whitespace == Some(true) {
            out =
                out.split('\n').enumerate().map(|(i, l)| if Some(i) == keep_line { l } else { l.trim_end_matches([' ', '\t']) }).collect::<Vec<_>>().join("\n");
        }
        match self.insert_final_newline {
            Some(true) if !out.is_empty() && !out.ends_with('\n') => out.push('\n'),
            Some(false) => {
                while out.ends_with('\n') {
                    out.pop();
                }
            }
            _ => {}
        }
        out
    }
}

/// Settings for `file`.
pub fn for_file(file: &Path) -> EditorConfig {
    let mut configs = Vec::new();
    let mut dir = file.parent();
    while let Some(d) = dir {
        if let Ok(text) = std::fs::read_to_string(d.join(".editorconfig")) {
            let root = parse(&text).0;
            configs.push((d.to_path_buf(), text));
            if root {
                break;
            }
        }
        dir = d.parent();
    }
    let mut out = EditorConfig::default();
    // Farthest first, so nearer files override.
    for (d, text) in configs.iter().rev() {
        let rel = file.strip_prefix(d).unwrap_or(file).to_string_lossy().replace('\\', "/");
        for (glob, props) in parse(text).1 {
            if glob_matches(&glob, &rel) {
                for (k, v) in props {
                    apply(&mut out, &k, &v);
                }
            }
        }
    }
    if out.indent_tabs == Some(true) && out.indent_size.is_none() {
        out.indent_size = out.tab_width;
    }
    out
}

fn apply(c: &mut EditorConfig, k: &str, v: &str) {
    let unset = v == "unset";
    let num = v.parse::<usize>().ok().filter(|n| *n > 0);
    let flag = match v {
        "true" => Some(true),
        "false" => Some(false),
        _ => None,
    };
    match k {
        "indent_style" => {
            c.indent_tabs = if unset {
                None
            } else {
                match v {
                    "tab" => Some(true),
                    "space" => Some(false),
                    _ => c.indent_tabs,
                }
            }
        }
        "indent_size" => {
            c.indent_size = if unset {
                None
            } else if v == "tab" {
                c.tab_width.or(c.indent_size)
            } else {
                num.or(c.indent_size)
            }
        }
        "tab_width" => c.tab_width = if unset { None } else { num.or(c.tab_width) },
        "end_of_line" => {
            c.crlf = if unset {
                None
            } else {
                match v {
                    "crlf" => Some(true),
                    "lf" => Some(false),
                    _ => c.crlf,
                }
            }
        }
        "trim_trailing_whitespace" => c.trim_trailing_whitespace = if unset { None } else { flag.or(c.trim_trailing_whitespace) },
        "insert_final_newline" => c.insert_final_newline = if unset { None } else { flag.or(c.insert_final_newline) },
        "max_line_length" => {
            c.max_line_length = if unset {
                None
            } else if v == "off" {
                Some(None)
            } else {
                num.map(Some).or(c.max_line_length)
            }
        }
        "charset" => c.charset = if unset { None } else { Some(v.to_string()) },
        _ => {}
    }
}

/// (root = true?, sections of (glob, [(key, value)])); keys and values lowercased.
fn parse(text: &str) -> (bool, Vec<(String, Vec<(String, String)>)>) {
    let mut root = false;
    let mut sections: Vec<(String, Vec<(String, String)>)> = Vec::new();
    for line in text.lines() {
        let l = line.trim();
        if l.is_empty() || l.starts_with('#') || l.starts_with(';') {
            continue;
        }
        if l.starts_with('[') && l.ends_with(']') {
            sections.push((l[1..l.len() - 1].trim().to_string(), Vec::new()));
            continue;
        }
        let Some((k, v)) = l.split_once('=').or_else(|| l.split_once(':')) else { continue };
        let (k, v) = (k.trim().to_lowercase(), v.trim().to_lowercase());
        match sections.last_mut() {
            Some(s) => s.1.push((k, v)),
            None if k == "root" => root = v == "true",
            None => {}
        }
    }
    (root, sections)
}

/// EditorConfig globs: `*`, `**`, `?`, `[abc]`, `[!abc]`, `{a,b}`, `{1..3}`. A glob without
/// '/' matches the file name in any folder.
pub fn glob_matches(glob: &str, rel: &str) -> bool {
    let glob = glob.strip_prefix('/').map(str::to_string).unwrap_or_else(|| if glob.contains('/') { glob.to_string() } else { format!("**/{glob}") });
    expand_braces(&glob).iter().any(|g| matches(g.as_bytes(), rel.as_bytes()))
}

fn expand_braces(g: &str) -> Vec<String> {
    let Some(open) = g.find('{') else { return vec![g.to_string()] };
    let mut depth = 0;
    let mut close = None;
    for (i, c) in g[open..].char_indices() {
        match c {
            '{' => depth += 1,
            '}' => {
                depth -= 1;
                if depth == 0 {
                    close = Some(open + i);
                    break;
                }
            }
            _ => {}
        }
    }
    let Some(close) = close else { return vec![g.to_string()] };
    let (pre, inner, post) = (&g[..open], &g[open + 1..close], &g[close + 1..]);
    let mut alts: Vec<String> = Vec::new();
    if let Some((a, b)) = inner.split_once("..") {
        if let (Ok(a), Ok(b)) = (a.parse::<i64>(), b.parse::<i64>()) {
            alts = (a.min(b)..=a.max(b)).map(|n| n.to_string()).collect();
        }
    }
    if alts.is_empty() {
        let mut depth = 0;
        let mut cur = String::new();
        for c in inner.chars() {
            match c {
                '{' => depth += 1,
                '}' => depth -= 1,
                ',' if depth == 0 => {
                    alts.push(std::mem::take(&mut cur));
                    continue;
                }
                _ => {}
            }
            cur.push(c);
        }
        alts.push(cur);
    }
    alts.iter().flat_map(|a| expand_braces(&format!("{pre}{a}{post}"))).collect()
}

fn matches(p: &[u8], s: &[u8]) -> bool {
    if p.is_empty() {
        return s.is_empty();
    }
    if p.starts_with(b"**/") {
        // Zero or more folders.
        if matches(&p[3..], s) {
            return true;
        }
        return s.iter().enumerate().any(|(i, &c)| c == b'/' && matches(&p[3..], &s[i + 1..]));
    }
    if p.starts_with(b"**") {
        return (0..=s.len()).any(|i| matches(&p[2..], &s[i..]));
    }
    match p[0] {
        b'*' => (0..=s.len()).take_while(|&i| i == 0 || s[i - 1] != b'/').any(|i| matches(&p[1..], &s[i..])),
        b'?' => !s.is_empty() && s[0] != b'/' && matches(&p[1..], &s[1..]),
        b'[' => {
            let Some(end) = p.iter().position(|&c| c == b']') else { return !s.is_empty() && s[0] == b'[' && matches(&p[1..], &s[1..]) };
            if s.is_empty() {
                return false;
            }
            let (neg, set) = match p.get(1) {
                Some(b'!') | Some(b'^') => (true, &p[2..end]),
                _ => (false, &p[1..end]),
            };
            let mut hit = false;
            let mut i = 0;
            while i < set.len() {
                if i + 2 < set.len() && set[i + 1] == b'-' {
                    hit |= set[i] <= s[0] && s[0] <= set[i + 2];
                    i += 3;
                } else {
                    hit |= set[i] == s[0];
                    i += 1;
                }
            }
            hit != neg && matches(&p[end + 1..], &s[1..])
        }
        b'\\' if p.len() > 1 => !s.is_empty() && s[0] == p[1] && matches(&p[2..], &s[1..]),
        c => !s.is_empty() && s[0] == c && matches(&p[1..], &s[1..]),
    }
}

/// The indent a file already uses, when nothing configures it: tabs, or the most common
/// step between indented lines (2, 4 or 8 spaces).
pub fn detect_indent(text: &str) -> Option<String> {
    let (mut tabs, mut spaces) = (0, 0);
    let mut steps = [0usize; 9];
    let mut prev = 0usize;
    for line in text.lines().take(2000) {
        if line.trim().is_empty() {
            continue;
        }
        if line.starts_with('\t') {
            tabs += 1;
            continue;
        }
        let n = line.len() - line.trim_start_matches(' ').len();
        if n > 0 {
            spaces += 1;
        }
        if n > prev && n - prev <= 8 {
            steps[n - prev] += 1;
        }
        prev = n;
    }
    if tabs > spaces && tabs > 0 {
        return Some("\t".into());
    }
    let best = [2usize, 4, 8].into_iter().max_by_key(|&k| (steps[k], k == 4))?;
    (steps[best] > 0).then(|| " ".repeat(best))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn globs() {
        assert!(glob_matches("*", "a/b/c.py"));
        assert!(glob_matches("*.py", "pkg/mod.py"));
        assert!(!glob_matches("*.py", "pkg/mod.pyc"));
        assert!(glob_matches("*.{js,ts}", "web/app.ts"));
        assert!(glob_matches("lib/**.js", "lib/a/b.js"));
        assert!(!glob_matches("lib/*.js", "lib/a/b.js"));
        assert!(glob_matches("/Makefile", "Makefile"));
        assert!(!glob_matches("/Makefile", "sub/Makefile"));
        assert!(glob_matches("[Mm]akefile", "Makefile"));
        assert!(glob_matches("file{1..3}.txt", "file2.txt"));
        assert!(glob_matches("{package.json,.travis.yml}", ".travis.yml"));
    }

    #[test]
    fn files_and_overrides() {
        let dir = std::env::temp_dir().join(format!("brisk-ec-{}", std::process::id()));
        let sub = dir.join("web");
        std::fs::create_dir_all(&sub).unwrap();
        std::fs::write(
            dir.join(".editorconfig"),
            "root = true\n\n[*]\nindent_style = space\nindent_size = 4\nend_of_line = lf\ninsert_final_newline = true\ntrim_trailing_whitespace = true\n\n[Makefile]\nindent_style = tab\n",
        )
        .unwrap();
        std::fs::write(sub.join(".editorconfig"), "[*.js]\nindent_size = 2\nend_of_line = crlf\n").unwrap();
        let py = for_file(&dir.join("a.py"));
        assert_eq!(py.indent_unit().as_deref(), Some("    "));
        assert_eq!(py.crlf, Some(false));
        let js = for_file(&sub.join("app.js"));
        assert_eq!(js.indent_unit().as_deref(), Some("  "));
        assert_eq!(js.crlf, Some(true));
        assert_eq!(for_file(&dir.join("Makefile")).indent_unit().as_deref(), Some("\t"));
        assert_eq!(py.fix_on_save("a = 1   \nb = 2\t", None), "a = 1\nb = 2\n");
        assert_eq!(py.fix_on_save("a = 1   \nb", Some(0)), "a = 1   \nb\n");
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn detection() {
        assert_eq!(detect_indent("def f():\n  if x:\n    y\n  z\n").as_deref(), Some("  "));
        assert_eq!(detect_indent("def f():\n    return 1\n").as_deref(), Some("    "));
        assert_eq!(detect_indent("all:\n\tcc x\n\tcc y\n").as_deref(), Some("\t"));
        assert_eq!(detect_indent("x = 1\n"), None);
    }
}
