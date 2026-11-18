//! TODO / FIXME items in comments of the project's text files (`.gitignore` is respected
//! through the project file list). In plain-text files (.txt, .md, .rst) any line counts.

use std::path::{Path, PathBuf};

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Item {
    pub path: PathBuf,
    /// 0-based line and the character column of the keyword.
    pub line: usize,
    pub col: usize,
    /// From the keyword to the end of the comment line.
    pub text: String,
}

const KEYWORDS: [&str; 2] = ["todo", "fixme"];
const MAX_FILE: u64 = 2 * 1024 * 1024;

fn plain_text(path: &Path) -> bool {
    path.extension().and_then(|e| e.to_str()).is_some_and(|e| matches!(e.to_ascii_lowercase().as_str(), "txt" | "md" | "rst"))
}

/// Byte offset where a comment starts on this line, if any.
fn comment_start(line: &str) -> Option<usize> {
    let t = line.trim_start();
    let indent = line.len() - t.len();
    // Continuation lines of block comments and docstrings.
    if t.starts_with('*') || t.starts_with(';') {
        return Some(indent);
    }
    ["#", "//", "/*", "<!--", "--", "\"\"\"", "'''"].iter().filter_map(|m| line.find(m)).min()
}

fn is_word_char(c: char) -> bool {
    c.is_alphanumeric() || c == '_'
}

/// The TODO / FIXME on one line: (byte offset of the keyword, text from it).
pub fn find_in_line(line: &str, plain: bool) -> Option<(usize, String)> {
    let from = if plain { 0 } else { comment_start(line)? };
    let lower = line.to_lowercase();
    // to_lowercase keeps byte offsets for ASCII keywords only when lengths match.
    if lower.len() != line.len() {
        return None;
    }
    let mut best: Option<usize> = None;
    for k in KEYWORDS {
        let mut start = from;
        while let Some(i) = lower[start..].find(k).map(|i| i + start) {
            let before = lower[..i].chars().next_back();
            let after = lower[i + k.len()..].chars().next();
            if !before.is_some_and(is_word_char) && !after.is_some_and(is_word_char) {
                best = Some(best.map_or(i, |b: usize| b.min(i)));
                break;
            }
            start = i + k.len();
        }
    }
    let i = best?;
    let mut text = line[i..].trim_end().to_string();
    for end in ["*/", "-->", "\"\"\"", "'''"] {
        if let Some(e) = text.find(end) {
            text.truncate(e);
        }
    }
    let text = text.trim_end().chars().take(240).collect();
    Some((i, text))
}

pub fn scan_text(path: &Path, text: &str) -> Vec<Item> {
    let plain = plain_text(path);
    text.lines()
        .enumerate()
        .filter_map(|(n, l)| {
            let (byte, t) = find_in_line(l, plain)?;
            Some(Item { path: path.to_path_buf(), line: n, col: l[..byte].chars().count(), text: t })
        })
        .collect()
}

/// One file from disk; binary and very large files are skipped.
pub fn scan_file(path: &Path) -> Vec<Item> {
    if std::fs::metadata(path).map(|m| m.len() > MAX_FILE).unwrap_or(true) {
        return Vec::new();
    }
    let Ok(bytes) = std::fs::read(path) else { return Vec::new() };
    if bytes.iter().take(8000).any(|&b| b == 0) {
        return Vec::new();
    }
    scan_text(path, &String::from_utf8_lossy(&bytes))
}

/// The whole project, grouped by file in path order.
pub fn scan_project(root: &Path) -> Vec<(PathBuf, Vec<Item>)> {
    let list = brisk_project::FileList::scan(root);
    list.files
        .iter()
        .filter_map(|rel| {
            let p = root.join(rel);
            let items = scan_file(&p);
            (!items.is_empty()).then_some((p, items))
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn keywords_in_comments_only() {
        assert_eq!(find_in_line("x = 1  # TODO: fix this", false), Some((9, "TODO: fix this".into())));
        assert_eq!(find_in_line("    // fixme later", false), Some((7, "fixme later".into())));
        assert_eq!(find_in_line("/* TODO tidy */ int x;", false), Some((3, "TODO tidy".into())));
        assert_eq!(find_in_line("todo = load()  # plain comment", false), None);
        assert_eq!(find_in_line("# todos and fixmes are not words", false), None);
        assert_eq!(find_in_line("call_todo()", false), None);
        assert_eq!(find_in_line(" * FIXME: in a block", false), Some((3, "FIXME: in a block".into())));
        assert_eq!(find_in_line("- [ ] TODO write docs", true), Some((6, "TODO write docs".into())));
    }

    #[test]
    fn project_respects_gitignore() {
        let dir = std::env::temp_dir().join(format!("brisk-todo-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(dir.join("build")).unwrap();
        std::fs::create_dir_all(dir.join(".git")).unwrap();
        std::fs::write(dir.join(".gitignore"), "build/\n").unwrap();
        std::fs::write(dir.join("a.py"), "x = 1\n# TODO first\ny = 2  # FIXME: second\n").unwrap();
        std::fs::write(dir.join("build/gen.py"), "# TODO ignored\n").unwrap();
        std::fs::write(dir.join("b.bin"), b"\0\0# TODO binary").unwrap();
        let r = scan_project(&dir);
        assert_eq!(r.len(), 1, "{r:?}");
        assert_eq!(r[0].1.iter().map(|i| (i.line, i.col, i.text.as_str())).collect::<Vec<_>>(), vec![(1, 2, "TODO first"), (2, 9, "FIXME: second")]);
        let _ = std::fs::remove_dir_all(&dir);
    }
}
