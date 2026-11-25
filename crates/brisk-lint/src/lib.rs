//! Code inspections through ruff, UI-free: finding the executable, checking unsaved text or
//! the whole project, PyCharm-style severities, quick fixes and `# noqa` suppression.

use brisk_platform::python::Layout;
use serde::Deserialize;
use std::io::Write;
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};

/// How a problem is shown, like PyCharm's highlighting levels.
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub enum Severity {
    /// Red wavy underline: syntax errors, undefined names.
    Error,
    /// Yellow wavy underline.
    Warning,
    /// Greyed-out text: unused imports and variables.
    Unused,
}

/// A position: 0-based line and 0-based column in characters (Unicode scalar values, the
/// unit ruff reports and the editor's rope uses).
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct Pos {
    pub line: usize,
    pub col: usize,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Edit {
    pub start: Pos,
    pub end: Pos,
    pub content: String,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Fix {
    pub message: String,
    /// ruff marks some fixes "unsafe" (may change behavior); PyCharm's ruff plugin still
    /// offers them, labelled.
    pub safe: bool,
    pub edits: Vec<Edit>,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Diagnostic {
    pub file: PathBuf,
    /// Rule code (`F401`), or `invalid-syntax`.
    pub code: String,
    pub message: String,
    pub severity: Severity,
    pub start: Pos,
    pub end: Pos,
    pub fix: Option<Fix>,
    /// Line where a `# noqa` comment suppresses it (0-based); `None` for syntax errors.
    pub noqa_line: Option<usize>,
}

impl Diagnostic {
    /// "Undefined name `x` (F821)", the way the tooltip and Problems view show it.
    pub fn title(&self) -> String {
        if self.code == "invalid-syntax" {
            format!("Syntax error: {}", self.message)
        } else {
            format!("{} ({})", self.message, self.code)
        }
    }
}

pub fn severity_of(code: &str) -> Severity {
    match code {
        "invalid-syntax" | "E999" | "F821" | "F822" | "F823" | "F706" | "F704" | "F702" | "F701" | "PLE0117" | "PLE0118" => Severity::Error,
        "F401" | "F841" | "F811" | "F842" | "ARG001" | "ARG002" | "ARG003" | "ARG004" | "ARG005" => Severity::Unused,
        _ => Severity::Warning,
    }
}

#[derive(Deserialize)]
struct JLoc {
    row: usize,
    column: usize,
}

impl JLoc {
    fn pos(&self) -> Pos {
        Pos { line: self.row.saturating_sub(1), col: self.column.saturating_sub(1) }
    }
}

#[derive(Deserialize)]
struct JEdit {
    content: String,
    location: JLoc,
    end_location: JLoc,
}

#[derive(Deserialize)]
struct JFix {
    applicability: String,
    edits: Vec<JEdit>,
    message: Option<String>,
}

#[derive(Deserialize)]
struct JDiag {
    code: Option<String>,
    message: String,
    location: JLoc,
    end_location: JLoc,
    filename: String,
    fix: Option<JFix>,
    noqa_row: Option<usize>,
}

/// Parse `ruff check --output-format=json`.
pub fn parse(json: &str) -> Result<Vec<Diagnostic>, String> {
    let raw: Vec<JDiag> = serde_json::from_str(json).map_err(|e| format!("unexpected ruff output: {e}"))?;
    let mut out: Vec<Diagnostic> = raw
        .into_iter()
        .map(|d| {
            let code = d.code.unwrap_or_else(|| "invalid-syntax".into());
            let fix = d.fix.filter(|f| f.applicability != "display-only").map(|f| Fix {
                message: f.message.unwrap_or_else(|| format!("Fix {code}")),
                safe: f.applicability == "safe",
                edits: f.edits.into_iter().map(|e| Edit { start: e.location.pos(), end: e.end_location.pos(), content: e.content }).collect(),
            });
            Diagnostic {
                file: PathBuf::from(d.filename),
                severity: severity_of(&code),
                code,
                message: d.message,
                start: d.location.pos(),
                end: d.end_location.pos(),
                fix,
                noqa_line: d.noqa_row.map(|r| r.saturating_sub(1)),
            }
        })
        .collect();
    out.sort_by(|a, b| (&a.file, a.start, a.severity).cmp(&(&b.file, b.start, b.severity)));
    Ok(out)
}

fn exe(name: &str) -> String {
    Layout::host().exe(name)
}

/// A tool shipped with Brisk: `lib/<name>.exe` next to the executable (the portable
/// package), or `vendor/tools/<name>.exe` above it (development builds; see
/// scripts/fetch-tools.ps1).
pub fn bundled_tool(name: &str) -> Option<PathBuf> {
    let exe_path = std::env::current_exe().ok()?;
    let dir = exe_path.parent()?;
    let p = dir.join("lib").join(exe(name));
    if p.is_file() {
        return Some(p);
    }
    dir.ancestors().map(|a| a.join("vendor").join("tools").join(exe(name))).find(|p| p.is_file())
}

/// ruff the way PyCharm's ruff integration finds it: next to the project interpreter (its
/// virtualenv or Scripts folder), then on `PATH`; `None` also tries the bundled copy.
pub fn find_ruff(python: Option<&Path>) -> Option<PathBuf> {
    if let Some(dir) = python.and_then(Path::parent) {
        if let Some(p) = Layout::host().tool_dirs(dir).into_iter().map(|d| d.join(exe("ruff"))).find(|p| p.is_file()) {
            return Some(p);
        }
    }
    if python.is_some() {
        return None;
    }
    let on_path = std::env::var_os("PATH").and_then(|path| std::env::split_paths(&path).map(|d| d.join(exe("ruff"))).find(|p| p.is_file()));
    on_path.or_else(|| bundled_tool("ruff"))
}

fn command(ruff: &Path, root: &Path) -> Command {
    let mut c = Command::new(ruff);
    c.current_dir(root).stdin(Stdio::piped()).stdout(Stdio::piped()).stderr(Stdio::piped());
    brisk_platform::process::no_window(&mut c);
    c
}

fn run(mut c: Command, stdin: Option<&str>) -> Result<String, String> {
    let mut child = c.spawn().map_err(|e| format!("cannot start ruff: {e}"))?;
    brisk_platform::process::adopt(&child);
    if let Some(text) = stdin {
        let mut i = child.stdin.take().ok_or("no stdin")?;
        let text = text.to_string();
        // Write from a thread: ruff may start answering before it has read everything.
        std::thread::spawn(move || {
            let _ = i.write_all(text.as_bytes());
        });
    } else {
        drop(child.stdin.take());
    }
    let out = child.wait_with_output().map_err(|e| e.to_string())?;
    let stdout = String::from_utf8_lossy(&out.stdout).into_owned();
    // Exit code 1 means "problems found"; 2 is a real failure (bad config, crash).
    if out.status.code() == Some(2) || (stdout.trim().is_empty() && !out.status.success()) {
        let err = String::from_utf8_lossy(&out.stderr);
        return Err(err.lines().find(|l| !l.trim().is_empty()).unwrap_or("ruff failed").trim().to_string());
    }
    Ok(stdout)
}

/// Check one file's current (possibly unsaved) text. Uses the project's ruff configuration
/// (`pyproject.toml`, `ruff.toml`) for that path.
pub fn check_text(ruff: &Path, root: &Path, file: &Path, text: &str) -> Result<Vec<Diagnostic>, String> {
    let mut c = command(ruff, root);
    c.args(["check", "--output-format=json", "--exit-zero", "--force-exclude", "--stdin-filename"]).arg(file).arg("-");
    let mut v = parse(&run(c, Some(text))?)?;
    for d in &mut v {
        d.file = file.to_path_buf();
    }
    Ok(v)
}

/// Check the whole project (the Problems view's "Project Errors" tab).
pub fn check_project(ruff: &Path, root: &Path) -> Result<Vec<Diagnostic>, String> {
    let mut c = command(ruff, root);
    c.args(["check", "--output-format=json", "--exit-zero", "."]);
    let mut v = parse(&run(c, None)?)?;
    for d in &mut v {
        if d.file.is_relative() {
            d.file = root.join(&d.file);
        }
    }
    Ok(v)
}

/// "Fix all problems in file" (optionally only one rule): ruff's own `--fix` on the text.
/// Returns the fixed text.
pub fn fix_all(ruff: &Path, root: &Path, file: &Path, text: &str, only: Option<&str>) -> Result<String, String> {
    let mut c = command(ruff, root);
    c.args(["check", "--fix", "--exit-zero", "--quiet", "--force-exclude"]);
    if let Some(code) = only {
        c.args(["--select", code]);
    }
    c.arg("--stdin-filename").arg(file).arg("-");
    run(c, Some(text))
}

/// Reformat Code: `ruff format` on the text, or only on `lines` (first and last, 0-based)
/// when there is a selection. Returns the formatted text.
pub fn format_text(ruff: &Path, root: &Path, file: &Path, text: &str, lines: Option<(usize, usize)>) -> Result<String, String> {
    let mut c = command(ruff, root);
    c.args(["format", "--quiet", "--force-exclude"]);
    if let Some((a, z)) = lines {
        // 1-based line:column; column 1 of the line after the last one ends the range.
        c.arg(format!("--range={}:1-{}:1", a + 1, z + 2));
    }
    c.arg("--stdin-filename").arg(file).arg("-");
    run(c, Some(text))
}

/// Optimize Imports: drop unused imports and sort them (ruff rules F401 and I001).
pub fn optimize_imports(ruff: &Path, root: &Path, file: &Path, text: &str) -> Result<String, String> {
    fix_all(ruff, root, file, text, Some("F401,I001"))
}

/// What Optimize Imports did, compared by import statements: "Removed 2 unused imports,
/// sorted the rest", "Imports sorted", "Imports are already optimized".
pub fn imports_summary(before: &str, after: &str) -> String {
    fn names(t: &str) -> usize {
        // Count imported names: `import a, b` is 2, `from x import (a, b)` is 2.
        let mut n = 0;
        let mut in_paren = false;
        for l in t.lines() {
            let s = l.split('#').next().unwrap_or("").trim();
            if in_paren {
                n += s.trim_end_matches(')').split(',').filter(|p| !p.trim().is_empty()).count();
                in_paren = !s.contains(')');
                continue;
            }
            if let Some(rest) = s.strip_prefix("import ") {
                n += rest.split(',').count();
            } else if s.starts_with("from ") {
                if let Some((_, rest)) = s.split_once(" import ") {
                    let rest = rest.trim().trim_start_matches('(');
                    n += rest.trim_end_matches(')').split(',').filter(|p| !p.trim().is_empty()).count();
                    in_paren = s.contains('(') && !s.contains(')');
                }
            }
        }
        n
    }
    let (b, a) = (before.replace("\r\n", "\n"), after.replace("\r\n", "\n"));
    if a == b {
        return "Imports are already optimized".into();
    }
    match names(&b).saturating_sub(names(&a)) {
        0 => "Imports sorted".into(),
        1 => "Removed 1 unused import; imports sorted".into(),
        n => format!("Removed {n} unused imports; imports sorted"),
    }
}

/// Apply edits to text (positions refer to the original text). Used by tests and as the
/// reference for the editor, which applies the same edits through its undo history.
pub fn apply_edits(text: &str, edits: &[Edit]) -> String {
    let line_starts: Vec<usize> = std::iter::once(0).chain(text.char_indices().filter(|(_, c)| *c == '\n').map(|(i, _)| i + 1)).collect();
    let byte_of = |p: Pos| -> usize {
        let Some(&ls) = line_starts.get(p.line) else { return text.len() };
        let line = &text[ls..];
        ls + line.char_indices().nth(p.col).map(|(b, _)| b).unwrap_or_else(|| line.find('\n').unwrap_or(line.len()))
    };
    let mut sorted: Vec<&Edit> = edits.iter().collect();
    sorted.sort_by_key(|e| std::cmp::Reverse(e.start));
    let mut s = text.to_string();
    // Applied back to front: an edit reaching into one already applied (duplicates, or the
    // same file listed twice in a workspace edit) would use stale offsets; skip it.
    let mut limit = text.len();
    for e in sorted {
        let (a, b) = (byte_of(e.start), byte_of(e.end));
        let b = b.max(a);
        if b > limit {
            continue;
        }
        s.replace_range(a..b, &e.content);
        limit = a;
    }
    s
}

/// The line with a `# noqa: CODE` comment added (or the code appended to an existing one),
/// PyCharm's "Suppress for line".
pub fn noqa_line(line: &str, code: &str) -> String {
    if let Some(i) = line.find("# noqa") {
        let (head, tail) = line.split_at(i);
        let rest = &tail["# noqa".len()..];
        if let Some(codes) = rest.strip_prefix(':') {
            let codes = codes.trim();
            if codes.split([',', ' ']).any(|c| c.trim() == code) {
                return line.to_string();
            }
            return format!("{head}# noqa: {codes}, {code}");
        }
        // A bare `# noqa` already suppresses everything.
        return line.to_string();
    }
    let trimmed = line.trim_end();
    format!("{trimmed}  # noqa: {code}")
}

/// ty rule names are kebab-case (`invalid-assignment`); ruff codes are `F401`-style.
pub fn is_ty_code(code: &str) -> bool {
    !code.is_empty() && code.chars().all(|c| c.is_ascii_lowercase() || c == '-')
}

/// The line with a `# ty: ignore[rule]` comment added, or the rule appended to an existing one.
pub fn ty_ignore_line(line: &str, rule: &str) -> String {
    const TAG: &str = "# ty: ignore[";
    if let Some(i) = line.find(TAG) {
        let inner_start = i + TAG.len();
        if let Some(close) = line[inner_start..].find(']') {
            let inner = &line[inner_start..inner_start + close];
            if inner.split(',').any(|r| r.trim() == rule) {
                return line.to_string();
            }
            let sep = if inner.trim().is_empty() { "" } else { ", " };
            return format!("{}{sep}{rule}{}", &line[..inner_start + close], &line[inner_start + close..]);
        }
    }
    format!("{}  # ty: ignore[{rule}]", line.trim_end())
}

#[cfg(test)]
mod tests;
