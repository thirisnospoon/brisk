//! Debugging with debugpy, as in PyCharm: Shift+F9 / Ctrl+Shift+F9 start, breakpoints
//! (Ctrl+F8 or a click on the line numbers; right click for a condition), F8 / F7 / Shift+F8
//! step, F9 resume, Alt+F9 run to cursor, Ctrl+F2 stop, Alt+F8 evaluate. The Debug window
//! shows frames, variables (expandable) and the console with an evaluate line.

use super::{BottomTool, PopupItem, PopupKind, Target, Workspace};
use crate::editor::breakpoints::Breakpoint;
use crate::editor::Editor;
use crate::theme::{Theme, mono_font};
use brisk_dap::{Client, Event, Frame, Launch, SourceBreakpoint, Variable};
use brisk_keymap::Action;
use brisk_run::RunConfig;
use futures::StreamExt;
use gpui::prelude::*;
use gpui::{div, px, Context, Entity, FocusHandle, ScrollHandle, Task, Window};
use serde_json::{json, Value};
use std::path::{Path, PathBuf};
use std::sync::Arc;

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct StoredBp {
    pub path: PathBuf,
    pub line: usize,
    pub condition: Option<String>,
    pub enabled: bool,
}

impl StoredBp {
    fn breakpoint(&self) -> Breakpoint {
        Breakpoint { enabled: self.enabled, ..Breakpoint::new(self.condition.clone()) }
    }
}

/// View Breakpoints (Ctrl+Shift+F8).
pub struct BpDialog {
    pub sel: usize,
    /// Typing goes into the condition: Space is a space, Esc / Enter leave the field.
    pub editing: bool,
}

/// A row of View Breakpoints.
#[derive(Clone, Debug, PartialEq)]
pub enum BpRow {
    Raised,
    Uncaught,
    /// An exception breakpoint for one class (index into `exc_classes`).
    Class(usize, ExcClass),
    Line(PathBuf, usize, Breakpoint),
}

/// A Python exception breakpoint for a class: `ValueError`, `mypkg.MyError`.
#[derive(Clone, Debug, PartialEq)]
pub struct ExcClass {
    pub name: String,
    /// Stop where it is raised, even if handled.
    pub raised: bool,
    /// Stop when nothing handles it.
    pub uncaught: bool,
    pub enabled: bool,
}

impl ExcClass {
    fn new() -> Self {
        Self { name: String::new(), raised: true, uncaught: true, enabled: true }
    }
}

/// setExceptionBreakpoints arguments. With class breakpoints everything goes as
/// exceptionOptions (debugpy then ignores `filters`); "Python Exceptions" alone means all.
fn exception_args(muted: bool, raised: bool, uncaught: bool, classes: &[ExcClass]) -> Value {
    if muted {
        return json!({"filters": []});
    }
    let mut filters = Vec::new();
    if raised {
        filters.push("raised");
    }
    if uncaught {
        filters.push("uncaught");
    }
    let classes: Vec<&ExcClass> = classes.iter().filter(|c| c.enabled && !c.name.trim().is_empty() && (c.raised || c.uncaught)).collect();
    if classes.is_empty() {
        return json!({"filters": filters});
    }
    let all = json!({"names": ["Python Exceptions"]});
    let mut opts = Vec::new();
    // Raised covers uncaught: the raise happens first.
    if raised {
        opts.push(json!({"path": [all], "breakMode": "always"}));
    } else if uncaught {
        opts.push(json!({"path": [all], "breakMode": "unhandled"}));
    }
    for c in classes {
        let mode = if c.raised { "always" } else { "unhandled" };
        opts.push(json!({"path": [all, {"names": [c.name.trim()]}], "breakMode": mode}));
    }
    json!({"filters": filters, "exceptionOptions": opts})
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum DebugTab {
    Debugger,
    Console,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum State {
    Starting,
    Running,
    Paused(i64),
    Ended,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Line {
    Out,
    Err,
    Info,
    Input,
    Result,
}

pub struct Session {
    client: Arc<Client>,
    pub name: String,
    pub state: State,
    _events: Task<()>,
    /// Run to Cursor: a breakpoint that is not the user's.
    temp: Option<(PathBuf, usize)>,
    pub exit_code: Option<i64>,
    /// Attached to a running process: Stop detaches and leaves it running.
    pub attached: bool,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum RowKind {
    Scope,
    Var,
    WatchHeader,
    /// A watch: its index in `Debug::watches`.
    Watch(usize),
}

#[derive(Clone, Debug)]
pub struct VarRow {
    pub depth: usize,
    pub var: Variable,
    pub expanded: bool,
    pub kind: RowKind,
    /// The `variablesReference` of the container (setVariable), 0 for scopes and watches.
    pub parent: i64,
}

impl VarRow {
    /// A return value debugpy shows after Step Over / Step Out ("(return) f"): "↩ f()".
    fn return_label(&self) -> Option<String> {
        (self.depth == 1).then(|| return_label(&self.var.name)).flatten()
    }

    fn var(depth: usize, var: Variable, parent: i64) -> Self {
        Self { depth, var, expanded: false, kind: RowKind::Var, parent }
    }
}

/// What the variables tree is editing.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum EditTarget {
    NewWatch,
    Watch(usize),
    /// Set Value of this row.
    Value(usize),
}

#[derive(Clone, Debug)]
pub struct VarEdit {
    pub target: EditTarget,
    pub text: String,
}

pub struct Debug {
    pub session: Option<Session>,
    /// Breakpoints of files that are not open.
    pub stored: Vec<StoredBp>,
    pub console: Vec<(String, Line)>,
    pub input: String,
    history: Vec<String>,
    history_pos: usize,
    /// Tab in the console: debugpy's completions for the input's last name.
    pub completion: Option<ConsoleCompletion>,
    pub frames: Vec<Frame>,
    pub frame: usize,
    pub vars: Vec<VarRow>,
    /// Watch expressions (persisted per project).
    pub watches: Vec<String>,
    /// The selected row of the variables tree.
    pub sel: Option<usize>,
    pub edit: Option<VarEdit>,
    /// Stopped on an exception: "Type: message".
    pub exception: Option<String>,
    /// The working directory of the session (relative traceback paths).
    pub cwd: PathBuf,
    pub tab: DebugTab,
    pub focus: FocusHandle,
    console_scroll: ScrollHandle,
    /// Lines with exec marks, to clear them.
    marked: Vec<PathBuf>,
    /// The configuration of the last session (Rerun).
    pub last: Option<RunConfig>,
    /// The adapter is being started; no session yet.
    pub pending: bool,
    /// Bumped by every start and stop: a start that finishes late is dropped.
    gen: u64,
    /// Python Exception Breakpoint: stop on raised (even handled) / uncaught exceptions.
    pub exc_raised: bool,
    pub exc_uncaught: bool,
    pub exc_classes: Vec<ExcClass>,
    pub bp_dialog: Option<BpDialog>,
    /// Mute Breakpoints: none are sent (exception breakpoints neither); for this window only.
    pub muted: bool,
    /// The program's threads at a stop: (id, name). The chooser shows when there are several.
    pub threads: Vec<(i64, String)>,
    pub threads_open: bool,
    /// Smart Step Into: the calls on the line (debugpy stepInTargets) to choose from.
    pub chooser: Option<Chooser>,
    /// The process of the last Attach (Rerun attaches again while it lives): (pid, label).
    pub last_attach: Option<(u32, String)>,
}

pub enum ChooserKind {
    /// Smart Step Into on this thread.
    StepInto(i64),
    /// Attach to Process: target ids are process ids.
    Attach,
}

pub struct Chooser {
    pub kind: ChooserKind,
    /// (target id, label).
    pub targets: Vec<(i64, String)>,
    pub sel: usize,
}

impl Debug {
    pub fn new(root: &Path, cx: &mut gpui::App) -> Self {
        Self {
            session: None,
            stored: load(root),
            console: Vec::new(),
            input: String::new(),
            history: load_history(),
            history_pos: usize::MAX,
            completion: None,
            frames: Vec::new(),
            frame: 0,
            vars: Vec::new(),
            watches: load_watches(root),
            sel: None,
            edit: None,
            exception: None,
            cwd: root.to_path_buf(),
            tab: DebugTab::Debugger,
            focus: cx.focus_handle(),
            console_scroll: ScrollHandle::new(),
            marked: Vec::new(),
            last: None,
            pending: false,
            gen: 0,
            exc_raised: load_exc(root).0,
            exc_uncaught: load_exc(root).1,
            exc_classes: load_exc_classes(root),
            bp_dialog: None,
            muted: false,
            threads: Vec::new(),
            threads_open: false,
            chooser: None,
            last_attach: None,
        }
    }

    /// setExceptionBreakpoints arguments for the current settings.
    pub fn exception_args(&self) -> Value {
        exception_args(self.muted, self.exc_raised, self.exc_uncaught, &self.exc_classes)
    }

    /// A session is starting, running or paused (Stop applies).
    pub fn is_live(&self) -> bool {
        self.pending || self.session.as_ref().is_some_and(|s| s.state != State::Ended)
    }

    pub fn state(&self) -> Option<State> {
        self.session.as_ref().map(|s| s.state)
    }

    pub fn paused_thread(&self) -> Option<i64> {
        match self.state() {
            Some(State::Paused(t)) => Some(t),
            _ => None,
        }
    }

    fn print(&mut self, text: &str, kind: Line) {
        // Output arrives in chunks: continue the last line until a newline.
        let mut parts = text.split('\n').peekable();
        if let Some(first) = parts.next() {
            match self.console.last_mut() {
                Some((last, k)) if *k == kind && !last.ends_with('\n') && matches!(kind, Line::Out | Line::Err) => last.push_str(first),
                _ => self.console.push((first.to_string(), kind)),
            }
        }
        for p in parts {
            if let Some((last, _)) = self.console.last_mut() {
                last.push('\n');
            }
            self.console.push((p.to_string(), kind));
        }
        if self.console.last().is_some_and(|(l, _)| l.is_empty()) {
            self.console.pop();
            if let Some((last, _)) = self.console.last_mut() {
                if !last.ends_with('\n') {
                    last.push('\n');
                }
            }
        }
        if self.console.len() > 5000 {
            self.console.drain(..1000);
        }
        self.console_scroll.scroll_to_bottom();
    }

    /// Rows before the first scope: the Watches section and what is expanded under it.
    fn watch_len(&self) -> usize {
        self.vars.iter().position(|r| r.kind == RowKind::Scope).unwrap_or(self.vars.len())
    }

    /// The Watches section from evaluated values (None: not evaluated).
    fn watch_section(&self, vals: Option<Vec<Result<Variable, String>>>) -> Vec<VarRow> {
        if self.watches.is_empty() {
            return Vec::new();
        }
        let head = Variable { name: "Watches".into(), value: String::new(), type_name: String::new(), reference: 0 };
        let mut v = vec![VarRow { depth: 0, var: head, expanded: true, kind: RowKind::WatchHeader, parent: 0 }];
        for (i, w) in self.watches.iter().enumerate() {
            let var = match vals.as_ref().and_then(|v| v.get(i)) {
                Some(Ok(x)) => Variable { name: w.clone(), ..x.clone() },
                Some(Err(e)) => Variable { name: w.clone(), value: format!("<error: {e}>"), type_name: String::new(), reference: -1 },
                None => Variable { name: w.clone(), value: String::new(), type_name: String::new(), reference: 0 },
            };
            v.push(VarRow { depth: 1, var, expanded: false, kind: RowKind::Watch(i), parent: 0 });
        }
        v
    }

    /// Replace the Watches section, keeping the scopes below it.
    fn set_watch_rows(&mut self, rows: Vec<VarRow>) {
        let n = self.watch_len();
        let had = self.sel.and_then(|s| self.vars.get(s)).map(|r| (r.kind, r.var.name.clone()));
        self.vars.splice(..n, rows);
        self.sel = had.and_then(|(k, name)| self.vars.iter().position(|r| r.kind == k && r.var.name == name)).or(self.sel.filter(|&s| s < self.vars.len()));
    }

    /// Watches without values (the program runs or ended).
    fn idle_watches(&mut self) {
        self.vars.clear();
        self.vars = self.watch_section(None);
        self.sel = None;
    }

    fn info(&mut self, text: impl Into<String>) {
        let mut t: String = text.into();
        t.push('\n');
        self.console.push((t, Line::Info));
        self.console_scroll.scroll_to_bottom();
    }
}

fn store_path() -> PathBuf {
    brisk_db::config_dir().join("breakpoints.json")
}

fn load(root: &Path) -> Vec<StoredBp> {
    let Ok(s) = std::fs::read_to_string(store_path()) else { return Vec::new() };
    let Ok(v) = serde_json::from_str::<Value>(&s) else { return Vec::new() };
    let Some(list) = v.get(root.to_string_lossy().to_lowercase()).and_then(Value::as_array) else { return Vec::new() };
    list.iter()
        .filter_map(|b| {
            Some(StoredBp {
                path: root.join(b.get("path")?.as_str()?),
                line: b.get("line")?.as_u64()? as usize,
                condition: b.get("condition").and_then(Value::as_str).map(str::to_string),
                enabled: b.get("enabled").and_then(Value::as_bool).unwrap_or(true),
            })
        })
        .collect()
}

fn exc_path() -> PathBuf {
    brisk_db::config_dir().join("exception_breakpoints.json")
}

/// (on raised, on uncaught) for the project; PyCharm's default stops on uncaught only.
fn load_exc(root: &Path) -> (bool, bool) {
    let v = std::fs::read_to_string(exc_path()).ok().and_then(|s| serde_json::from_str::<Value>(&s).ok());
    let p = v.as_ref().and_then(|v| v.get(root.to_string_lossy().to_lowercase()));
    let get = |k: &str, d: bool| p.and_then(|p| p.get(k)).and_then(Value::as_bool).unwrap_or(d);
    (get("raised", false), get("uncaught", true))
}

fn load_exc_classes(root: &Path) -> Vec<ExcClass> {
    let v = std::fs::read_to_string(exc_path()).ok().and_then(|s| serde_json::from_str::<Value>(&s).ok());
    let list = v.as_ref().and_then(|v| v.get(root.to_string_lossy().to_lowercase())).and_then(|p| p.get("classes")).and_then(Value::as_array).cloned().unwrap_or_default();
    list.iter()
        .filter_map(|c| {
            let b = |k: &str, d: bool| c.get(k).and_then(Value::as_bool).unwrap_or(d);
            Some(ExcClass { name: c.get("name")?.as_str()?.to_string(), raised: b("raised", true), uncaught: b("uncaught", true), enabled: b("enabled", true) })
        })
        .filter(|c| !c.name.is_empty())
        .collect()
}

fn save_exc(root: &Path, raised: bool, uncaught: bool, classes: &[ExcClass]) {
    let path = exc_path();
    let mut all: serde_json::Map<String, Value> =
        std::fs::read_to_string(&path).ok().and_then(|s| serde_json::from_str::<Value>(&s).ok()).and_then(|v| v.as_object().cloned()).unwrap_or_default();
    let classes: Vec<Value> = classes
        .iter()
        .filter(|c| !c.name.trim().is_empty())
        .map(|c| json!({"name": c.name.trim(), "raised": c.raised, "uncaught": c.uncaught, "enabled": c.enabled}))
        .collect();
    all.insert(root.to_string_lossy().to_lowercase(), json!({"raised": raised, "uncaught": uncaught, "classes": classes}));
    if let Some(d) = path.parent() {
        let _ = std::fs::create_dir_all(d);
    }
    if let Ok(s) = serde_json::to_string_pretty(&Value::Object(all)) {
        let _ = std::fs::write(path, s);
    }
}

/// The completion list under the console's input line.
pub struct ConsoleCompletion {
    /// (label, type) from debugpy, for the name that began at byte `start` of the input.
    pub items: Vec<(String, String)>,
    pub start: usize,
    pub sel: usize,
}

impl ConsoleCompletion {
    /// The items that still match what is typed after `start`.
    fn shown<'a>(&'a self, input: &str) -> Vec<&'a (String, String)> {
        let q = input.get(self.start..).unwrap_or("").to_lowercase();
        self.items.iter().filter(|(l, _)| l.to_lowercase().starts_with(&q)).collect()
    }
}

/// Where the name being typed at the end of the console input begins (byte index).
fn completion_start(input: &str) -> usize {
    input.char_indices().rev().take_while(|(_, c)| c.is_alphanumeric() || *c == '_').last().map(|(i, _)| i).unwrap_or(input.len())
}

/// The longest prefix every label shares (case-sensitive).
fn common_prefix<'a>(labels: impl IntoIterator<Item = &'a str>) -> String {
    let mut it = labels.into_iter();
    let Some(first) = it.next() else { return String::new() };
    let mut p: &str = first;
    for l in it {
        let n = p.chars().zip(l.chars()).take_while(|(a, b)| a == b).map(|(a, _)| a.len_utf8()).sum();
        p = &p[..n];
    }
    p.to_string()
}

/// Public names first (dunder and _private last), keeping debugpy's order otherwise.
fn order_completions(items: &mut [(String, String)]) {
    items.sort_by_key(|(l, _)| (l.starts_with("__"), l.starts_with('_')));
}

fn history_path() -> PathBuf {
    brisk_db::config_dir().join("debug_console_history.json")
}

fn load_history() -> Vec<String> {
    std::fs::read_to_string(history_path()).ok().and_then(|s| serde_json::from_str(&s).ok()).unwrap_or_default()
}

fn watches_path() -> PathBuf {
    brisk_db::config_dir().join("watches.json")
}

fn load_watches(root: &Path) -> Vec<String> {
    let Ok(s) = std::fs::read_to_string(watches_path()) else { return Vec::new() };
    let Ok(v) = serde_json::from_str::<Value>(&s) else { return Vec::new() };
    v.get(root.to_string_lossy().to_lowercase()).and_then(Value::as_array).map(|a| a.iter().filter_map(Value::as_str).map(str::to_string).collect()).unwrap_or_default()
}

fn save_watches(root: &Path, list: &[String]) {
    let path = watches_path();
    let mut all: serde_json::Map<String, Value> =
        std::fs::read_to_string(&path).ok().and_then(|s| serde_json::from_str::<Value>(&s).ok()).and_then(|v| v.as_object().cloned()).unwrap_or_default();
    let key = root.to_string_lossy().to_lowercase();
    if list.is_empty() {
        all.remove(&key);
    } else {
        all.insert(key, json!(list));
    }
    if let Some(d) = path.parent() {
        let _ = std::fs::create_dir_all(d);
    }
    if let Ok(s) = serde_json::to_string_pretty(&Value::Object(all)) {
        let _ = std::fs::write(path, s);
    }
}

/// debugpy's exceptionInfo traceback lists the innermost frame first and ends with the
/// exception line: Python's order (innermost last), without that line.
fn python_order(stack: &str) -> String {
    let mut lines: Vec<&str> = stack.trim_end().lines().collect();
    while lines.last().is_some_and(|l| !l.starts_with(' ')) {
        lines.pop();
    }
    if !lines.first().is_some_and(|l| l.trim_start().starts_with("File \"")) {
        return lines.join("\n");
    }
    let mut frames: Vec<Vec<&str>> = Vec::new();
    for l in lines {
        if l.trim_start().starts_with("File \"") || frames.is_empty() {
            frames.push(vec![l]);
        } else if let Some(f) = frames.last_mut() {
            f.push(l);
        }
    }
    frames.reverse();
    frames.concat().join("\n")
}

const KEYWORDS: &[&str] = &[
    "False", "None", "True", "and", "as", "assert", "async", "await", "break", "class", "continue", "def", "del", "elif", "else", "except", "finally", "for",
    "from", "global", "if", "import", "in", "is", "lambda", "nonlocal", "not", "or", "pass", "raise", "return", "try", "while", "with", "yield",
];

/// The expression under the mouse for a value hover: the name at `col` with the dotted
/// names before it (`obj.attr`), not inside a string or comment, not a keyword.
fn hover_expression(line: &str, col: usize) -> Option<String> {
    let c: Vec<char> = line.chars().collect();
    let ident = |ch: char| ch.is_alphanumeric() || ch == '_';
    if !c.get(col).copied().is_some_and(ident) {
        return None;
    }
    // Inside a comment or a string: count quotes before the point.
    let before: String = c[..col].iter().collect();
    let (mut quote, mut i) = (None::<char>, 0);
    let bc: Vec<char> = before.chars().collect();
    while i < bc.len() {
        match (quote, bc[i]) {
            (None, '#') => return None,
            (None, q @ ('"' | '\'')) => quote = Some(q),
            (Some(_), '\\') => i += 1,
            (Some(q), ch) if ch == q => quote = None,
            _ => {}
        }
        i += 1;
    }
    if quote.is_some() {
        return None;
    }
    let mut start = col;
    while start > 0 && ident(c[start - 1]) {
        start -= 1;
    }
    let mut end = col;
    while end < c.len() && ident(c[end]) {
        end += 1;
    }
    let word: String = c[start..end].iter().collect();
    if KEYWORDS.contains(&word.as_str()) || word.chars().next().is_some_and(|ch| ch.is_ascii_digit()) {
        return None;
    }
    while start >= 2 && c[start - 1] == '.' && ident(c[start - 2]) {
        let mut s = start - 1;
        while s > 0 && ident(c[s - 1]) {
            s -= 1;
        }
        start = s;
    }
    Some(c[start..end].iter().collect())
}

/// Names used on a line of Python: identifiers outside strings and comments, not after a dot.
fn line_names(line: &str) -> Vec<&str> {
    let b = line.as_bytes();
    let mut out: Vec<&str> = Vec::new();
    let mut i = 0;
    while i < b.len() {
        let c = b[i];
        if c == b'#' {
            break;
        }
        if c == b'"' || c == b'\'' {
            // Skip the string (prefixes like f"" leave their letter as a word: harmless).
            let q = c;
            i += 1;
            while i < b.len() && b[i] != q {
                i += if b[i] == b'\\' { 2 } else { 1 };
            }
            i += 1;
            continue;
        }
        if c.is_ascii_alphabetic() || c == b'_' {
            let s = i;
            while i < b.len() && (b[i].is_ascii_alphanumeric() || b[i] == b'_') {
                i += 1;
            }
            let after_dot = line[..s].trim_end().ends_with('.');
            let w = &line[s..i];
            if !after_dot && !out.contains(&w) {
                out.push(w);
            }
            continue;
        }
        i += 1;
    }
    out
}

/// PyCharm's inline values: for the lines of the current function up to the stop line,
/// `name: value` of the frame's variables used there. `lines` are the file's lines.
fn inline_values(lines: &[String], stop: usize, vars: &[Variable]) -> std::collections::HashMap<usize, String> {
    let mut out = std::collections::HashMap::new();
    let Some(cur) = lines.get(stop) else { return out };
    let indent = |l: &str| l.len() - l.trim_start().len();
    let cur_indent = indent(cur);
    // The function's `def` above the stop line (module level: up to 50 lines back).
    let start = (0..=stop)
        .rev()
        .find(|&i| {
            let t = lines[i].trim_start();
            (t.starts_with("def ") || t.starts_with("async def ")) && indent(&lines[i]) < cur_indent.max(1)
        })
        .unwrap_or(stop.saturating_sub(50))
        .max(stop.saturating_sub(300));
    for (i, l) in lines.iter().enumerate().take(stop + 1).skip(start) {
        let shown: Vec<String> = line_names(l)
            .into_iter()
            .filter_map(|n| vars.iter().find(|v| v.name == n))
            .map(|v| {
                let mut val: String = v.value.chars().take(60).collect();
                if v.value.chars().count() > 60 {
                    val.push('…');
                }
                format!("{}: {val}", v.name)
            })
            .collect();
        if !shown.is_empty() {
            out.insert(i, shown.join(", "));
        }
    }
    out
}

/// What a Python command line runs, short: `app.py` or `-m http.server` (the list's label).
fn process_target(cmd: &str) -> String {
    let rest = match cmd.strip_prefix('"') {
        Some(q) => q.split_once('"').map_or("", |x| x.1),
        None => cmd.split_once(char::is_whitespace).map_or("", |x| x.1),
    };
    let mut it = rest.split_whitespace();
    while let Some(t) = it.next() {
        if t == "-m" {
            return format!("-m {}", it.next().unwrap_or(""));
        }
        if t == "-c" {
            return "-c …".into();
        }
        if !t.starts_with('-') {
            let t = t.trim_matches('"');
            return t.rsplit(['/', '\\']).next().unwrap_or(t).to_string();
        }
    }
    String::new()
}

/// The statement starting at `line`: following lines join while brackets are open or
/// the line ends with a backslash (at most 30 lines).
fn statement_text(lines: &[String], line: usize) -> String {
    let mut out = String::new();
    let mut depth = 0i32;
    for l in lines.iter().skip(line).take(30) {
        let code = l.split('#').next().unwrap_or("");
        out.push_str(code);
        out.push('\n');
        depth += code.chars().map(|c| match c {
            '(' | '[' | '{' => 1,
            ')' | ']' | '}' => -1,
            _ => 0,
        }).sum::<i32>();
        if depth <= 0 && !code.trim_end().ends_with('\\') {
            break;
        }
    }
    out
}

/// Smart Step Into targets of this statement: debugpy lists every call of the frame's code
/// not run yet (later lines too), as `a(b(3))` or `f (call 2)`. A target stays when its
/// callee is called in `stmt`; when none would, all stay.
fn targets_on_statement(stmt: &str, targets: Vec<(i64, String)>) -> Vec<(i64, String)> {
    let squeezed: String = stmt.chars().filter(|c| !c.is_whitespace()).collect();
    let callee = |label: &str| -> String {
        let l = label.split(" (call ").next().unwrap_or(label);
        l.split('(').next().unwrap_or(l).chars().filter(|c| !c.is_whitespace()).collect()
    };
    let on = |label: &str| {
        let c = callee(label);
        if c.is_empty() {
            return true;
        }
        let pat = format!("{c}(");
        // The callee must not be the tail of a longer name (`xa(` is not `a(`).
        squeezed.match_indices(&pat).any(|(i, _)| {
            let before = squeezed[..i].chars().next_back();
            !before.is_some_and(|ch| ch.is_alphanumeric() || ch == '_' || (ch == '.' && !c.contains('.')))
        })
    };
    let kept: Vec<(i64, String)> = targets.iter().filter(|t| on(&t.1)).cloned().collect();
    if kept.is_empty() { targets } else { kept }
}

/// debugpy (showReturnValue) names the value a call just returned "(return) f".
fn return_label(name: &str) -> Option<String> {
    name.strip_prefix("(return) ").map(|f| format!("↩ {f}()"))
}

/// `pid<TAB>command line` for every process (Windows: CIM; elsewhere: `ps`).
fn process_lines() -> String {
    #[cfg(windows)]
    let mut c = {
        use std::os::windows::process::CommandExt;
        let mut c = std::process::Command::new("powershell");
        c.args([
            "-NoProfile",
            "-NonInteractive",
            "-Command",
            "Get-CimInstance Win32_Process -Filter \"Name like 'python%'\" | ForEach-Object { \"$($_.ProcessId)`t$($_.CommandLine)\" }",
        ]);
        c.creation_flags(0x0800_0000);
        c
    };
    #[cfg(not(windows))]
    let mut c = {
        let mut c = std::process::Command::new("ps");
        c.args(["-eo", "pid=,args="]);
        c
    };
    c.stdin(std::process::Stdio::null()).stderr(std::process::Stdio::null());
    c.output().map(|o| String::from_utf8_lossy(&o.stdout).into_owned()).unwrap_or_default()
}

/// `python [opts] (-m module | script) args` → what debugpy launches.
pub(super) fn launch_for(cmd: &brisk_run::Command) -> Launch {
    let mut l = Launch { python: cmd.program.clone(), cwd: cmd.cwd.clone(), just_my_code: true, ..Default::default() };
    let mut it = cmd.args.iter();
    while let Some(a) = it.next() {
        if a == "-m" {
            l.module = true;
            l.target = it.next().cloned().unwrap_or_default();
            break;
        } else if a == "-X" || a == "-W" {
            // Options with a value.
            l.python_args.push(a.clone());
            l.python_args.extend(it.next().cloned());
        } else if a.starts_with('-') {
            l.python_args.push(a.clone());
        } else {
            l.target = a.clone();
            break;
        }
    }
    l.args = it.cloned().collect();
    let mut env: Vec<(String, String)> = cmd.env.iter().map(|(k, v)| (k.clone(), v.clone())).collect();
    env.sort();
    l.env = env;
    l
}

impl Workspace {
    // --- Breakpoints ------------------------------------------------------------------

    /// All breakpoints by file (open editors first-hand, the rest stored).
    pub(super) fn all_breakpoints(&self, cx: &gpui::App) -> Vec<(PathBuf, Vec<(usize, Breakpoint)>)> {
        let mut out: Vec<(PathBuf, Vec<(usize, Breakpoint)>)> = Vec::new();
        let mut open = Vec::new();
        for e in self.editors() {
            let e = e.read(cx);
            let Some(p) = e.path.clone() else { continue };
            open.push(p.clone());
            let list = e.breakpoint_list();
            if !list.is_empty() {
                out.push((p, list));
            }
        }
        for b in &self.debug.stored {
            if open.contains(&b.path) {
                continue;
            }
            let bp = (b.line, b.breakpoint());
            match out.iter_mut().find(|(p, _)| *p == b.path) {
                Some((_, v)) => v.push(bp),
                None => out.push((b.path.clone(), vec![bp])),
            }
        }
        out.sort_by(|a, b| a.0.cmp(&b.0));
        out
    }

    pub(super) fn persist_breakpoints(&self, cx: &gpui::App) {
        let path = store_path();
        let mut all: serde_json::Map<String, Value> =
            std::fs::read_to_string(&path).ok().and_then(|s| serde_json::from_str::<Value>(&s).ok()).and_then(|v| v.as_object().cloned()).unwrap_or_default();
        let mut list = Vec::new();
        for (p, bps) in self.all_breakpoints(cx) {
            let rel = p.strip_prefix(&self.root).unwrap_or(&p).to_string_lossy().replace('\\', "/");
            for (line, b) in bps {
                list.push(json!({"path": rel, "line": line, "condition": b.condition, "enabled": b.enabled}));
            }
        }
        let key = self.root.to_string_lossy().to_lowercase();
        if list.is_empty() {
            all.remove(&key);
        } else {
            all.insert(key, Value::Array(list));
        }
        if let Some(d) = path.parent() {
            let _ = std::fs::create_dir_all(d);
        }
        if let Ok(s) = serde_json::to_string_pretty(&Value::Object(all)) {
            let _ = std::fs::write(path, s);
        }
    }

    /// A file was opened: its stored breakpoints move into the editor.
    pub(super) fn adopt_breakpoints(&mut self, ed: &Entity<Editor>, cx: &mut Context<Self>) {
        let Some(p) = ed.read(cx).path.clone() else { return };
        let mine: Vec<(usize, Breakpoint)> =
            self.debug.stored.iter().filter(|b| b.path == p).map(|b| (b.line, b.breakpoint())).collect();
        if mine.is_empty() {
            return;
        }
        self.debug.stored.retain(|b| b.path != p);
        let muted = self.debug.muted;
        ed.update(cx, |e, cx| {
            e.set_breakpoints(&mine);
            e.set_breakpoints_muted(muted);
            cx.notify();
        });
    }

    /// Mute Breakpoints: grey them in every editor and take them all out of a running session.
    pub(super) fn toggle_mute_breakpoints(&mut self, cx: &mut Context<Self>) {
        self.debug.muted = !self.debug.muted;
        let muted = self.debug.muted;
        for ed in self.editors().cloned().collect::<Vec<_>>() {
            ed.update(cx, |e, cx| {
                e.set_breakpoints_muted(muted);
                cx.notify();
            });
        }
        for (p, _) in self.all_breakpoints(cx) {
            self.send_breakpoints(&p, cx);
        }
        if self.debug.session.as_ref().is_some_and(|s| s.state != State::Ended) {
            self.debug_request("setExceptionBreakpoints", self.debug.exception_args());
        }
        self.status = if muted { "Breakpoints muted".into() } else { "Breakpoints unmuted".into() };
        cx.notify();
    }

    /// An editor closes: keep its breakpoints.
    pub(super) fn stash_breakpoints(&mut self, ed: &Entity<Editor>, cx: &mut Context<Self>) {
        let Some(p) = ed.read(cx).path.clone() else { return };
        let list = ed.read(cx).breakpoint_list();
        self.debug.stored.retain(|b| b.path != p);
        self.debug.stored.extend(list.into_iter().map(|(line, b)| StoredBp { path: p.clone(), line, condition: b.condition, enabled: b.enabled }));
    }

    /// A breakpoint changed in an editor: save, and tell a running debugger.
    pub(super) fn breakpoints_changed(&mut self, ed: Entity<Editor>, cx: &mut Context<Self>) {
        self.persist_breakpoints(cx);
        if let Some(p) = ed.read(cx).path.clone() {
            self.send_breakpoints(&p, cx);
        }
    }

    fn source_breakpoints(&self, path: &Path, cx: &gpui::App) -> Vec<SourceBreakpoint> {
        let mut v: Vec<SourceBreakpoint> = self
            .all_breakpoints(cx)
            .into_iter()
            .filter(|(p, _)| p == path && !self.debug.muted)
            .flat_map(|(_, l)| l.into_iter().filter(|(_, b)| b.enabled).map(|(line, b)| SourceBreakpoint { line, condition: b.condition }))
            .collect();
        if let Some((p, l)) = self.debug.session.as_ref().and_then(|s| s.temp.clone()) {
            if p == path && !v.iter().any(|b| b.line == l) {
                v.push(SourceBreakpoint { line: l, condition: None });
            }
        }
        v
    }

    /// setBreakpoints for one file; the reply marks verified breakpoints in its editor.
    fn send_breakpoints(&mut self, path: &Path, cx: &mut Context<Self>) {
        let Some(s) = self.debug.session.as_ref().filter(|s| s.state != State::Ended) else { return };
        let bps = self.source_breakpoints(path, cx);
        let rx = s.client.request("setBreakpoints", brisk_dap::set_breakpoints_args(path, &bps));
        let path = path.to_path_buf();
        cx.spawn(async move |this, cx| {
            let Ok(Ok(v)) = rx.await else { return };
            let verified = brisk_dap::parse_breakpoints(&v);
            let _ = this.update(cx, |this, cx| {
                // Replies list the temporary breakpoint last; editors only know the user's.
                if let Some(ed) = this.editors().find(|e| e.read(cx).path.as_ref() == Some(&path)).cloned() {
                    ed.update(cx, |e, cx| {
                        e.set_breakpoints_verified(&verified);
                        cx.notify();
                    });
                }
            });
        })
        .detach();
    }

    /// Ctrl+F8 at the caret.
    fn toggle_breakpoint_here(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let Some(ed) = self.focused_editor(window, cx).or_else(|| self.active_editor().cloned()) else { return };
        if !ed.read(cx).can_debug() {
            self.status = "Breakpoints are for Python files".into();
            return;
        }
        let line = ed.read(cx).caret_line_col().0;
        ed.update(cx, |e, cx| e.toggle_breakpoint(line, cx));
    }

    /// Right click on a breakpoint / Edit Breakpoint: the condition popup.
    pub(super) fn edit_breakpoint(&mut self, ed: Entity<Editor>, line: usize, cx: &mut Context<Self>) {
        let cond = ed.read(cx).breakpoint_list().into_iter().find(|(l, _)| *l == line).and_then(|(_, b)| b.condition).unwrap_or_default();
        self.debug_edit = Some((ed, line));
        self.open_popup(PopupKind::BreakpointCondition);
        if let Some(p) = self.popup.as_mut() {
            p.query = cond;
        }
        self.refresh_popup();
        cx.notify();
    }

    pub(super) fn condition_items(&self, q: &str) -> Vec<PopupItem> {
        let q = q.trim();
        let mut v = vec![PopupItem {
            label: if q.is_empty() { "Stop always (no condition)".into() } else { format!("Stop when: {q}") },
            detail: String::new(),
            hint: "Enter".into(),
            target: Target::BpCondition(q.to_string()),
        }];
        v.push(PopupItem { label: "Remove breakpoint".into(), detail: String::new(), hint: String::new(), target: Target::BpRemove });
        v
    }

    pub(super) fn pick_condition(&mut self, target: Target, window: &mut Window, cx: &mut Context<Self>) {
        let Some((ed, line)) = self.debug_edit.take() else { return };
        match target {
            Target::BpCondition(c) => ed.update(cx, |e, cx| e.set_breakpoint_condition(line, Some(c), cx)),
            Target::BpRemove => {
                if ed.read(cx).breakpoint_list().iter().any(|(l, _)| *l == line) {
                    ed.update(cx, |e, cx| e.toggle_breakpoint(line, cx));
                }
            }
            _ => {}
        }
        self.focus_editor(window, cx);
    }

    // --- View Breakpoints dialog -------------------------------------------------------

    pub(super) fn bp_rows(&self, cx: &gpui::App) -> Vec<BpRow> {
        let mut v = vec![BpRow::Raised, BpRow::Uncaught];
        v.extend(self.debug.exc_classes.iter().cloned().enumerate().map(|(i, c)| BpRow::Class(i, c)));
        for (p, bps) in self.all_breakpoints(cx) {
            v.extend(bps.into_iter().map(|(l, b)| BpRow::Line(p.clone(), l, b)));
        }
        v
    }

    pub(super) fn open_bp_dialog(&mut self, cx: &mut Context<Self>) {
        // The first line breakpoint is selected, as in PyCharm.
        let sel = self.bp_rows(cx).iter().position(|r| matches!(r, BpRow::Line(..))).unwrap_or(0);
        self.debug.bp_dialog = Some(BpDialog { sel, editing: false });
        cx.notify();
    }

    fn bp_editor(&self, path: &Path, cx: &gpui::App) -> Option<Entity<Editor>> {
        self.editors().find(|e| e.read(cx).path.as_deref() == Some(path)).cloned()
    }

    /// A stored (not open) breakpoint changed: save, tell the debugger.
    fn stored_bp_changed(&mut self, path: &Path, cx: &mut Context<Self>) {
        self.persist_breakpoints(cx);
        self.send_breakpoints(path, cx);
        cx.notify();
    }

    pub(super) fn bp_set_enabled(&mut self, path: &Path, line: usize, on: bool, cx: &mut Context<Self>) {
        match self.bp_editor(path, cx) {
            Some(ed) => ed.update(cx, |e, cx| e.set_breakpoint_enabled(line, on, cx)),
            None => {
                if let Some(b) = self.debug.stored.iter_mut().find(|b| b.path == path && b.line == line) {
                    b.enabled = on;
                }
                self.stored_bp_changed(path, cx);
            }
        }
    }

    pub(super) fn bp_set_condition(&mut self, path: &Path, line: usize, cond: Option<String>, cx: &mut Context<Self>) {
        let cond = cond.filter(|c| !c.trim().is_empty());
        match self.bp_editor(path, cx) {
            Some(ed) => ed.update(cx, |e, cx| e.set_breakpoint_condition(line, cond, cx)),
            None => {
                if let Some(b) = self.debug.stored.iter_mut().find(|b| b.path == path && b.line == line) {
                    b.condition = cond;
                }
                self.stored_bp_changed(path, cx);
            }
        }
    }

    pub(super) fn bp_remove(&mut self, path: &Path, line: usize, cx: &mut Context<Self>) {
        match self.bp_editor(path, cx) {
            Some(ed) => {
                if ed.read(cx).breakpoint_list().iter().any(|(l, _)| *l == line) {
                    ed.update(cx, |e, cx| e.toggle_breakpoint(line, cx));
                }
            }
            None => {
                self.debug.stored.retain(|b| !(b.path == path && b.line == line));
                self.stored_bp_changed(path, cx);
            }
        }
    }

    /// The Python Exception Breakpoint checkboxes.
    pub(super) fn set_exception_breakpoints(&mut self, raised: bool, uncaught: bool, cx: &mut Context<Self>) {
        self.debug.exc_raised = raised;
        self.debug.exc_uncaught = uncaught;
        save_exc(&self.root, raised, uncaught, &self.debug.exc_classes);
        if self.debug.session.as_ref().is_some_and(|s| s.state != State::Ended) {
            self.debug_request("setExceptionBreakpoints", self.debug.exception_args());
        }
        cx.notify();
    }

    /// A class exception breakpoint changed: save and tell a running debugger.
    pub(super) fn bp_update_class(&mut self, i: usize, f: impl FnOnce(&mut ExcClass), cx: &mut Context<Self>) {
        if let Some(c) = self.debug.exc_classes.get_mut(i) {
            f(c);
        }
        let (r, u) = (self.debug.exc_raised, self.debug.exc_uncaught);
        self.set_exception_breakpoints(r, u, cx);
    }

    /// "+ Exception": a new class row, its name field ready for typing.
    pub(super) fn bp_add_class(&mut self, cx: &mut Context<Self>) {
        self.bp_drop_empty_classes(cx);
        self.debug.exc_classes.push(ExcClass::new());
        let i = self.debug.exc_classes.len() - 1;
        let sel = self.bp_rows(cx).iter().position(|r| matches!(r, BpRow::Class(j, _) if *j == i)).unwrap_or(0);
        if let Some(d) = self.debug.bp_dialog.as_mut() {
            d.sel = sel;
            d.editing = true;
        }
        cx.notify();
    }

    pub(super) fn bp_remove_class(&mut self, i: usize, cx: &mut Context<Self>) {
        if i < self.debug.exc_classes.len() {
            self.debug.exc_classes.remove(i);
        }
        let (r, u) = (self.debug.exc_raised, self.debug.exc_uncaught);
        self.set_exception_breakpoints(r, u, cx);
    }

    /// A class row left without a name goes away.
    fn bp_drop_empty_classes(&mut self, cx: &mut Context<Self>) {
        if self.debug.exc_classes.iter().any(|c| c.name.trim().is_empty()) {
            self.debug.exc_classes.retain(|c| !c.name.trim().is_empty());
            let (r, u) = (self.debug.exc_raised, self.debug.exc_uncaught);
            self.set_exception_breakpoints(r, u, cx);
        }
    }

    /// Space / a click on the checkbox.
    fn bp_toggle_row(&mut self, row: &BpRow, cx: &mut Context<Self>) {
        let (r, u) = (self.debug.exc_raised, self.debug.exc_uncaught);
        match row {
            BpRow::Raised => self.set_exception_breakpoints(!r, u, cx),
            BpRow::Uncaught => self.set_exception_breakpoints(r, !u, cx),
            BpRow::Class(i, c) => {
                let on = !c.enabled;
                self.bp_update_class(*i, |c| c.enabled = on, cx);
            }
            BpRow::Line(p, l, b) => self.bp_set_enabled(p, *l, !b.enabled, cx),
        }
    }

    /// Enter / a double click: the breakpoint's line in the editor.
    fn bp_go_to(&mut self, row: &BpRow, window: &mut Window, cx: &mut Context<Self>) {
        let BpRow::Line(p, l, _) = row else { return };
        self.debug.bp_dialog = None;
        self.open_file(p, window, cx);
        if let Some(ed) = self.active_editor().cloned() {
            ed.update(cx, |e, cx| e.go_to_line(*l, cx));
        }
        self.focus_editor(window, cx);
        cx.notify();
    }

    pub(super) fn bp_dialog_key(&mut self, e: &gpui::KeyDownEvent, window: &mut Window, cx: &mut Context<Self>) {
        let rows = self.bp_rows(cx);
        let Some(d) = self.debug.bp_dialog.as_mut() else { return };
        d.sel = d.sel.min(rows.len().saturating_sub(1));
        let sel = d.sel;
        let row = rows.get(sel).cloned();
        let k = &e.keystroke;
        if k.modifiers.control || k.modifiers.alt || k.modifiers.platform {
            if k.key == "f8" && k.modifiers.control && k.modifiers.shift {
                self.debug.bp_dialog = None;
            }
            cx.notify();
            return;
        }
        let was_editing = d.editing;
        match k.key.as_str() {
            "escape" | "enter" if d.editing => d.editing = false,
            "escape" => self.debug.bp_dialog = None,
            "up" => {
                d.editing = false;
                d.sel = sel.saturating_sub(1);
            }
            "down" => {
                d.editing = false;
                d.sel = (sel + 1).min(rows.len().saturating_sub(1));
            }
            "insert" | "+" if !d.editing => self.bp_add_class(cx),
            "space" if !d.editing => {
                if let Some(r) = row {
                    self.bp_toggle_row(&r, cx);
                }
            }
            "space" if matches!(row, Some(BpRow::Class(..))) => {}
            "space" => {
                if let Some(BpRow::Line(p, l, b)) = row {
                    let mut c = b.condition.unwrap_or_default();
                    c.push(' ');
                    self.bp_set_condition(&p, l, Some(c), cx);
                }
            }
            "enter" | "f4" => {
                if let Some(r) = row {
                    self.bp_go_to(&r, window, cx);
                }
            }
            "delete" => match row {
                Some(BpRow::Line(p, l, _)) => self.bp_remove(&p, l, cx),
                Some(BpRow::Class(i, _)) => self.bp_remove_class(i, cx),
                _ => {}
            },
            "backspace" if matches!(row, Some(BpRow::Class(..))) => {
                if let Some(BpRow::Class(i, _)) = row {
                    d.editing = true;
                    self.bp_update_class(i, |c| {
                        c.name.pop();
                    }, cx);
                }
            }
            "backspace" => {
                if let Some(BpRow::Line(p, l, b)) = row {
                    let mut c = b.condition.unwrap_or_default();
                    c.pop();
                    self.bp_set_condition(&p, l, Some(c), cx);
                }
            }
            _ => {
                // Typing edits a class name (letters, digits, _ and .).
                if let (Some(BpRow::Class(i, _)), Some(t)) = (&row, k.key_char.as_deref().filter(|t| t.chars().all(|c| c.is_alphanumeric() || c == '_' || c == '.'))) {
                    d.editing = true;
                    let t = t.to_string();
                    self.bp_update_class(*i, |c| c.name.push_str(&t), cx);
                    cx.notify();
                    return;
                }
                // Typing edits the selected breakpoint's condition.
                if let (Some(BpRow::Line(p, l, b)), Some(t)) = (row, k.key_char.as_deref().filter(|t| !t.chars().any(char::is_control))) {
                    d.editing = true;
                    let mut c = b.condition.unwrap_or_default();
                    c.push_str(t);
                    self.bp_set_condition(&p, l, Some(c), cx);
                }
            }
        }
        // Leaving a class name field: an empty one is dropped.
        if was_editing && !self.debug.bp_dialog.as_ref().is_some_and(|d| d.editing) {
            self.bp_drop_empty_classes(cx);
        }
        cx.notify();
    }

    pub(super) fn render_bp_dialog(&mut self, cx: &mut Context<Self>) -> Option<gpui::AnyElement> {
        let sel = self.debug.bp_dialog.as_ref()?.sel;
        let editing = self.debug.bp_dialog.as_ref().is_some_and(|d| d.editing);
        let rows = self.bp_rows(cx);
        let sel = sel.min(rows.len().saturating_sub(1));
        let check = |on: bool| {
            div()
                .size(px(14.))
                .flex_shrink_0()
                .flex()
                .items_center()
                .justify_center()
                .rounded(px(3.))
                .border_1()
                .border_color(if on { Theme::accent() } else { Theme::text_dim() })
                .when(on, |x| x.bg(Theme::accent()).child(crate::icons::icon("check-bold", 10., gpui::white())))
        };
        let group = |label: &'static str| div().px(px(8.)).pt(px(6.)).pb(px(2.)).text_size(px(11.)).text_color(Theme::text_dim()).child(label);
        let mut list: Vec<gpui::AnyElement> = vec![group("PYTHON EXCEPTION BREAKPOINTS").into_any_element()];
        let first_line = rows.iter().position(|r| matches!(r, BpRow::Line(..)));
        for (i, r) in rows.iter().enumerate() {
            if Some(i) == first_line {
                list.push(group("PYTHON LINE BREAKPOINTS").into_any_element());
            }
            let (on, label, detail) = match r {
                BpRow::Raised => (self.debug.exc_raised, "On raised exceptions".to_string(), "even when handled".to_string()),
                BpRow::Uncaught => (self.debug.exc_uncaught, "On uncaught exceptions".to_string(), String::new()),
                BpRow::Class(_, c) => (
                    c.enabled,
                    if c.name.is_empty() { "<class name>".to_string() } else { c.name.clone() },
                    match (c.raised, c.uncaught) {
                        (true, _) => "on raise".to_string(),
                        (false, true) => "on termination".to_string(),
                        _ => "never".to_string(),
                    },
                ),
                BpRow::Line(p, l, b) => {
                    let rel = p.strip_prefix(&self.root).unwrap_or(p).to_string_lossy().replace('\\', "/");
                    (b.enabled, format!("{rel}:{}", l + 1), b.condition.clone().map(|c| format!("if {c}")).unwrap_or_default())
                }
            };
            let row = r.clone();
            let row2 = r.clone();
            let selected = i == sel;
            list.push(
                div()
                    .id(("bp-row", i))
                    .h(px(24.))
                    .px(px(8.))
                    .flex()
                    .items_center()
                    .gap(px(8.))
                    .cursor_pointer()
                    .when(selected, |x| x.bg(Theme::selection_row()))
                    .when(!selected, |x| x.hover(|s| s.bg(Theme::hover_row())))
                    .child(
                        div().id(("bp-check", i)).child(check(on)).on_click(cx.listener(move |this, _, _, cx| {
                            cx.stop_propagation();
                            this.bp_toggle_row(&row, cx);
                        })),
                    )
                    .child(div().text_color(if on { Theme::text() } else { Theme::text_muted() }).whitespace_nowrap().child(label))
                    .child(div().text_color(Theme::text_dim()).whitespace_nowrap().overflow_hidden().child(detail))
                    .on_click(cx.listener(move |this, ev: &gpui::ClickEvent, w, cx| {
                        if let Some(d) = this.debug.bp_dialog.as_mut() {
                            d.sel = i;
                            d.editing = false;
                        }
                        this.bp_drop_empty_classes(cx);
                        if ev.click_count() >= 2 {
                            this.bp_go_to(&row2, w, cx);
                        }
                        cx.notify();
                    }))
                    .into_any_element(),
            );
        }
        let button = |id: &'static str, label: &'static str, primary: bool, on: bool| {
            div()
                .id(id)
                .px(px(14.))
                .h(px(28.))
                .flex()
                .items_center()
                .rounded(px(5.))
                .when(!on, |b| b.text_color(Theme::text_dim()).border_1().border_color(Theme::divider()))
                .when(on, |b| b.cursor_pointer().active(|s| s.bg(Theme::press())))
                .when(on && primary, |b| b.bg(Theme::accent()).text_color(gpui::white()))
                .when(on && !primary, |b| b.border_1().border_color(Theme::divider()).hover(|st| st.bg(Theme::hover_row())))
                .child(label)
        };
        let cur = rows.get(sel).cloned();
        let details: gpui::AnyElement = match &cur {
            Some(BpRow::Line(p, l, b)) => {
                let cond = b.condition.clone().unwrap_or_default();
                let (p2, l2) = (p.clone(), *l);
                div()
                    .flex()
                    .flex_col()
                    .gap(px(10.))
                    .child(div().font_weight(gpui::FontWeight::SEMIBOLD).child(format!("{}:{}", p.file_name().map(|n| n.to_string_lossy().to_string()).unwrap_or_default(), l + 1)))
                    .child(
                        div().id("bp-enabled").flex().items_center().gap(px(8.)).cursor_pointer().child(check(b.enabled)).child("Enabled").on_click(cx.listener(move |this, _, _, cx| {
                            if let Some(BpRow::Line(p, l, b)) = this.bp_rows(cx).into_iter().find(|r| matches!(r, BpRow::Line(p, l, _) if *p == p2 && *l == l2)) {
                                this.bp_set_enabled(&p, l, !b.enabled, cx);
                            }
                        })),
                    )
                    .child(div().text_color(Theme::text_muted()).child("Condition (type to edit):"))
                    .child(
                        div()
                            .id("bp-condition")
                            .h(px(28.))
                            .px(px(8.))
                            .flex()
                            .items_center()
                            .rounded(px(5.))
                            .border_1()
                            .border_color(if editing { Theme::accent() } else { Theme::divider() })
                            .bg(Theme::editor_bg())
                            .font_family(mono_font())
                            .cursor_text()
                            .child(cond)
                            .when(editing, |x| x.child(div().w(px(1.5)).h(px(15.)).bg(Theme::caret())))
                            .on_click(cx.listener(|this, _, _, cx| {
                                if let Some(d) = this.debug.bp_dialog.as_mut() {
                                    d.editing = true;
                                }
                                cx.notify();
                            })),
                    )
                    .child(div().text_size(px(12.)).text_color(Theme::text_dim()).child("A Python expression: the debugger stops only when it is true. Empty: always."))
                    .into_any_element()
            }
            Some(BpRow::Class(i, c)) => {
                let i = *i;
                let flag = |id: &'static str, on: bool, label: &'static str, f: fn(&mut ExcClass)| {
                    div().id(id).flex().items_center().gap(px(8.)).cursor_pointer().child(check(on)).child(label).on_click(cx.listener(move |this, _, _, cx| this.bp_update_class(i, f, cx)))
                };
                div()
                    .flex()
                    .flex_col()
                    .gap(px(10.))
                    .child(div().font_weight(gpui::FontWeight::SEMIBOLD).child("Python exception class"))
                    .child(
                        div()
                            .id("bp-class-name")
                            .h(px(28.))
                            .px(px(8.))
                            .flex()
                            .items_center()
                            .rounded(px(5.))
                            .border_1()
                            .border_color(if editing { Theme::accent() } else { Theme::divider() })
                            .bg(Theme::editor_bg())
                            .font_family(mono_font())
                            .cursor_text()
                            .child(c.name.clone())
                            .when(editing, |x| x.child(div().w(px(1.5)).h(px(15.)).bg(Theme::caret())))
                            .on_click(cx.listener(|this, _, _, cx| {
                                if let Some(d) = this.debug.bp_dialog.as_mut() {
                                    d.editing = true;
                                }
                                cx.notify();
                            })),
                    )
                    .child(flag("bp-class-enabled", c.enabled, "Enabled", |c| c.enabled = !c.enabled))
                    .child(flag("bp-class-raise", c.raised, "On raise (even when handled)", |c| c.raised = !c.raised))
                    .child(flag("bp-class-uncaught", c.uncaught, "On termination (uncaught)", |c| c.uncaught = !c.uncaught))
                    .child(div().text_size(px(12.)).text_color(Theme::text_dim()).child("A built-in name (ValueError) or the full name of an importable class (mypkg.errors.MyError); its subclasses stop too. A class defined in the script being run cannot be found by debugpy."))
                    .into_any_element()
            }
            Some(BpRow::Raised) => div()
                .flex()
                .flex_col()
                .gap(px(8.))
                .child(div().font_weight(gpui::FontWeight::SEMIBOLD).child("On raised exceptions"))
                .child(div().text_color(Theme::text_muted()).child("Stop wherever an exception is raised, even if a try block handles it (debugpy filter \"raised\")."))
                .into_any_element(),
            Some(BpRow::Uncaught) => div()
                .flex()
                .flex_col()
                .gap(px(8.))
                .child(div().font_weight(gpui::FontWeight::SEMIBOLD).child("On uncaught exceptions"))
                .child(div().text_color(Theme::text_muted()).child("Stop where an exception nothing handles is raised (debugpy filter \"uncaught\")."))
                .into_any_element(),
            None => div().into_any_element(),
        };
        let is_line = matches!(cur, Some(BpRow::Line(..)));
        let removable = is_line || matches!(cur, Some(BpRow::Class(..)));
        let (cur_rm, cur_go) = (cur.clone(), cur.clone());
        Some(
            div()
                .absolute()
                .top_0()
                .left_0()
                .size_full()
                .flex()
                .justify_center()
                .items_start()
                .pt(px(70.))
                .occlude()
                .child(
                    div()
                        .id("bp-dialog")
                        .w(px(760.))
                        .flex()
                        .flex_col()
                        .p(px(16.))
                        .gap(px(10.))
                        .bg(Theme::panel())
                        .border_1()
                        .border_color(Theme::divider())
                        .rounded(px(10.))
                        .map(crate::ui_kit::floating)
                        .text_size(px(13.))
                        .text_color(Theme::text())
                        .child(div().font_weight(gpui::FontWeight::SEMIBOLD).child("Breakpoints"))
                        .child(
                            div()
                                .flex()
                                .h(px(320.))
                                .gap(px(12.))
                                .child(
                                    div()
                                        .id("bp-list")
                                        .w(px(380.))
                                        .flex_shrink_0()
                                        .overflow_y_scroll()
                                        .border_1()
                                        .border_color(Theme::divider())
                                        .rounded(px(6.))
                                        .bg(Theme::editor_bg())
                                        .children(list),
                                )
                                .child(div().flex_1().min_w_0().child(details)),
                        )
                        .child(
                            div()
                                .flex()
                                .items_center()
                                .gap(px(8.))
                                .child(div().flex_1().min_w_0().text_size(px(12.)).text_color(Theme::text_dim()).child("Space: on/off · type: edit · Insert: + exception · Delete: remove · Enter: go to · Esc: close"))
                                .child(button("bp-add-class", "+ Exception", false, true).on_click(cx.listener(|this, _, _, cx| this.bp_add_class(cx))))
                                .child(button("bp-remove", "Remove", false, removable).on_click(cx.listener(move |this, _, _, cx| match cur_rm.clone() {
                                    Some(BpRow::Line(p, l, _)) => this.bp_remove(&p, l, cx),
                                    Some(BpRow::Class(i, _)) => this.bp_remove_class(i, cx),
                                    _ => {}
                                })))
                                .child(button("bp-goto", "Go to", false, is_line).on_click(cx.listener(move |this, _, w, cx| {
                                    if let Some(r) = cur_go.clone() {
                                        this.bp_go_to(&r, w, cx);
                                    }
                                })))
                                .child(button("bp-done", "Done", true, true).on_click(cx.listener(|this, _, _, cx| {
                                    this.debug.bp_dialog = None;
                                    this.bp_drop_empty_classes(cx);
                                    cx.notify();
                                }))),
                        ),
                )
                .into_any_element(),
        )
    }

    // --- Session ----------------------------------------------------------------------

    pub(super) fn debug_action(&mut self, action: Action, window: &mut Window, cx: &mut Context<Self>) -> bool {
        use Action as A;
        match action {
            A::ToggleBreakpoint => self.toggle_breakpoint_here(window, cx),
            A::ViewBreakpoints => self.open_bp_dialog(cx),
            A::MuteBreakpoints => self.toggle_mute_breakpoints(cx),
            A::SmartStepInto => self.smart_step_into(cx),
            A::AttachToProcess => self.list_processes(cx),
            A::SetNextStatement => {
                let Some(ed) = self.focused_editor(window, cx).or_else(|| self.active_editor().cloned()) else { return true };
                let line = ed.read(cx).caret_line_col().0;
                self.set_next_statement(ed, line, cx);
            }
            A::Debug => {
                self.reload_runs();
                let cfg = self.runs.file.selected_config().cloned().or_else(|| self.context_config(window, cx));
                match cfg {
                    Some(cfg) => self.start_debug(cfg, window, cx),
                    None => self.status = "Nothing to debug: open a Python file or select a configuration".into(),
                }
            }
            A::DebugContext => {
                let Some(ctx) = self.context_config(window, cx) else {
                    self.status = "Debug: open a Python file first".into();
                    return true;
                };
                self.reload_runs();
                let name = self.runs.file.use_for_file(ctx);
                self.save_runs();
                if let Some(cfg) = self.runs.file.get(&name).cloned() {
                    self.start_debug(cfg, window, cx);
                }
            }
            A::Resume | A::StepOver | A::StepInto | A::StepOut | A::RunToCursor => {
                let Some(thread) = self.debug.paused_thread() else {
                    self.status = "The debugger is not paused".into();
                    return true;
                };
                let command = match action {
                    A::StepOver => "next",
                    A::StepInto => "stepIn",
                    A::StepOut => "stepOut",
                    _ => "continue",
                };
                if action == A::RunToCursor {
                    let Some(ed) = self.active_editor().cloned() else { return true };
                    let (path, line) = (ed.read(cx).path.clone(), ed.read(cx).caret_line_col().0);
                    let Some(path) = path else { return true };
                    if let Some(s) = self.debug.session.as_mut() {
                        s.temp = Some((path.clone(), line));
                    }
                    self.send_breakpoints(&path, cx);
                }
                self.debug_request(command, json!({"threadId": thread}));
                self.set_running(cx);
            }
            A::Stop if self.debug.is_live() => self.stop_debug(cx),
            A::PauseProgram => {
                if self.debug.state() != Some(State::Running) {
                    self.status = "Pause: the program is not running".into();
                    return true;
                }
                self.pause_debug(cx);
            }
            A::AddToWatches => {
                // The selection or the word at the caret; nothing there: type one in.
                let text = self.active_editor().map(|ed| {
                    let e = ed.read(cx);
                    let sel = e.buffer.selected_text();
                    if sel.trim().is_empty() {
                        e.buffer.slice(e.buffer.word_range_at(e.buffer.primary().head))
                    } else {
                        sel
                    }
                });
                let text = text.map(|t| t.trim().to_string()).filter(|t| !t.is_empty() && !t.contains('\n'));
                self.debug.tab = DebugTab::Debugger;
                self.bottom = Some(BottomTool::Debug);
                match text {
                    Some(t) if self.focused_editor(window, cx).is_some() => self.add_watch(t, cx),
                    _ => self.debug.edit = Some(VarEdit { target: EditTarget::NewWatch, text: String::new() }),
                }
                window.focus(&self.debug.focus.clone());
            }
            A::RerunDebug => match (self.debug.last.clone(), self.debug.last_attach.clone()) {
                (Some(cfg), _) => self.start_debug(cfg, window, cx),
                (None, Some((pid, label))) => self.reattach(pid, label, window, cx),
                (None, None) => self.status = "Rerun: nothing has been debugged yet".into(),
            },
            A::EvaluateExpression => {
                if self.debug.paused_thread().is_none() {
                    self.status = "Evaluate: the debugger is not paused".into();
                    return true;
                }
                // The selection or the word at the caret goes into the console's input.
                if let Some(ed) = self.active_editor() {
                    let e = ed.read(cx);
                    let sel = e.buffer.selected_text();
                    let t = if sel.trim().is_empty() {
                        let r = e.buffer.word_range_at(e.buffer.primary().head);
                        e.buffer.slice(r)
                    } else {
                        sel
                    };
                    self.debug.input = t.trim().to_string();
                }
                self.debug.tab = DebugTab::Console;
                self.bottom = Some(BottomTool::Debug);
                window.focus(&self.debug.focus.clone());
            }
            _ => return false,
        }
        cx.notify();
        true
    }

    fn debug_request(&self, command: &str, args: Value) {
        if let Some(s) = &self.debug.session {
            let _ = s.client.request(command, args);
        }
    }

    fn set_running(&mut self, cx: &mut Context<Self>) {
        if let Some(s) = self.debug.session.as_mut() {
            if s.state != State::Ended {
                s.state = State::Running;
            }
        }
        self.debug.frames.clear();
        self.debug.threads.clear();
        self.debug.threads_open = false;
        self.debug.idle_watches();
        self.debug.exception = None;
        self.clear_exec_marks(cx);
    }

    fn clear_exec_marks(&mut self, cx: &mut Context<Self>) {
        let marked = std::mem::take(&mut self.debug.marked);
        for e in self.editors().cloned().collect::<Vec<_>>() {
            if e.read(cx).path.as_ref().is_some_and(|p| marked.contains(p)) || e.read(cx).exec_line.is_some() {
                e.update(cx, |e, cx| {
                    e.exec_line = None;
                    e.clear_inline_values();
                    cx.notify();
                });
            }
        }
    }

    /// Pause Program: debugpy pauses all threads; the main one is asked.
    fn pause_debug(&mut self, cx: &mut Context<Self>) {
        let Some(s) = &self.debug.session else { return };
        let client = s.client.clone();
        let threads = client.request("threads", json!({}));
        cx.spawn(async move |this, cx| {
            let list = match threads.await {
                Ok(Ok(v)) => v.get("threads").and_then(Value::as_array).cloned().unwrap_or_default(),
                _ => Vec::new(),
            };
            let main = list.iter().find(|t| t.get("name").and_then(Value::as_str) == Some("MainThread")).or(list.first());
            let id = main.and_then(|t| t.get("id")).and_then(Value::as_i64).unwrap_or(0);
            if let Ok(Err(e)) = client.request("pause", json!({"threadId": id})).await {
                let _ = this.update(cx, |this, cx| {
                    this.debug.info(format!("Cannot pause: {e}"));
                    cx.notify();
                });
            }
        })
        .detach();
        self.status = "Pausing…".into();
    }

    /// Debug a configuration (also pytest ones: a file, a folder or `file::test`).
    pub(super) fn start_debug(&mut self, cfg: RunConfig, window: &mut Window, cx: &mut Context<Self>) {
        if cfg.kind == brisk_run::RunKind::Compound {
            // One debug session at a time: debug the members one by one.
            self.show_toast(format!("'{}' is a Compound: run it, or debug its configurations one at a time", cfg.name), true, cx);
            return;
        }
        if self.before_launch(&cfg, super::before_launch_ui::Then::Debug, window, cx) {
            return;
        }
        self.save_all(cx);
        if self.debug.is_live() {
            self.stop_debug(cx);
        }
        self.debug.last = Some(cfg.clone());
        self.debug.last_attach = None;
        let python = match &cfg.interpreter {
            Some(p) => Some(brisk_run::resolve(&self.root, p)),
            None => self.runs.interpreter(&self.root).map(|i| i.path),
        };
        let Some(python) = python else {
            self.show_toast("No Python interpreter configured: create a .venv or install Python".into(), true, cx);
            return;
        };
        // pytest runs as `-m pytest <targets>` (a file, a folder or `file::test`); without the
        // Run window's reporting plugin its output is plain text in the console.
        if !self.env_files_ok(&cfg, cx) {
            return;
        }
        let cmd = brisk_run::build(&self.root, &cfg, &python);
        let launch = launch_for(&cmd);
        self.debug.cwd = launch.cwd.clone();
        self.debug.exception = None;
        let name = cfg.name.clone();
        self.debug.console.clear();
        self.debug.frames.clear();
        self.debug.idle_watches();
        self.debug.tab = DebugTab::Console;
        self.debug.info(cmd.display());
        self.bottom = Some(BottomTool::Debug);
        self.status = format!("Debugging '{name}'");
        let root = self.root.clone();
        self.debug.gen += 1;
        self.debug.pending = true;
        let gen = self.debug.gen;
        cx.spawn_in(window, async move |this, cx| {
            let adapter = cx.background_executor().spawn(async move { brisk_dap::adapter_command(&python) }).await;
            let started = adapter.and_then(|a| Client::start(&a.program, &a.args, &root, &[]).map_err(|e| format!("cannot start debugpy: {e}")));
            let _ = this.update_in(cx, |this, window, cx| {
                if this.debug.gen != gen {
                    // Stopped (or started again) while debugpy was starting.
                    if let Ok((client, _)) = &started {
                        client.kill();
                    }
                    return;
                }
                this.debug.pending = false;
                match started {
                Err(e) => {
                    this.debug.info(e.clone());
                    this.show_toast(e, true, cx);
                }
                    Ok((client, events)) => this.attach_session(client, events, name, ("launch", brisk_dap::launch_args(&launch)), window, cx),
                }
                cx.notify();
            });
        })
        .detach();
        cx.notify();
    }

    /// A started adapter becomes the session: initialize, then `req` (launch or attach).
    fn attach_session(&mut self, client: Arc<Client>, mut events: futures::channel::mpsc::UnboundedReceiver<Event>, name: String, req: (&'static str, Value), window: &mut Window, cx: &mut Context<Self>) {
        let task = cx.spawn_in(window, async move |this, cx| {
            while let Some(ev) = events.next().await {
                if this.update_in(cx, |this, window, cx| this.debug_event(ev, window, cx)).is_err() {
                    break;
                }
            }
        });
        let attached = req.0 == "attach";
        self.debug.session = Some(Session { client: client.clone(), name, state: State::Starting, _events: task, temp: None, exit_code: None, attached });
        cx.spawn(async move |this, cx| {
            let init = client.request("initialize", brisk_dap::initialize_args()).await;
            let err = match init {
                Ok(Ok(_)) => None,
                Ok(Err(e)) => Some(e),
                Err(_) => Some("no reply".to_string()),
            };
            if let Some(e) = err {
                let _ = this.update(cx, |this, cx| {
                    this.debug.info(format!("debugpy did not start: {e}"));
                    cx.notify();
                });
                return;
            }
            let (command, args) = req;
            let res = client.request(command, args).await;
            if let Ok(Err(e)) = res {
                let _ = this.update(cx, |this, cx| {
                    this.debug.info(format!("Cannot {command}: {e}"));
                    this.end_session(cx);
                    cx.notify();
                });
            }
        })
        .detach();
    }

    fn stop_debug(&mut self, cx: &mut Context<Self>) {
        self.debug.gen += 1;
        self.debug.pending = false;
        if let Some(s) = self.debug.session.as_ref().filter(|s| s.state != State::Ended) {
            let rx = s.client.request("disconnect", json!({"terminateDebuggee": !s.attached}));
            let client = s.client.clone();
            cx.spawn(async move |_, cx| {
                let _ = futures::future::select(rx, cx.background_executor().timer(std::time::Duration::from_secs(2))).await;
                client.kill();
            })
            .detach();
        }
        let detached = self.debug.session.as_ref().is_some_and(|s| s.attached && s.state != State::Ended);
        self.debug.info(if detached { "Detached: the process keeps running" } else { "Stopped" });
        self.end_session(cx);
    }

    fn end_session(&mut self, cx: &mut Context<Self>) {
        if let Some(s) = self.debug.session.as_mut() {
            s.state = State::Ended;
        }
        self.debug.frames.clear();
        self.debug.threads.clear();
        self.debug.threads_open = false;
        self.debug.idle_watches();
        self.debug.edit = None;
        self.debug.exception = None;
        self.clear_exec_marks(cx);
        for e in self.editors().cloned().collect::<Vec<_>>() {
            e.update(cx, |e, cx| {
                e.clear_breakpoints_verified();
                cx.notify();
            });
        }
    }

    fn debug_event(&mut self, ev: Event, window: &mut Window, cx: &mut Context<Self>) {
        let Event::Dap { event, body } = ev else {
            if self.debug.state().is_some_and(|s| s != State::Ended) {
                let code = self.debug.session.as_ref().and_then(|s| s.exit_code);
                self.debug.info(match code {
                    Some(c) => format!("Process finished with exit code {c}"),
                    None => "Debugger disconnected".into(),
                });
                self.end_session(cx);
            }
            cx.notify();
            return;
        };
        match event.as_str() {
            "initialized" => {
                for (p, _) in self.all_breakpoints(cx) {
                    self.send_breakpoints(&p, cx);
                }
                self.debug_request("setExceptionBreakpoints", self.debug.exception_args());
                self.debug_request("configurationDone", json!({}));
                if let Some(s) = self.debug.session.as_mut() {
                    s.state = State::Running;
                }
            }
            "output" => {
                let cat = body.get("category").and_then(Value::as_str).unwrap_or("console");
                let text = body.get("output").and_then(Value::as_str).unwrap_or("");
                match cat {
                    "stdout" => self.debug.print(text, Line::Out),
                    "stderr" => self.debug.print(text, Line::Err),
                    "telemetry" | "important" => {}
                    _ => self.debug.print(text, Line::Info),
                }
            }
            "stopped" => {
                let thread = body.get("threadId").and_then(Value::as_i64).unwrap_or(1);
                if let Some(s) = self.debug.session.as_mut() {
                    s.state = State::Paused(thread);
                    // Run to Cursor reached (or another breakpoint): the temporary one goes.
                    if let Some((p, _)) = s.temp.take() {
                        self.send_breakpoints(&p, cx);
                    }
                }
                if body.get("reason").and_then(Value::as_str) == Some("exception") {
                    let d = body.get("text").or_else(|| body.get("description")).and_then(Value::as_str).unwrap_or("exception");
                    self.debug.exception = Some(d.to_string());
                    self.exception_info(thread, cx);
                }
                self.debug.tab = DebugTab::Debugger;
                self.bottom = Some(BottomTool::Debug);
                self.load_frames(thread, window, cx);
                self.load_threads(cx);
            }
            "continued" => self.set_running(cx),
            "exited" => {
                if let Some(s) = self.debug.session.as_mut() {
                    s.exit_code = body.get("exitCode").and_then(Value::as_i64);
                }
            }
            "terminated" => {
                let code = self.debug.session.as_ref().and_then(|s| s.exit_code);
                if self.debug.state() != Some(State::Ended) {
                    self.debug.info(match code {
                        Some(c) => format!("\nProcess finished with exit code {c}"),
                        None => "\nProcess finished".into(),
                    });
                }
                self.end_session(cx);
                self.debug_request("disconnect", json!({}));
            }
            _ => {}
        }
        cx.notify();
    }

    /// Stopped on an exception: its type, message and traceback (DAP exceptionInfo).
    fn exception_info(&mut self, thread: i64, cx: &mut Context<Self>) {
        let Some(s) = &self.debug.session else { return };
        let rx = s.client.request("exceptionInfo", json!({"threadId": thread}));
        cx.spawn(async move |this, cx| {
            let res = rx.await;
            let _ = this.update(cx, |this, cx| {
                let (head, stack) = match res {
                    Ok(Ok(v)) => {
                        let id = v.get("exceptionId").and_then(Value::as_str).unwrap_or("");
                        let ty = v.pointer("/details/typeName").and_then(Value::as_str).unwrap_or(id);
                        let msg = v.pointer("/details/message").or_else(|| v.get("description")).and_then(Value::as_str).unwrap_or("");
                        let head = if msg.is_empty() { ty.to_string() } else { format!("{ty}: {msg}") };
                        (head, v.pointer("/details/stackTrace").and_then(Value::as_str).unwrap_or("").to_string())
                    }
                    _ => (this.debug.exception.clone().unwrap_or_else(|| "exception".into()), String::new()),
                };
                this.debug.print("Traceback (most recent call last):\n", Line::Err);
                let stack = python_order(&stack);
                if !stack.is_empty() {
                    this.debug.print(&format!("{stack}\n"), Line::Err);
                }
                this.debug.print(&format!("{head}\n"), Line::Err);
                this.debug.exception = Some(head);
                cx.notify();
            });
        })
        .detach();
    }

    /// A console link: `File "x.py", line N` (relative to the session's directory).
    fn console_link(&self, path: &str, line: usize) -> Option<(PathBuf, usize)> {
        let p = PathBuf::from(path);
        let p = if p.is_absolute() { p } else { self.debug.cwd.join(p) };
        p.is_file().then_some((p, line))
    }

    pub(super) fn open_console_link(&mut self, path: &Path, line: usize, window: &mut Window, cx: &mut Context<Self>) {
        self.open_file(path, window, cx);
        if let Some(ed) = self.active_editor().cloned() {
            ed.update(cx, |e, cx| e.go_to_line(line.saturating_sub(1), cx));
        }
        self.focus_editor(window, cx);
        cx.notify();
    }

    // --- Smart Step Into ----------------------------------------------------------------

    /// Shift+F7: the calls on the stop line; one call steps into it right away.
    fn smart_step_into(&mut self, cx: &mut Context<Self>) {
        let (Some(thread), Some(frame)) = (self.debug.paused_thread(), self.debug.frames.get(self.debug.frame).map(|f| f.id)) else {
            self.status = "The debugger is not paused".into();
            return;
        };
        let Some(s) = &self.debug.session else { return };
        let rx = s.client.request("stepInTargets", json!({"frameId": frame}));
        // The stop's statement, from its editor (or the file on disk).
        let stmt = self.debug.frames.get(self.debug.frame).and_then(|f| {
            let p = f.path.clone()?;
            let lines: Vec<String> = match self.editors().find(|e| e.read(cx).path.as_ref() == Some(&p)) {
                Some(ed) => {
                    let b = &ed.read(cx).buffer;
                    (f.line..(f.line + 30).min(b.len_lines())).map(|l| b.line(l)).collect()
                }
                None => std::fs::read_to_string(&p).ok()?.lines().skip(f.line).take(30).map(String::from).collect(),
            };
            Some(statement_text(&lines, 0))
        });
        cx.spawn(async move |this, cx| {
            let res = rx.await;
            let _ = this.update(cx, |this, cx| {
                if this.debug.paused_thread() != Some(thread) {
                    return;
                }
                let targets: Vec<(i64, String)> = match &res {
                    Ok(Ok(v)) => v
                        .get("targets")
                        .and_then(Value::as_array)
                        .map(|a| a.iter().filter_map(|t| Some((t.get("id")?.as_i64()?, t.get("label").and_then(Value::as_str).unwrap_or("?").to_string()))).collect())
                        .unwrap_or_default(),
                    _ => Vec::new(),
                };
                let targets = match &stmt {
                    Some(s) => targets_on_statement(s, targets),
                    None => targets,
                };
                match targets.len() {
                    // Nothing to choose (or no support): a plain Step Into.
                    0 => {
                        this.debug_request("stepIn", json!({"threadId": thread}));
                        this.set_running(cx);
                    }
                    1 => {
                        this.debug_request("stepIn", json!({"threadId": thread, "targetId": targets[0].0}));
                        this.set_running(cx);
                    }
                    _ => this.debug.chooser = Some(Chooser { kind: ChooserKind::StepInto(thread), targets, sel: 0 }),
                }
                cx.notify();
            });
        })
        .detach();
    }

    /// Step into the chosen call / attach to the chosen process.
    pub(super) fn chooser_go(&mut self, i: usize, window: &mut Window, cx: &mut Context<Self>) {
        let Some(st) = self.debug.chooser.take() else { return };
        if let Some((id, label)) = st.targets.get(i) {
            match st.kind {
                ChooserKind::StepInto(thread) => {
                    if self.debug.paused_thread() == Some(thread) {
                        self.debug_request("stepIn", json!({"threadId": thread, "targetId": id}));
                        self.set_running(cx);
                    }
                }
                ChooserKind::Attach => self.attach_to_process(*id as u32, label.clone(), window, cx),
            }
        }
        cx.notify();
    }

    // --- Set Next Statement -----------------------------------------------------------------

    /// Jump to Cursor / a dragged execution arrow: the paused thread continues from `line`
    /// of this editor's file without running the lines between (debugpy gotoTargets + goto).
    pub(super) fn set_next_statement(&mut self, ed: Entity<Editor>, line: usize, cx: &mut Context<Self>) {
        let Some(thread) = self.debug.paused_thread() else {
            self.status = "Jump to Cursor: the debugger is not paused".into();
            return;
        };
        let Some(path) = ed.read(cx).path.clone() else { return };
        let Some(s) = &self.debug.session else { return };
        let client = s.client.clone();
        // debugpy moves the innermost frame, whatever file the target names: another file
        // would mean that line number in the paused one. Other limits (into a loop, out of the
        // function) debugpy reports itself, and the program stays paused where it was.
        if self.debug.frames.first().and_then(|f| f.path.as_ref()) != Some(&path) {
            let msg = format!("Cannot jump to line {}: only in the file where the program is paused", line + 1);
            self.debug.info(msg.clone());
            self.show_toast(msg, true, cx);
            return;
        }
        cx.spawn(async move |this, cx| {
            let targets = client.request("gotoTargets", json!({"source": {"path": path.to_string_lossy()}, "line": line + 1})).await;
            let target = match &targets {
                Ok(Ok(v)) => v.pointer("/targets/0/id").and_then(Value::as_i64),
                _ => None,
            };
            let res = match target {
                Some(id) => match client.request("goto", json!({"threadId": thread, "targetId": id})).await {
                    Ok(Ok(_)) => Ok(()),
                    Ok(Err(e)) => Err(e),
                    Err(_) => Err("no reply".into()),
                },
                None => Err(match targets {
                    Ok(Err(e)) => e,
                    _ => "no target on this line".into(),
                }),
            };
            let _ = this.update(cx, |this, cx| {
                match res {
                    // The thread stops again on the new line ("stopped", reason "goto").
                    Ok(()) => this.status = format!("Next statement: line {}", line + 1),
                    Err(e) => {
                        let msg = format!("Cannot jump to line {}: {e}", line + 1);
                        this.debug.info(msg.clone());
                        this.show_toast(msg, true, cx);
                    }
                }
                cx.notify();
            });
        })
        .detach();
    }

    // --- Attach to Process ----------------------------------------------------------------

    /// Ctrl+Alt+F5: running Python processes (not debugpy's own) to choose from.
    fn list_processes(&mut self, cx: &mut Context<Self>) {
        self.status = "Looking for Python processes…".into();
        cx.spawn(async move |this, cx| {
            let ps = cx.background_executor().spawn(async move { process_lines() }).await;
            let _ = this.update(cx, |this, cx| {
                let procs = brisk_dap::python_processes(&ps, std::process::id());
                if procs.is_empty() {
                    this.status = "No Python processes to attach to".into();
                } else {
                    this.status.clear();
                    let targets = procs.into_iter().map(|(pid, cmd)| (pid as i64, format!("{pid}  {}    {cmd}", process_target(&cmd)))).collect();
                    this.debug.chooser = Some(Chooser { kind: ChooserKind::Attach, targets, sel: 0 });
                }
                cx.notify();
            });
        })
        .detach();
    }

    /// Rerun after an Attach: the same process again if it still runs.
    fn reattach(&mut self, pid: u32, label: String, window: &mut Window, cx: &mut Context<Self>) {
        cx.spawn_in(window, async move |this, cx| {
            let ps = cx.background_executor().spawn(async move { process_lines() }).await;
            let alive = brisk_dap::python_processes(&ps, std::process::id()).iter().any(|p| p.0 == pid);
            let _ = this.update_in(cx, |this, window, cx| {
                if alive {
                    this.attach_to_process(pid, label, window, cx);
                } else {
                    let msg = format!("Process {pid} has exited: use Attach to Process (Ctrl+Alt+F5) to choose another");
                    this.debug.info(msg.clone());
                    this.show_toast(msg, false, cx);
                }
                cx.notify();
            });
        })
        .detach();
    }

    /// debugpy's adapter (with the project interpreter) injects itself into `pid`.
    pub(super) fn attach_to_process(&mut self, pid: u32, label: String, window: &mut Window, cx: &mut Context<Self>) {
        if self.debug.is_live() {
            self.stop_debug(cx);
        }
        let Some(python) = self.runs.interpreter(&self.root).map(|i| i.path) else {
            self.show_toast("No Python interpreter configured: create a .venv or install Python".into(), true, cx);
            return;
        };
        let name = format!("pid {pid}");
        self.debug.last = None;
        self.debug.last_attach = Some((pid, label.clone()));
        self.debug.exception = None;
        self.debug.console.clear();
        self.debug.frames.clear();
        self.debug.idle_watches();
        self.debug.tab = DebugTab::Console;
        self.debug.info(format!("Attaching to {label}"));
        self.bottom = Some(BottomTool::Debug);
        self.status = format!("Attaching to process {pid}");
        let root = self.root.clone();
        self.debug.gen += 1;
        self.debug.pending = true;
        let gen = self.debug.gen;
        cx.spawn_in(window, async move |this, cx| {
            let adapter = cx.background_executor().spawn(async move { brisk_dap::adapter_command(&python) }).await;
            let started = adapter.and_then(|a| Client::start(&a.program, &a.args, &root, &[]).map_err(|e| format!("cannot start debugpy: {e}")));
            let _ = this.update_in(cx, |this, window, cx| {
                if this.debug.gen != gen {
                    if let Ok((client, _)) = &started {
                        client.kill();
                    }
                    return;
                }
                this.debug.pending = false;
                match started {
                    Err(e) => {
                        this.debug.info(e.clone());
                        this.show_toast(e, true, cx);
                    }
                    Ok((client, events)) => this.attach_session(client, events, name, ("attach", brisk_dap::attach_args(pid, true)), window, cx),
                }
                cx.notify();
            });
        })
        .detach();
        cx.notify();
    }

    pub(super) fn chooser_key(&mut self, e: &gpui::KeyDownEvent, window: &mut Window, cx: &mut Context<Self>) {
        let Some(st) = self.debug.chooser.as_mut() else { return };
        let n = st.targets.len();
        match e.keystroke.key.as_str() {
            "escape" => self.debug.chooser = None,
            "up" => st.sel = (st.sel + n - 1) % n.max(1),
            "down" | "tab" => st.sel = (st.sel + 1) % n.max(1),
            "enter" | "f7" => {
                let i = st.sel;
                self.chooser_go(i, window, cx);
            }
            _ => {}
        }
        cx.notify();
    }

    pub(super) fn render_chooser(&mut self, cx: &mut Context<Self>) -> Option<gpui::AnyElement> {
        let st = self.debug.chooser.as_ref()?;
        let title = match st.kind {
            ChooserKind::StepInto(_) => "Smart Step Into  (Enter: step into · Esc: cancel)",
            ChooserKind::Attach => "Attach to Process  (Enter: attach · Esc: cancel)",
        };
        let rows = st.targets.iter().enumerate().map(|(i, (_, label))| {
            let selected = i == st.sel;
            div()
                .id(("smart-step", i))
                .h(px(24.))
                .px(px(10.))
                .flex()
                .items_center()
                .rounded(px(4.))
                .cursor_pointer()
                .font_family(mono_font())
                .when(selected, |x| x.bg(Theme::selection_row()))
                .when(!selected, |x| x.hover(|s| s.bg(Theme::hover_row())))
                .overflow_hidden()
                .whitespace_nowrap()
                .text_ellipsis()
                .child(label.clone())
                .on_click(cx.listener(move |this, _, w, cx| this.chooser_go(i, w, cx)))
        });
        Some(
            div()
                .absolute()
                .top_0()
                .left_0()
                .size_full()
                .flex()
                .justify_center()
                .items_start()
                .pt(px(120.))
                .occlude()
                .on_mouse_down(gpui::MouseButton::Left, cx.listener(|this, _, _, cx| {
                    this.debug.chooser = None;
                    cx.notify();
                }))
                .child(
                    div()
                        .id("smart-step-popup")
                        .min_w(px(260.))
                        .max_w(px(900.))
                        .flex()
                        .flex_col()
                        .p(px(6.))
                        .gap(px(2.))
                        .bg(Theme::panel())
                        .border_1()
                        .border_color(Theme::divider())
                        .rounded(px(8.))
                        .map(crate::ui_kit::floating)
                        .text_size(px(13.))
                        .text_color(Theme::text())
                        .on_mouse_down(gpui::MouseButton::Left, |_, _, cx| cx.stop_propagation())
                        .child(div().px(px(10.)).pb(px(4.)).text_size(px(12.)).text_color(Theme::text_dim()).child(title))
                        .children(rows),
                )
                .into_any_element(),
        )
    }

    /// The thread list for the chooser above Frames.
    fn load_threads(&mut self, cx: &mut Context<Self>) {
        let Some(s) = &self.debug.session else { return };
        let rx = s.client.request("threads", json!({}));
        cx.spawn(async move |this, cx| {
            let Ok(Ok(v)) = rx.await else { return };
            let threads: Vec<(i64, String)> = v
                .get("threads")
                .and_then(Value::as_array)
                .map(|a| a.iter().filter_map(|t| Some((t.get("id")?.as_i64()?, t.get("name").and_then(Value::as_str).unwrap_or("").to_string()))).collect())
                .unwrap_or_default();
            let _ = this.update(cx, |this, cx| {
                if this.debug.paused_thread().is_some() {
                    this.debug.threads = threads;
                    cx.notify();
                }
            });
        })
        .detach();
    }

    /// Another thread in the chooser: its stack and variables; steps and Resume go to it.
    pub(super) fn select_thread(&mut self, id: i64, window: &mut Window, cx: &mut Context<Self>) {
        self.debug.threads_open = false;
        let Some(s) = self.debug.session.as_mut().filter(|s| matches!(s.state, State::Paused(_))) else { return };
        s.state = State::Paused(id);
        self.debug.frames.clear();
        self.load_frames(id, window, cx);
        cx.notify();
    }

    fn render_threads(&self, cx: &mut Context<Self>) -> Option<gpui::AnyElement> {
        let d = &self.debug;
        let cur = d.paused_thread()?;
        if d.threads.len() < 2 {
            return None;
        }
        let name = |id: i64| d.threads.iter().find(|t| t.0 == id).map(|t| t.1.clone()).unwrap_or_else(|| format!("Thread {id}"));
        let head = div()
            .id("dbg-thread")
            .h(px(24.))
            .px(px(8.))
            .flex()
            .items_center()
            .gap(px(6.))
            .cursor_pointer()
            .border_b_1()
            .border_color(Theme::divider())
            .hover(|s| s.bg(Theme::hover_row()))
            .child(div().text_color(Theme::text_dim()).child("Thread"))
            .child(div().flex_1().child(name(cur)))
            .child(crate::icons::icon(if d.threads_open { "chevron-up" } else { "chevron-down" }, 12., Theme::text_dim()))
            .on_click(cx.listener(|this, _, _, cx| {
                this.debug.threads_open = !this.debug.threads_open;
                cx.notify();
            }));
        let list = d.threads.iter().map(|(id, n)| {
            let id = *id;
            div()
                .id(("dbg-thread-row", id as u64))
                .h(px(22.))
                .pl(px(20.))
                .flex()
                .items_center()
                .cursor_pointer()
                .when(id == cur, |x| x.bg(Theme::selection_row()))
                .when(id != cur, |x| x.hover(|s| s.bg(Theme::hover_row())))
                .child(n.clone())
                .on_click(cx.listener(move |this, _, w, cx| this.select_thread(id, w, cx)))
        });
        Some(
            div()
                .flex()
                .flex_col()
                .child(head)
                .when(d.threads_open, |x| x.children(list).child(div().h(px(1.)).bg(Theme::divider())))
                .into_any_element(),
        )
    }

    fn load_frames(&mut self, thread: i64, window: &mut Window, cx: &mut Context<Self>) {
        let Some(s) = &self.debug.session else { return };
        let rx = s.client.request("stackTrace", json!({"threadId": thread, "startFrame": 0, "levels": 100}));
        cx.spawn_in(window, async move |this, cx| {
            let Ok(Ok(v)) = rx.await else { return };
            let frames = brisk_dap::parse_frames(&v);
            let _ = this.update_in(cx, |this, window, cx| {
                this.debug.frames = frames;
                this.select_frame(0, window, cx);
            });
        })
        .detach();
    }

    /// Show a frame: its line in the editor, its variables.
    pub(super) fn select_frame(&mut self, i: usize, window: &mut Window, cx: &mut Context<Self>) {
        let Some(f) = self.debug.frames.get(i).cloned() else { return };
        self.debug.frame = i;
        self.clear_exec_marks(cx);
        if let Some(p) = f.path.clone().filter(|p| p.is_file()) {
            let focus_back = self.debug.focus.is_focused(window);
            self.open_file(&p, window, cx);
            if let Some(ed) = self.active_editor().cloned() {
                ed.update(cx, |e, cx| {
                    e.exec_line = Some(f.line);
                    e.go_to_line(f.line, cx);
                });
            }
            self.debug.marked.push(p);
            if focus_back {
                window.focus(&self.debug.focus.clone());
            }
        }
        self.debug.idle_watches();
        self.evaluate_watches(cx);
        let Some(s) = &self.debug.session else { return };
        let rx = s.client.request("scopes", json!({"frameId": f.id}));
        let client = s.client.clone();
        let frame_path = f.path.clone();
        let frame_line = f.line;
        cx.spawn(async move |this, cx| {
            let Ok(Ok(v)) = rx.await else { return };
            let scopes = brisk_dap::parse_scopes(&v);
            // The first scope (locals) opens right away, like PyCharm's flat list.
            let mut first = match scopes.first() {
                Some(s) => client.request("variables", json!({"variablesReference": s.reference})).await.ok().and_then(Result::ok).map(|v| brisk_dap::parse_variables(&v)),
                None => None,
            };
            // Values just returned by calls first.
            if let Some(v) = first.as_mut() {
                v.sort_by_key(|v| return_label(&v.name).is_none());
            }
            let _ = this.update(cx, |this, cx| {
                let mut rows = Vec::new();
                for (k, s) in scopes.into_iter().enumerate() {
                    let open = k == 0 && first.is_some();
                    let parent = s.reference;
                    rows.push(VarRow { depth: 0, var: s, expanded: open, kind: RowKind::Scope, parent: 0 });
                    if open {
                        rows.extend(first.clone().unwrap_or_default().into_iter().map(|v| VarRow::var(1, v, parent)));
                    }
                }
                let n = this.debug.watch_len();
                this.debug.vars.truncate(n);
                this.debug.vars.extend(rows);
                if let (Some(p), Some(vars)) = (frame_path.as_ref(), first.as_ref()) {
                    if let Some(ed) = this.editors().find(|e| e.read(cx).path.as_ref() == Some(p)).cloned() {
                        let lines: Vec<String> = (0..=frame_line.min(ed.read(cx).buffer.len_lines().saturating_sub(1))).map(|l| ed.read(cx).buffer.line(l)).collect();
                        let values = inline_values(&lines, frame_line, vars);
                        ed.update(cx, |e, cx| {
                            e.set_inline_values(values);
                            cx.notify();
                        });
                    }
                }
                cx.notify();
            });
        })
        .detach();
    }

    /// Expand or collapse a variable row.
    pub(super) fn toggle_var(&mut self, i: usize, cx: &mut Context<Self>) {
        let Some(row) = self.debug.vars.get(i).cloned() else { return };
        self.debug.sel = Some(i);
        if row.var.reference <= 0 {
            return;
        }
        if row.expanded {
            let end = (i + 1..self.debug.vars.len()).find(|&j| self.debug.vars[j].depth <= row.depth).unwrap_or(self.debug.vars.len());
            self.debug.vars.drain(i + 1..end);
            self.debug.vars[i].expanded = false;
            cx.notify();
            return;
        }
        let Some(s) = &self.debug.session else { return };
        let rx = s.client.request("variables", json!({"variablesReference": row.var.reference}));
        cx.spawn(async move |this, cx| {
            let Ok(Ok(v)) = rx.await else { return };
            let kids = brisk_dap::parse_variables(&v);
            let _ = this.update(cx, |this, cx| {
                if this.debug.vars.get(i).map(|r| r.var.reference) != Some(row.var.reference) {
                    return;
                }
                this.debug.vars[i].expanded = true;
                let at = i + 1;
                for (k, v) in kids.into_iter().enumerate() {
                    this.debug.vars.insert(at + k, VarRow::var(row.depth + 1, v, row.var.reference));
                }
                cx.notify();
            });
        })
        .detach();
    }

    // --- Value on hover ---------------------------------------------------------------

    /// Mouse rest on a name while paused: `name = {type} value` and its first children, in
    /// the editor's hover popup. False when the debugger is not paused here (docs instead).
    pub(super) fn debug_hover(&mut self, ed: &Entity<Editor>, pos: usize, at: gpui::Point<gpui::Pixels>, cx: &mut Context<Self>) -> bool {
        let frame = self.debug.frames.get(self.debug.frame).map(|f| f.id);
        let live = self.debug.session.as_ref().filter(|s| matches!(s.state, State::Paused(_)));
        let (Some(s), Some(frame)) = (live, frame) else { return false };
        let e = ed.read(cx);
        if !e.is_python() || pos >= e.buffer.len_chars() {
            return false;
        }
        let (line, col) = e.buffer.pos_to_line_col(pos);
        let Some(expr) = hover_expression(&e.buffer.line(line), col) else { return false };
        let client = s.client.clone();
        let ed = ed.clone();
        cx.spawn(async move |this, cx| {
            let res = client.request("evaluate", json!({"expression": expr, "frameId": frame, "context": "hover"})).await;
            let Ok(Ok(v)) = res else {
                // Not a value here (a module, an unknown name): the documentation.
                let _ = this.update(cx, |this, cx| this.quick_doc(ed, Some((pos, at)), cx));
                return;
            };
            let (value, reference) = brisk_dap::parse_evaluate(&v);
            let ty = v.get("type").and_then(Value::as_str).unwrap_or("");
            let mut text = if ty.is_empty() { format!("{expr} = {value}") } else { format!("{expr} = {{{ty}}} {value}") };
            if reference > 0 {
                if let Ok(Ok(kids)) = client.request("variables", json!({"variablesReference": reference})).await {
                    // debugpy's "special variables" / "function variables" groups are left out.
                    let kids: Vec<Variable> = brisk_dap::parse_variables(&kids).into_iter().filter(|k| !k.name.contains(' ')).collect();
                    for k in kids.iter().take(30) {
                        let v: String = k.value.chars().take(120).collect();
                        text.push_str(&if k.type_name.is_empty() { format!("\n    {} = {v}", k.name) } else { format!("\n    {} = {{{}}} {v}", k.name, k.type_name) });
                    }
                    if kids.len() > 30 {
                        text.push_str(&format!("\n    … {} more", kids.len() - 30));
                    }
                }
            }
            let md = format!("```\n{text}\n```");
            let _ = ed.update(cx, |e, cx| e.show_doc(&md, crate::editor::Anchor::Mouse(pos, at), cx));
        })
        .detach();
        true
    }

    // --- Watches, Set Value, Copy Value ---------------------------------------------------

    /// Evaluate every watch in the selected frame (on each stop and after changes).
    fn evaluate_watches(&mut self, cx: &mut Context<Self>) {
        let frame = self.debug.frames.get(self.debug.frame).map(|f| f.id);
        let live = self.debug.session.as_ref().filter(|s| matches!(s.state, State::Paused(_)));
        let (Some(s), Some(frame)) = (live, frame) else {
            let rows = self.debug.watch_section(None);
            self.debug.set_watch_rows(rows);
            return;
        };
        let client = s.client.clone();
        let exprs = self.debug.watches.clone();
        cx.spawn(async move |this, cx| {
            let mut vals = Vec::new();
            for e in &exprs {
                let r = client.request("evaluate", json!({"expression": e, "frameId": frame, "context": "watch"})).await;
                vals.push(match r {
                    Ok(Ok(v)) => {
                        let (value, reference) = brisk_dap::parse_evaluate(&v);
                        let type_name = v.get("type").and_then(Value::as_str).unwrap_or("").to_string();
                        Ok(Variable { name: e.clone(), value, type_name, reference })
                    }
                    Ok(Err(e)) => Err(e),
                    Err(_) => Err("no reply".into()),
                });
            }
            let _ = this.update(cx, |this, cx| {
                // Stale: the watches or the frame changed meanwhile.
                if this.debug.watches != exprs || this.debug.frames.get(this.debug.frame).map(|f| f.id) != Some(frame) {
                    return;
                }
                let rows = this.debug.watch_section(Some(vals));
                this.debug.set_watch_rows(rows);
                cx.notify();
            });
        })
        .detach();
    }

    fn watches_changed(&mut self, cx: &mut Context<Self>) {
        save_watches(&self.root, &self.debug.watches);
        self.evaluate_watches(cx);
        cx.notify();
    }

    pub(super) fn add_watch(&mut self, expr: String, cx: &mut Context<Self>) {
        let expr = expr.trim().to_string();
        if expr.is_empty() {
            return;
        }
        if !self.debug.watches.contains(&expr) {
            self.debug.watches.push(expr);
        }
        self.watches_changed(cx);
    }

    pub(super) fn remove_watch(&mut self, i: usize, cx: &mut Context<Self>) {
        if i < self.debug.watches.len() {
            self.debug.watches.remove(i);
            self.watches_changed(cx);
        }
    }

    fn edit_watch(&mut self, i: usize, expr: String, cx: &mut Context<Self>) {
        let expr = expr.trim().to_string();
        if expr.is_empty() {
            return self.remove_watch(i, cx);
        }
        if let Some(w) = self.debug.watches.get_mut(i) {
            *w = expr;
            self.watches_changed(cx);
        }
    }

    /// F2 / double click: edit a watch's expression, or set a variable's value.
    pub(super) fn begin_edit(&mut self, i: usize) {
        let Some(r) = self.debug.vars.get(i) else { return };
        self.debug.sel = Some(i);
        self.debug.edit = match r.kind {
            RowKind::Watch(w) => Some(VarEdit { target: EditTarget::Watch(w), text: self.debug.watches.get(w).cloned().unwrap_or_default() }),
            RowKind::WatchHeader => Some(VarEdit { target: EditTarget::NewWatch, text: String::new() }),
            RowKind::Var if r.parent > 0 && self.debug.paused_thread().is_some() => Some(VarEdit { target: EditTarget::Value(i), text: r.var.value.clone() }),
            _ => None,
        };
    }

    pub(super) fn commit_edit(&mut self, cx: &mut Context<Self>) {
        let Some(ed) = self.debug.edit.take() else { return };
        match ed.target {
            EditTarget::NewWatch => self.add_watch(ed.text, cx),
            EditTarget::Watch(w) => self.edit_watch(w, ed.text, cx),
            EditTarget::Value(i) => self.set_value(i, ed.text, cx),
        }
        cx.notify();
    }

    /// Set Value (DAP setVariable) of a variable row.
    pub(super) fn set_value(&mut self, i: usize, value: String, cx: &mut Context<Self>) {
        let Some(row) = self.debug.vars.get(i).cloned().filter(|r| r.kind == RowKind::Var && r.parent > 0) else { return };
        let Some(s) = self.debug.session.as_ref().filter(|s| matches!(s.state, State::Paused(_))) else { return };
        let rx = s.client.request("setVariable", json!({"variablesReference": row.parent, "name": row.var.name, "value": value}));
        cx.spawn(async move |this, cx| {
            let res = rx.await;
            let _ = this.update(cx, |this, cx| {
                match res {
                    Ok(Ok(v)) => {
                        let at = if this.debug.vars.get(i).is_some_and(|r| r.parent == row.parent && r.var.name == row.var.name) {
                            Some(i)
                        } else {
                            this.debug.vars.iter().position(|r| r.parent == row.parent && r.var.name == row.var.name)
                        };
                        if let Some(at) = at {
                            if this.debug.vars[at].expanded {
                                // Collapse: the children are of the old value.
                                this.toggle_var(at, cx);
                            }
                            let r = &mut this.debug.vars[at];
                            r.var.value = v.get("value").and_then(Value::as_str).unwrap_or("").to_string();
                            if let Some(t) = v.get("type").and_then(Value::as_str) {
                                r.var.type_name = t.to_string();
                            }
                            r.var.reference = v.get("variablesReference").and_then(Value::as_i64).unwrap_or(0);
                        }
                        this.evaluate_watches(cx);
                    }
                    Ok(Err(e)) => {
                        this.status = format!("Cannot set value: {e}");
                        this.debug.info(format!("Cannot set {}: {e}", row.var.name));
                    }
                    Err(_) => {}
                }
                cx.notify();
            });
        })
        .detach();
    }

    pub(super) fn copy_value(&mut self, i: usize, cx: &mut Context<Self>) {
        let Some(r) = self.debug.vars.get(i) else { return };
        if matches!(r.kind, RowKind::Scope | RowKind::WatchHeader) {
            return;
        }
        cx.write_to_clipboard(gpui::ClipboardItem::new_string(r.var.value.clone()));
        self.status = format!("Copied the value of {}", r.var.name);
    }

    /// Keys in the variables tree (the Debugger tab has focus).
    fn vars_key(&mut self, k: &gpui::Keystroke, cx: &mut Context<Self>) -> bool {
        let ctrl = k.modifiers.control || k.modifiers.platform;
        if let Some(ed) = self.debug.edit.as_mut() {
            match k.key.as_str() {
                "enter" => self.commit_edit(cx),
                "escape" => self.debug.edit = None,
                "backspace" => {
                    ed.text.pop();
                }
                // gpui gives Space no key_char.
                "space" if !ctrl => ed.text.push(' '),
                "v" if ctrl => {
                    if let Some(t) = cx.read_from_clipboard().and_then(|c| c.text()) {
                        ed.text.push_str(t.lines().next().unwrap_or(""));
                    }
                }
                _ if ctrl || k.modifiers.alt => return false,
                _ => match k.key_char.as_deref() {
                    Some(t) if !t.chars().any(char::is_control) => ed.text.push_str(t),
                    _ => return false,
                },
            }
            cx.notify();
            return true;
        }
        let n = self.debug.vars.len();
        let sel = self.debug.sel.filter(|&s| s < n);
        match (k.key.as_str(), ctrl) {
            ("c", true) => {
                if let Some(s) = sel {
                    self.copy_value(s, cx);
                }
            }
            (_, true) => return false,
            ("down", _) if n > 0 => self.debug.sel = Some(sel.map_or(0, |s| (s + 1).min(n - 1))),
            ("up", _) if n > 0 => self.debug.sel = Some(sel.map_or(0, |s| s.saturating_sub(1))),
            ("right", _) => {
                if let Some(s) = sel.filter(|&s| !self.debug.vars[s].expanded) {
                    self.toggle_var(s, cx);
                }
            }
            ("left", _) => {
                if let Some(s) = sel {
                    let r = &self.debug.vars[s];
                    if r.expanded && r.var.reference > 0 {
                        self.toggle_var(s, cx);
                    } else if let Some(p) = (0..s).rev().find(|&j| self.debug.vars[j].depth < r.depth) {
                        self.debug.sel = Some(p);
                    }
                }
            }
            ("enter", _) => {
                if let Some(s) = sel {
                    if self.debug.vars[s].var.reference > 0 {
                        self.toggle_var(s, cx);
                    } else {
                        self.begin_edit(s);
                    }
                }
            }
            ("f2", _) => {
                if let Some(s) = sel {
                    self.begin_edit(s);
                }
            }
            ("insert", _) => self.debug.edit = Some(VarEdit { target: EditTarget::NewWatch, text: String::new() }),
            ("delete", _) => {
                if let Some(RowKind::Watch(w)) = sel.map(|s| self.debug.vars[s].kind) {
                    self.remove_watch(w, cx);
                }
            }
            _ => return false,
        }
        cx.notify();
        true
    }

    /// Enter in the console's input line: evaluate in the selected frame.
    fn evaluate_input(&mut self, cx: &mut Context<Self>) {
        let expr = std::mem::take(&mut self.debug.input);
        if expr.trim().is_empty() {
            return;
        }
        self.debug.completion = None;
        self.debug.history.retain(|h| *h != expr);
        self.debug.history.push(expr.clone());
        let n = self.debug.history.len().saturating_sub(100);
        self.debug.history.drain(..n);
        self.debug.history_pos = usize::MAX;
        let _ = std::fs::create_dir_all(brisk_db::config_dir());
        let _ = std::fs::write(history_path(), serde_json::to_string(&self.debug.history).unwrap_or_default());
        self.debug.print(&format!(">>> {expr}\n"), Line::Input);
        let frame = self.debug.frames.get(self.debug.frame).map(|f| f.id);
        let Some(s) = self.debug.session.as_ref().filter(|s| s.state != State::Ended) else {
            self.debug.info("The debugger is not running");
            return;
        };
        let mut args = json!({"expression": expr, "context": "repl"});
        if let Some(f) = frame {
            args["frameId"] = json!(f);
        }
        let rx = s.client.request("evaluate", args);
        cx.spawn(async move |this, cx| {
            let res = rx.await;
            let _ = this.update(cx, |this, cx| {
                match res {
                    Ok(Ok(v)) => {
                        let (r, _) = brisk_dap::parse_evaluate(&v);
                        if !r.is_empty() {
                            this.debug.print(&format!("{r}\n"), Line::Result);
                        }
                    }
                    Ok(Err(e)) => this.debug.print(&format!("{e}\n"), Line::Err),
                    Err(_) => {}
                }
                cx.notify();
            });
        })
        .detach();
    }

    /// Tab in the console: ask debugpy what the name at the end of the input can be.
    fn complete_input(&mut self, cx: &mut Context<Self>) {
        let Some(frame) = self.debug.frames.get(self.debug.frame).map(|f| f.id) else {
            self.status = "Completion works while the program is paused".into();
            return;
        };
        let Some(s) = self.debug.session.as_ref().filter(|s| s.state != State::Ended) else { return };
        let text = self.debug.input.clone();
        let rx = s.client.request("completions", json!({"text": text, "column": text.chars().count() + 1, "frameId": frame}));
        cx.spawn(async move |this, cx| {
            let res = rx.await;
            let _ = this.update(cx, |this, cx| {
                if this.debug.input != text {
                    return;
                }
                let mut items: Vec<(String, String)> = match res {
                    Ok(Ok(v)) => v
                        .get("targets")
                        .and_then(Value::as_array)
                        .map(|a| a.iter().filter_map(|t| Some((t.get("label")?.as_str()?.to_string(), t.get("type").and_then(Value::as_str).unwrap_or("").to_string()))).collect())
                        .unwrap_or_default(),
                    _ => Vec::new(),
                };
                order_completions(&mut items);
                let start = completion_start(&text);
                match items.len() {
                    0 => this.status = "No completions".into(),
                    1 => {
                        this.debug.input.truncate(start);
                        this.debug.input.push_str(&items[0].0);
                    }
                    _ => {
                        let typed = &text[start..];
                        let p = common_prefix(items.iter().map(|i| i.0.as_str()));
                        if p.len() > typed.len() && p.starts_with(typed) {
                            this.debug.input.truncate(start);
                            this.debug.input.push_str(&p);
                        }
                        this.debug.completion = Some(ConsoleCompletion { items, start, sel: 0 });
                    }
                }
                cx.notify();
            });
        })
        .detach();
    }

    /// Keys while the completion list is open; false lets the input line have the key.
    fn completion_key(&mut self, key: &str) -> bool {
        let Some(c) = self.debug.completion.as_mut() else { return false };
        let n = c.shown(&self.debug.input).len();
        match key {
            "up" => c.sel = c.sel.checked_sub(1).unwrap_or(n.saturating_sub(1)),
            "down" => c.sel = if c.sel + 1 < n { c.sel + 1 } else { 0 },
            "pageup" => c.sel = c.sel.saturating_sub(8),
            "pagedown" => c.sel = (c.sel + 8).min(n.saturating_sub(1)),
            "escape" => self.debug.completion = None,
            "enter" | "tab" => {
                let start = c.start;
                if let Some(l) = c.shown(&self.debug.input).get(c.sel).map(|i| i.0.clone()) {
                    self.debug.input.truncate(start);
                    self.debug.input.push_str(&l);
                }
                self.debug.completion = None;
            }
            _ => return false,
        }
        true
    }

    /// After the input changed: the list follows what is typed, or closes.
    fn refilter_completion(&mut self) {
        let Some(c) = self.debug.completion.as_mut() else { return };
        let input = &self.debug.input;
        let start_ok = input.len() >= c.start && input.get(c.start..).is_some_and(|t| t.chars().all(|ch| ch.is_alphanumeric() || ch == '_'));
        let n = if start_ok { c.shown(input).len() } else { 0 };
        if n == 0 {
            self.debug.completion = None;
        } else {
            c.sel = c.sel.min(n - 1);
        }
    }

    /// Keys while the Debug window has focus: the console's input line.
    pub(super) fn debug_key(&mut self, e: &gpui::KeyDownEvent, window: &Window, cx: &mut Context<Self>) -> bool {
        if !self.debug.focus.is_focused(window) {
            return false;
        }
        let k = &e.keystroke;
        if self.debug.tab == DebugTab::Debugger {
            return self.vars_key(k, cx);
        }
        if k.modifiers.control || k.modifiers.alt || k.modifiers.platform {
            return false;
        }
        if self.completion_key(k.key.as_str()) {
            cx.notify();
            return true;
        }
        match k.key.as_str() {
            "tab" if !k.modifiers.shift => self.complete_input(cx),
            "enter" => self.evaluate_input(cx),
            "backspace" => {
                self.debug.input.pop();
            }
            "space" => self.debug.input.push(' '),
            "escape" => self.debug.input.clear(),
            "up" => {
                let pos = self.debug.history_pos.min(self.debug.history.len());
                if pos > 0 {
                    self.debug.history_pos = pos - 1;
                    self.debug.input = self.debug.history[pos - 1].clone();
                }
            }
            "down" => {
                if self.debug.history_pos.saturating_add(1) < self.debug.history.len() {
                    self.debug.history_pos += 1;
                    self.debug.input = self.debug.history[self.debug.history_pos].clone();
                } else {
                    self.debug.history_pos = self.debug.history.len();
                    self.debug.input.clear();
                }
            }
            _ => match k.key_char.as_deref() {
                Some(t) if !t.chars().any(char::is_control) => self.debug.input.push_str(t),
                _ => return false,
            },
        }
        self.refilter_completion();
        cx.notify();
        true
    }

    // --- Debug window -------------------------------------------------------------------

    pub(super) fn render_debug(&mut self, window: &Window, cx: &mut Context<Self>) -> gpui::AnyElement {
        let d = &self.debug;
        let state = d.state();
        let paused = matches!(state, Some(State::Paused(_)));
        let live = d.is_live();
        let running = state == Some(State::Running) && !d.pending;
        let can_rerun = d.last.is_some() || d.last_attach.is_some();
        let title = match (&d.session, state) {
            _ if d.pending => format!("{} — starting debugpy…", d.last.as_ref().map(|c| c.name.as_str()).unwrap_or("")),
            (Some(s), Some(State::Paused(_))) => {
                let at = d.frames.get(d.frame).map(|f| format!(" at {}:{}", f.path.as_ref().and_then(|p| p.file_name()).map(|n| n.to_string_lossy().to_string()).unwrap_or_default(), f.line + 1)).unwrap_or_default();
                format!("{} — paused{at}", s.name)
            }
            (Some(s), Some(State::Running)) => format!("{} — running", s.name),
            (Some(s), Some(State::Starting)) => format!("{} — starting debugpy…", s.name),
            (Some(s), _) => format!("{} — finished", s.name),
            (None, _) => "No debug session. Shift+F9 debugs the selected configuration, Ctrl+Shift+F9 the current file.".into(),
        };
        let tool = |id: &'static str, icon: &'static str, tip: &'static str, on: bool, color: gpui::Hsla, action: Action, cx: &mut Context<Self>| {
            div()
                .id(id)
                .w(px(26.))
                .h(px(26.))
                .flex()
                .items_center()
                .justify_center()
                .rounded(px(5.))
                .when(on, |x| x.cursor_pointer().hover(|s| s.bg(Theme::hover_row())).active(|s| s.bg(Theme::press())))
                .child(crate::icons::icon(icon, 16., if on { color } else { Theme::text_dim().opacity(0.5) }))
                .tooltip(move |_, cx| cx.new(|_| super::Tip(tip.into())).into())
                .on_click(cx.listener(move |this, _, w, cx| {
                    if on {
                        this.dispatch(action, w, cx);
                    }
                }))
        };
        let green = Theme::run_icon();
        let plain = Theme::text_muted();
        let toolbar = div()
            .flex()
            .items_center()
            .gap(px(2.))
            .px(px(6.))
            .h(px(30.))
            .border_b_1()
            .border_color(Theme::divider())
            .child(tool("dbg-rerun", "rotate-cw", "Rerun Debug Session", can_rerun, green, Action::RerunDebug, cx))
            .child(tool("dbg-resume", "play", "Resume Program  F9", paused && !d.pending, green, Action::Resume, cx))
            .child(tool("dbg-pause", "pause", "Pause Program", running, plain, Action::PauseProgram, cx))
            .child(tool("dbg-stop", "stop-solid", "Stop  Ctrl+F2", live, Theme::error(), Action::Stop, cx))
            .child(div().w(px(1.)).h(px(16.)).mx(px(4.)).bg(Theme::divider()))
            .child(tool("dbg-over", "step-over", "Step Over  F8", paused, plain, Action::StepOver, cx))
            .child(tool("dbg-into", "step-into", "Step Into  F7", paused, plain, Action::StepInto, cx))
            .child(tool("dbg-out", "step-out", "Step Out  Shift+F8", paused, plain, Action::StepOut, cx))
            .child(tool("dbg-cursor", "arrow-right-to-line", "Run to Cursor  Alt+F9", paused, plain, Action::RunToCursor, cx))
            .child(tool("dbg-eval", "calculator", "Evaluate Expression  Alt+F8", paused, plain, Action::EvaluateExpression, cx))
            .child(div().w(px(1.)).h(px(16.)).mx(px(4.)).bg(Theme::divider()))
            .child(tool("dbg-bps", "breakpoints", "View Breakpoints  Ctrl+Shift+F8", true, Theme::breakpoint(), Action::ViewBreakpoints, cx))
            .child(
                // Mute Breakpoints: a crossed-out dot, pressed while on.
                div()
                    .id("dbg-mute")
                    .w(px(26.))
                    .h(px(26.))
                    .flex()
                    .items_center()
                    .justify_center()
                    .rounded(px(5.))
                    .cursor_pointer()
                    .when(d.muted, |x| x.bg(Theme::selection_row()))
                    .when(!d.muted, |x| x.hover(|s| s.bg(Theme::hover_row())))
                    .active(|s| s.bg(Theme::press()))
                    .child(crate::icons::icon("mute-breakpoints", 16., if d.muted { Theme::text() } else { plain }))
                    .tooltip(|_, cx| cx.new(|_| super::Tip("Mute Breakpoints".into())).into())
                    .on_click(cx.listener(|this, _, w, cx| this.dispatch(Action::MuteBreakpoints, w, cx))),
            )
            .child(div().pl(px(10.)).text_size(px(12.)).text_color(Theme::text_muted()).child(title));
        let tab = |id: &'static str, label: &'static str, t: DebugTab, cur: DebugTab, cx: &mut Context<Self>| {
            div()
                .id(id)
                .px(px(10.))
                .h(px(26.))
                .flex()
                .items_center()
                .cursor_pointer()
                .text_color(if t == cur { Theme::text() } else { Theme::text_muted() })
                .when(t == cur, |x| x.border_b_2().border_color(Theme::accent()))
                .child(label)
                .on_click(cx.listener(move |this, _, w, cx| {
                    this.debug.tab = t;
                    w.focus(&this.debug.focus.clone());
                    cx.notify();
                }))
        };
        let tabs = div().flex().px(px(6.)).child(tab("dbg-tab-dbg", "Debugger", DebugTab::Debugger, d.tab, cx)).child(tab("dbg-tab-con", "Console", DebugTab::Console, d.tab, cx));
        let body: gpui::AnyElement = match d.tab {
            DebugTab::Debugger => self.render_debugger_tab(window, cx),
            DebugTab::Console => self.render_console_tab(window, cx),
        };
        div()
            .id("debug-window")
            .track_focus(&self.debug.focus)
            .size_full()
            .flex()
            .flex_col()
            .text_size(px(13.))
            .child(toolbar)
            .child(tabs)
            .child(div().flex_1().min_h_0().border_t_1().border_color(Theme::divider()).child(body))
            .on_mouse_down(gpui::MouseButton::Left, cx.listener(|this, _, w, cx| {
                w.focus(&this.debug.focus.clone());
                cx.notify();
            }))
            .into_any_element()
    }

    fn render_debugger_tab(&mut self, window: &Window, cx: &mut Context<Self>) -> gpui::AnyElement {
        let threads = self.render_threads(cx);
        let d = &self.debug;
        let frames = div()
            .id("dbg-frames")
            .w(px(300.))
            .h_full()
            .flex_shrink_0()
            .overflow_y_scroll()
            .border_r_1()
            .border_color(Theme::divider())
            .children(threads)
            .when(d.frames.is_empty(), |x| x.child(div().p(px(8.)).text_color(Theme::text_dim()).child(if d.paused_thread().is_some() { "Loading…" } else { "Frames appear when the program is paused" })))
            .children(d.frames.iter().enumerate().map(|(i, f)| {
                let file = f.path.as_ref().and_then(|p| p.file_name()).map(|n| n.to_string_lossy().to_string()).unwrap_or_default();
                let mine = f.path.as_ref().is_some_and(|p| p.starts_with(&self.root));
                div()
                    .id(("dbg-frame", i))
                    .h(px(22.))
                    .px(px(8.))
                    .flex()
                    .items_center()
                    .gap(px(6.))
                    .cursor_pointer()
                    .when(i == d.frame, |x| x.bg(Theme::selection_row()))
                    .when(i != d.frame, |x| x.hover(|s| s.bg(Theme::hover_row())))
                    .text_color(if mine { Theme::text() } else { Theme::text_muted() })
                    .child(f.name.clone())
                    .child(div().text_color(Theme::text_dim()).text_size(px(12.)).child(format!("{file}:{}", f.line + 1)))
                    .on_click(cx.listener(move |this, _, w, cx| {
                        this.select_frame(i, w, cx);
                        cx.notify();
                    }))
            }));
        let paused = d.paused_thread().is_some();
        let focused = d.focus.is_focused(window);
        let caret = || div().w(px(1.5)).h(px(14.)).bg(Theme::caret());
        let input = |text: &str, focused: bool| div().flex().items_center().px(px(3.)).border_1().border_color(Theme::accent()).rounded(px(3.)).child(text.to_string()).when(focused, |x| x.child(caret()));
        let edit = d.edit.clone();
        let mut rows: Vec<gpui::AnyElement> = Vec::new();
        let new_watch_at = d.watch_len();
        let new_watch_row = |text: &str| {
            div()
                .id("dbg-new-watch")
                .h(px(21.))
                .pl(px(8. + 16.))
                .flex()
                .items_center()
                .gap(px(4.))
                .child(div().w(px(12.)).text_color(Theme::text_dim()).child("+"))
                .child(input(text, focused))
                .into_any_element()
        };
        for (i, r) in d.vars.iter().enumerate() {
            if i == new_watch_at {
                if let Some(e) = edit.as_ref().filter(|e| e.target == EditTarget::NewWatch) {
                    rows.push(new_watch_row(&e.text));
                }
            }
            let arrow = crate::icons::tree_arrow(r.var.reference > 0, r.expanded, Theme::text_dim());
            let header = matches!(r.kind, RowKind::Scope | RowKind::WatchHeader);
            let watch = matches!(r.kind, RowKind::Watch(_));
            let editing_name = matches!((r.kind, edit.as_ref().map(|e| e.target)), (RowKind::Watch(w), Some(EditTarget::Watch(x))) if w == x);
            let editing_value = edit.as_ref().is_some_and(|e| e.target == EditTarget::Value(i));
            let error = r.var.value.starts_with("<error: ") && watch;
            let returned = r.return_label();
            let name_color = if header {
                Theme::text_muted()
            } else if watch {
                Theme::accent()
            } else if returned.is_some() {
                Theme::syntax(brisk_syntax::Highlight::Function)
            } else {
                Theme::syntax(brisk_syntax::Highlight::Property)
            };
            let value: gpui::AnyElement = if editing_value {
                input(&edit.as_ref().map(|e| e.text.clone()).unwrap_or_default(), focused).into_any_element()
            } else if watch && r.var.value.is_empty() && !paused {
                div().text_color(Theme::text_dim()).child("not available: the program is not paused").into_any_element()
            } else {
                div().text_color(if error { Theme::error() } else { Theme::text() }).child(r.var.value.chars().take(300).collect::<String>()).into_any_element()
            };
            rows.push(
                div()
                    .id(("dbg-var", i))
                    .h(px(21.))
                    .pl(px(8. + 16. * r.depth as f32))
                    .pr(px(8.))
                    .flex()
                    .items_center()
                    .gap(px(4.))
                    .whitespace_nowrap()
                    .overflow_hidden()
                    .when(r.var.reference > 0, |x| x.cursor_pointer())
                    .when(d.sel == Some(i), |x| x.bg(if focused { Theme::selection_row() } else { Theme::hover_row() }))
                    .when(d.sel != Some(i), |x| x.hover(|s| s.bg(Theme::hover_row())))
                    .child(arrow)
                    .child(if editing_name {
                        input(&edit.as_ref().map(|e| e.text.clone()).unwrap_or_default(), focused).into_any_element()
                    } else {
                        div().text_color(name_color).child(returned.clone().unwrap_or_else(|| r.var.name.clone())).into_any_element()
                    })
                    .when(!header, |x| {
                        x.child(div().text_color(Theme::text_dim()).child("="))
                            .when(!r.var.type_name.is_empty(), |x| x.child(div().text_color(Theme::text_dim()).child(format!("{{{}}}", r.var.type_name))))
                            .child(value)
                    })
                    .on_click(cx.listener(move |this, ev: &gpui::ClickEvent, _, cx| {
                        if this.debug.edit.as_ref().is_some_and(|e| e.target != EditTarget::Value(i)) {
                            this.commit_edit(cx);
                        }
                        if ev.click_count() >= 2 {
                            this.begin_edit(i);
                        } else {
                            this.toggle_var(i, cx);
                        }
                        cx.notify();
                    }))
                    .into_any_element(),
            );
        }
        if new_watch_at >= d.vars.len() {
            if let Some(e) = edit.as_ref().filter(|e| e.target == EditTarget::NewWatch) {
                rows.push(new_watch_row(&e.text));
            }
        }
        if rows.is_empty() {
            rows.push(div().p(px(8.)).text_color(Theme::text_dim()).child(if paused { "Loading…" } else { "Variables appear when the program is paused. Insert adds a watch." }).into_any_element());
        }
        let sel = d.sel;
        let sel_kind = sel.and_then(|s| d.vars.get(s)).map(|r| (r.kind, r.parent));
        let can_remove = matches!(sel_kind, Some((RowKind::Watch(_), _)));
        let can_set = matches!(sel_kind, Some((RowKind::Var, p)) if p > 0) && paused;
        let can_copy = matches!(sel_kind, Some((RowKind::Var | RowKind::Watch(_), _)));
        let mini = |id: &'static str, label: &'static str, tip: &'static str, on: bool, f: fn(&mut Workspace, &mut Context<Workspace>), cx: &mut Context<Self>| {
            div()
                .id(id)
                .px(px(6.))
                .h(px(20.))
                .flex()
                .items_center()
                .rounded(px(4.))
                .text_size(px(12.))
                .text_color(if on { Theme::text() } else { Theme::text_dim() })
                .when(on, |x| x.cursor_pointer().hover(|s| s.bg(Theme::hover_row())).active(|s| s.bg(Theme::press())))
                .child(label)
                .tooltip(move |_, cx| cx.new(|_| super::Tip(tip.into())).into())
                .on_click(cx.listener(move |this, _, w, cx| {
                    if on {
                        f(this, cx);
                        w.focus(&this.debug.focus.clone());
                        cx.notify();
                    }
                }))
        };
        let bar = div()
            .flex()
            .items_center()
            .gap(px(2.))
            .px(px(4.))
            .h(px(24.))
            .border_b_1()
            .border_color(Theme::divider())
            .child(mini("dbg-watch-add", "+ Watch", "New Watch  Insert", true, |t, _| t.debug.edit = Some(VarEdit { target: EditTarget::NewWatch, text: String::new() }), cx))
            .child(mini("dbg-watch-remove", "− Watch", "Remove Watch  Delete", can_remove, |t, cx| {
                if let Some(RowKind::Watch(w)) = t.debug.sel.and_then(|s| t.debug.vars.get(s)).map(|r| r.kind) {
                    t.remove_watch(w, cx);
                }
            }, cx))
            .child(mini("dbg-set-value", "Set Value", "Set Value  F2", can_set, |t, _| {
                if let Some(s) = t.debug.sel {
                    t.begin_edit(s);
                }
            }, cx))
            .child(mini("dbg-copy-value", "Copy Value", "Copy Value  Ctrl+C", can_copy, |t, cx| {
                if let Some(s) = t.debug.sel {
                    t.copy_value(s, cx);
                }
            }, cx));
        let vars = div()
            .id("dbg-vars")
            .flex_1()
            .h_full()
            .flex()
            .flex_col()
            .child(bar)
            .child(div().id("dbg-vars-list").flex_1().min_h_0().overflow_y_scroll().font_family(mono_font()).text_size(px(12.5)).children(rows));
        let banner = d.exception.clone().map(|e| {
            div()
                .flex_shrink_0()
                .px(px(8.))
                .py(px(3.))
                .flex()
                .items_center()
                .gap(px(6.))
                .bg(Theme::warning_bg())
                .border_b_1()
                .border_color(Theme::divider())
                .text_color(Theme::error())
                .child(div().font_weight(gpui::FontWeight::BOLD).child("Exception"))
                .child(div().whitespace_nowrap().overflow_hidden().child(e))
        });
        div().size_full().flex().flex_col().children(banner).child(div().flex_1().min_h_0().flex().child(frames).child(vars)).into_any_element()
    }

    fn render_console_tab(&mut self, window: &Window, cx: &mut Context<Self>) -> gpui::AnyElement {
        let d = &self.debug;
        let focused = d.focus.is_focused(window);
        let can_eval = d.paused_thread().is_some();
        let lines = div()
            .id("dbg-console")
            .flex_1()
            .min_h_0()
            .overflow_y_scroll()
            .track_scroll(&d.console_scroll)
            .px(px(8.))
            .py(px(4.))
            .font_family(mono_font())
            .text_size(px(12.5))
            .children(d.console.iter().enumerate().map(|(ln, (t, k))| {
                let color = match k {
                    Line::Out => Theme::text(),
                    Line::Err => Theme::error(),
                    Line::Info => Theme::text_muted(),
                    Line::Input => Theme::accent(),
                    Line::Result => Theme::syntax(brisk_syntax::Highlight::String),
                };
                let line = t.trim_end_matches('\n');
                let chars: Vec<char> = line.chars().collect();
                let mut pieces: Vec<gpui::AnyElement> = Vec::new();
                let mut at = 0;
                if matches!(k, Line::Out | Line::Err) {
                    for (j, l) in brisk_run::find_links(line).into_iter().enumerate() {
                        let Some((path, n)) = self.console_link(&l.path, l.line) else { continue };
                        if l.cols.start > at {
                            pieces.push(div().child(chars[at..l.cols.start].iter().collect::<String>()).into_any_element());
                        }
                        pieces.push(
                            div()
                                .id(("dbg-link", ln * 64 + j))
                                .text_color(Theme::accent())
                                .underline()
                                .cursor_pointer()
                                .child(chars[l.cols.clone()].iter().collect::<String>())
                                .on_click(cx.listener(move |this, _, w, cx| this.open_console_link(&path, n, w, cx)))
                                .into_any_element(),
                        );
                        at = l.cols.end;
                    }
                }
                if at < chars.len() || pieces.is_empty() {
                    pieces.push(div().child(chars[at.min(chars.len())..].iter().collect::<String>()).into_any_element());
                }
                div().min_h(px(17.)).flex().text_color(color).whitespace_nowrap().children(pieces)
            }));
        let input = div()
            .h(px(26.))
            .px(px(8.))
            .flex()
            .items_center()
            .gap(px(6.))
            .border_t_1()
            .border_color(Theme::divider())
            .font_family(mono_font())
            .text_size(px(12.5))
            .child(div().text_color(Theme::accent()).child(">>>"))
            .child(if d.input.is_empty() && !focused {
                div().text_color(Theme::text_dim()).child(if can_eval { "Evaluate an expression in the selected frame (Alt+F8)" } else { "Expressions can be evaluated while the program is paused" }).into_any_element()
            } else {
                div().flex().items_center().child(d.input.clone()).when(focused, |x| x.child(div().w(px(1.5)).h(px(15.)).bg(Theme::caret()))).into_any_element()
            });
        let popup = d.completion.as_ref().map(|c| {
            let shown = c.shown(&d.input);
            let first = c.sel.saturating_sub(7);
            let left = 8. + 30. + 7.5 * d.input[..c.start.min(d.input.len())].chars().count() as f32;
            div()
                .absolute()
                .bottom(px(28.))
                .left(px(left))
                .min_w(px(220.))
                .max_w(px(480.))
                .py(px(2.))
                .bg(Theme::panel())
                .border_1()
                .border_color(Theme::divider())
                .rounded(px(4.))
                .shadow_md()
                .font_family(mono_font())
                .text_size(px(12.5))
                .occlude()
                .children(shown.into_iter().enumerate().skip(first).take(8).map(|(j, (label, kind))| {
                    div()
                        .h(px(20.))
                        .px(px(8.))
                        .flex()
                        .items_center()
                        .gap(px(12.))
                        .when(j == c.sel, |x| x.bg(Theme::selection_row()))
                        .child(div().flex_1().whitespace_nowrap().child(label.clone()))
                        .child(div().text_color(Theme::text_dim()).text_size(px(11.)).child(kind.clone()))
                }))
        });
        div().size_full().relative().flex().flex_col().child(lines).child(input).children(popup).into_any_element()
    }

    // --- Smoke --------------------------------------------------------------------------

    /// BRISK_SMOKE_DEBUG=<file.py>: breakpoint, debug, frames, variables, evaluate, steps,
    /// run to cursor, conditional breakpoint, finish.
    pub(super) fn debug_smoke(&mut self, file: PathBuf, window: &mut Window, cx: &mut Context<Self>) {
        self.open_file(&file, window, cx);
        let Some(ed) = self.active_editor().cloned() else { return };
        let line_of = |needle: &str, cx: &mut Context<Self>| ed.read(cx).buffer.text().lines().position(|l| l.contains(needle)).unwrap_or(0);
        let bp_line = line_of("total = add(", cx);
        ed.update(cx, |e, cx| {
            e.set_breakpoints(&[]);
            e.go_to_line(bp_line, cx);
        });
        self.dispatch(Action::ToggleBreakpoint, window, cx);
        eprintln!("smoke-debug: ctrl-f8 on line {} -> {:?}", bp_line + 1, ed.read(cx).breakpoint_list().iter().map(|(l, _)| l + 1).collect::<Vec<_>>());
        let ret_line = line_of("return a + b", cx);
        let loop_line = line_of("acc +=", cx);
        let print_line = line_of("print(\"total\"", cx);
        let dir = file.parent().map(Path::to_path_buf).unwrap_or_default();
        cx.spawn_in(window, async move |this, cx| {
            let bg = cx.background_executor().clone();
            let wait = |ms| bg.timer(std::time::Duration::from_millis(ms));
            let state = |this: &gpui::WeakEntity<Workspace>, cx: &mut gpui::AsyncWindowContext| this.update(cx, |t, _| t.debug.state()).ok().flatten();
            // Wait until paused / ended.
            macro_rules! until {
                ($pred:expr) => {{
                    let mut ok = false;
                    for _ in 0..300 {
                        wait(100).await;
                        let s = state(&this, cx);
                        if $pred(s) {
                            ok = true;
                            break;
                        }
                    }
                    ok
                }};
            }
            let paused = |s: Option<State>| matches!(s, Some(State::Paused(_)));
            // Interpreters are discovered in the background at start.
            for _ in 0..100 {
                if this.update(cx, |t, _| t.runs.all.is_some()).unwrap_or(true) {
                    break;
                }
                wait(100).await;
            }
            let _ = this.update_in(cx, |t, w, cx| t.dispatch(Action::DebugContext, w, cx));
            let ok = until!(paused);
            wait(400).await;
            let _ = this.update_in(cx, |t, _, cx| {
                let f: Vec<String> = t.debug.frames.iter().map(|f| format!("{}:{}", f.name, f.line + 1)).collect();
                let v: Vec<String> = t.debug.vars.iter().map(|r| format!("{}{}={}", "  ".repeat(r.depth), r.var.name, r.var.value)).collect();
                let exec = t.active_editor().and_then(|e| e.read(cx).exec_line).map(|l| l + 1);
                eprintln!("smoke-debug: paused={ok} frames={f:?} exec_line={exec:?}");
                eprintln!("smoke-debug: variables {v:?}");
                let inl: Vec<String> = t.active_editor().map(|e| (0..12).filter_map(|l| e.read(cx).inline_value(l).map(|v| format!("{}: {v}", l + 1))).collect()).unwrap_or_default();
                eprintln!("smoke-debug: inline values {inl:?}");
            });
            if std::env::var_os("BRISK_SMOKE_DEBUG_HOLD").is_some() {
                // Screenshots: stay paused at the first breakpoint.
                let _ = this.update_in(cx, |t, _, cx| {
                    if let Some(i) = t.debug.vars.iter().position(|r| r.var.name == "items") {
                        t.toggle_var(i, cx);
                    }
                    t.add_watch("len(items)".into(), cx);
                });
                // Print what changes, for a driver that clicks the window (mouse checks).
                let mut last = String::new();
                loop {
                    wait(250).await;
                    let Ok(now) = this.update(cx, |t, cx| {
                        let bps: Vec<String> = t
                            .active_editor()
                            .map(|e| e.read(cx).breakpoint_list().into_iter().map(|(l, b)| format!("{}{}", l + 1, b.condition.map(|c| format!(" if {c}")).unwrap_or_default())).collect())
                            .unwrap_or_default();
                        format!(
                            "state={:?} top={:?} bps={bps:?} muted={} popup={} bpdlg={} tab={:?} sel={:?} edit={:?} watches={:?}",
                            t.debug.state(),
                            t.debug.frames.first().map(|f| f.line + 1),
                            t.debug.muted,
                            t.popup.is_some(),
                            t.debug.bp_dialog.as_ref().map_or("closed".to_string(), |d| format!("open(sel {} editing {})", d.sel, d.editing)),
                            t.debug.tab,
                            t.debug.sel.and_then(|s| t.debug.vars.get(s)).map(|r| r.var.name.clone()),
                            t.debug.edit.as_ref().map(|e| e.target),
                            t.debug.watches,
                        )
                    }) else {
                        return;
                    };
                    if now != last {
                        eprintln!("smoke-hold: {now}");
                        last = now;
                    }
                }
            }
            // Expand a list variable.
            let _ = this.update_in(cx, |t, _, cx| {
                if let Some(i) = t.debug.vars.iter().position(|r| r.var.name == "items") {
                    t.toggle_var(i, cx);
                }
            });
            wait(600).await;
            let _ = this.update_in(cx, |t, _, _| {
                let v: Vec<String> = t.debug.vars.iter().filter(|r| r.depth >= 2).map(|r| format!("{}={}", r.var.name, r.var.value)).collect();
                eprintln!("smoke-debug: expanded items -> {v:?}");
            });
            // Set Value of items[0], Copy Value of items.
            let _ = this.update_in(cx, |t, _, cx| {
                if let Some(i) = t.debug.vars.iter().position(|r| r.depth >= 2 && r.var.name == "0") {
                    t.set_value(i, "10".into(), cx);
                }
            });
            wait(800).await;
            let _ = this.update_in(cx, |t, _, cx| {
                let v = t.debug.vars.iter().find(|r| r.depth >= 2 && r.var.name == "0").map(|r| r.var.value.clone());
                if let Some(i) = t.debug.vars.iter().position(|r| r.var.name == "items") {
                    t.copy_value(i, cx);
                }
                let clip = cx.read_from_clipboard().and_then(|c| c.text());
                eprintln!("smoke-debug: set value items[0] -> {v:?}; copy value of items -> {clip:?}");
                t.debug.watches.clear();
                for w in ["len(items)", "total", "nope_x"] {
                    t.add_watch(w.into(), cx);
                }
            });
            wait(1000).await;
            let watch_rows = |t: &Workspace| -> Vec<String> {
                t.debug.vars.iter().filter(|r| matches!(r.kind, RowKind::Watch(_))).map(|r| format!("{}={}", r.var.name, r.var.value)).collect()
            };
            let _ = this.update_in(cx, |t, _, cx| {
                eprintln!("smoke-debug: watches -> {:?}", watch_rows(t));
                // Edit the first watch (as F2 + typing + Enter), remove the failing one (Delete).
                if let Some(i) = t.debug.vars.iter().position(|r| r.kind == RowKind::Watch(0)) {
                    t.begin_edit(i);
                    if let Some(e) = t.debug.edit.as_mut() {
                        e.text = "len(items) + 1".into();
                    }
                    t.commit_edit(cx);
                }
                t.remove_watch(2, cx);
            });
            wait(1000).await;
            let _ = this.update_in(cx, |t, _, _| {
                let saved = std::fs::read_to_string(watches_path()).unwrap_or_default();
                eprintln!("smoke-debug: edited watches -> {:?}; saved={}", watch_rows(t), saved.contains("len(items) + 1") && !saved.contains("nope_x"));
            });
            // Evaluate in the console.
            let _ = this.update_in(cx, |t, w, cx| {
                t.dispatch(Action::EvaluateExpression, w, cx);
                t.debug.input = "sum(items) * 10".into();
                t.evaluate_input(cx);
            });
            wait(800).await;
            let _ = this.update_in(cx, |t, _, cx| {
                let tail: Vec<String> = t.debug.console.iter().rev().take(2).map(|(l, _)| l.trim_end().to_string()).collect();
                eprintln!("smoke-debug: evaluate -> {tail:?}");
                // Tab completion: one match is inserted, several open the list.
                t.debug.input = "ite".into();
                t.complete_input(cx);
            });
            wait(800).await;
            let _ = this.update(cx, |t, cx| {
                eprintln!("smoke-debug: tab after 'ite' -> {:?} list {:?}", t.debug.input, t.debug.completion.as_ref().map(|c| c.shown(&t.debug.input).iter().map(|i| i.0.clone()).collect::<Vec<_>>()));
                t.debug.completion = None;
                t.debug.input = "items.".into();
                t.complete_input(cx);
            });
            wait(800).await;
            let _ = this.update(cx, |t, cx| {
                let shown: Vec<String> = t.debug.completion.as_ref().map(|c| c.shown(&t.debug.input).iter().take(4).map(|i| i.0.clone()).collect()).unwrap_or_default();
                eprintln!("smoke-debug: tab after 'items.' -> list {shown:?}");
                for ch in ["c", "o"] {
                    t.debug.input.push_str(ch);
                    t.refilter_completion();
                }
                let shown: Vec<String> = t.debug.completion.as_ref().map(|c| c.shown(&t.debug.input).iter().map(|i| i.0.clone()).collect()).unwrap_or_default();
                t.completion_key("down");
                t.completion_key("enter");
                eprintln!("smoke-debug: typed 'co' -> {shown:?}, down+enter -> {:?}; history file has {}", t.debug.input, load_history().iter().any(|h| h == "sum(items) * 10"));
                t.debug.input.clear();
                cx.notify();
            });
            // F7 into add(), Shift+F8 out, F8 over.
            let mut seen = Vec::new();
            for a in [Action::StepInto, Action::StepOut, Action::StepOver] {
                let _ = this.update_in(cx, |t, w, cx| t.dispatch(a, w, cx));
                until!(paused);
                wait(500).await;
                if let Ok(s) = this.update(cx, |t, _| t.debug.frames.first().map(|f| format!("{}:{}", f.name, f.line + 1))) {
                    seen.push(s.unwrap_or_default());
                }
                if a == Action::StepOut {
                    let _ = this.update(cx, |t, _| {
                        let r: Vec<String> = t.debug.vars.iter().filter_map(|r| r.return_label().map(|l| format!("{l} = {}", r.var.value))).collect();
                        let top = t.debug.vars.iter().find(|r| r.depth == 1).map(|r| r.var.name.clone());
                        eprintln!("smoke-debug: return values after shift-f8 -> {r:?} first local {top:?}");
                    });
                }
            }
            eprintln!("smoke-debug: f7, shift-f8, f8 -> {seen:?} (return line {})", ret_line + 1);
            wait(500).await;
            let _ = this.update_in(cx, |t, _, _| eprintln!("smoke-debug: watches after f8 -> {:?}", watch_rows(t)));
            // Conditional breakpoint inside the loop, then F9.
            let _ = this.update_in(cx, |t, _, cx| {
                if let Some(ed) = t.active_editor().cloned() {
                    ed.update(cx, |e, cx| e.set_breakpoint_condition(loop_line, Some("i == 3".into()), cx));
                }
            });
            wait(300).await;
            let _ = this.update_in(cx, |t, w, cx| t.dispatch(Action::Resume, w, cx));
            until!(paused);
            wait(500).await;
            let _ = this.update_in(cx, |t, _, cx| {
                let i = t.debug.vars.iter().find(|r| r.var.name == "i").map(|r| r.var.value.clone());
                let at = t.debug.frames.first().map(|f| f.line + 1);
                let marks: Vec<(usize, Option<String>, Option<bool>)> =
                    t.active_editor().map(|e| e.read(cx).breakpoint_list().into_iter().map(|(l, b)| (l + 1, b.condition, b.verified)).collect()).unwrap_or_default();
                eprintln!("smoke-debug: conditional bp -> line {at:?} i={i:?} breakpoints {marks:?}");
            });
            // Run to cursor at the print line (the loop breakpoint removed first).
            let _ = this.update_in(cx, |t, w, cx| {
                if let Some(ed) = t.active_editor().cloned() {
                    ed.update(cx, |e, cx| {
                        e.toggle_breakpoint(loop_line, cx);
                        e.go_to_line(print_line, cx);
                    });
                }
                t.dispatch(Action::RunToCursor, w, cx);
            });
            until!(paused);
            wait(500).await;
            let _ = this.update_in(cx, |t, _, _| eprintln!("smoke-debug: run to cursor -> {:?}", t.debug.frames.first().map(|f| f.line + 1)));
            let _ = this.update_in(cx, |t, w, cx| t.dispatch(Action::Resume, w, cx));
            let ended = until!(|s: Option<State>| s == Some(State::Ended));
            wait(300).await;
            let _ = this.update_in(cx, |t, _, cx| {
                let out: Vec<String> = t.debug.console.iter().filter(|(_, k)| matches!(k, Line::Out | Line::Info)).map(|(l, _)| l.trim_end().to_string()).collect();
                let exec = t.active_editor().and_then(|e| e.read(cx).exec_line);
                let inl = t.active_editor().map(|e| (0..12).any(|l| e.read(cx).inline_value(l).is_some()));
                eprintln!("smoke-debug: inline values left after the end -> {inl:?}");
                eprintln!("smoke-debug: finished={ended} exec_line={exec:?} console tail {:?}", out.iter().rev().take(3).collect::<Vec<_>>());
                eprintln!("smoke-debug: stored -> {}", std::fs::read_to_string(store_path()).map(|s| s.contains("dbg_main.py")).unwrap_or(false));
            });
            // Pause a busy program, rerun it, stop it while it runs.
            let spin = dir.join("dbg_spin.py");
            let _ = std::fs::write(&spin, "import time\n\nn = 0\nwhile True:\n    n += 1\n    time.sleep(0.01)\n");
            let _ = this.update_in(cx, |t, w, cx| {
                t.open_file(&spin, w, cx);
                t.dispatch(Action::DebugContext, w, cx);
            });
            let running = until!(|s: Option<State>| s == Some(State::Running));
            wait(1500).await;
            let _ = this.update_in(cx, |t, w, cx| t.dispatch(Action::PauseProgram, w, cx));
            let p = until!(paused);
            wait(500).await;
            let _ = this.update_in(cx, |t, _, _| {
                let n = t.debug.vars.iter().find(|r| r.var.name == "n").map(|r| r.var.value.clone());
                eprintln!("smoke-debug: pause running={running} paused={p} at {:?} n={n:?}", t.debug.frames.first().map(|f| format!("{}:{}", f.name, f.line + 1)));
            });
            let _ = this.update_in(cx, |t, w, cx| t.dispatch(Action::RerunDebug, w, cx));
            let rerun = until!(|s: Option<State>| s == Some(State::Running));
            wait(800).await;
            let _ = this.update_in(cx, |t, w, cx| {
                eprintln!("smoke-debug: rerun -> running={rerun} name={:?}", t.debug.session.as_ref().map(|s| s.name.clone()));
                t.dispatch(Action::Stop, w, cx);
            });
            let stopped = until!(|s: Option<State>| s == Some(State::Ended));
            // Stop while debugpy is still starting: no session may appear afterwards.
            let _ = this.update_in(cx, |t, w, cx| {
                t.dispatch(Action::RerunDebug, w, cx);
                let pending = t.debug.pending;
                t.dispatch(Action::Stop, w, cx);
                eprintln!("smoke-debug: stop -> {stopped}; stop while starting: pending={pending} after={}", t.debug.pending);
            });
            wait(3000).await;
            let _ = this.update_in(cx, |t, _, _| eprintln!("smoke-debug: late start dropped -> state={:?}", t.debug.state()));
            // An uncaught exception: red header, traceback links in the console.
            let exc = dir.join("dbg_exc.py");
            let _ = std::fs::write(&exc, "def f(x):\n    return 1 / x\n\n\nf(0)\n");
            let _ = this.update_in(cx, |t, w, cx| {
                t.open_file(&exc, w, cx);
                t.dispatch(Action::DebugContext, w, cx);
            });
            let p = until!(paused);
            wait(1000).await;
            let _ = this.update_in(cx, |t, w, cx| {
                let links: Vec<(PathBuf, usize)> = t
                    .debug
                    .console
                    .iter()
                    .filter(|(_, k)| matches!(k, Line::Err | Line::Out))
                    .flat_map(|(l, _)| brisk_run::find_links(l))
                    .filter_map(|l| t.console_link(&l.path, l.line))
                    .collect();
                eprintln!("smoke-debug: exception paused={p} header={:?} frame={:?} links={:?}", t.debug.exception, t.debug.frames.first().map(|f| format!("{}:{}", f.name, f.line + 1)), links.iter().map(|(p, l)| format!("{}:{l}", p.file_name().unwrap_or_default().to_string_lossy())).collect::<Vec<_>>());
                let err: Vec<&str> = t.debug.console.iter().filter(|(_, k)| *k == Line::Err).map(|(l, _)| l.trim_end()).collect();
                eprintln!("smoke-debug: exception console {err:?}");
                // Click on the first link: as the console's on_click does.
                if let Some((p, l)) = links.first().cloned() {
                    t.open_file(&spin, w, cx);
                    t.open_console_link(&p, l, w, cx);
                    let at = t.active_editor().map(|e| (e.read(cx).path.clone().and_then(|p| p.file_name().map(|n| n.to_string_lossy().to_string())), e.read(cx).caret_line_col().0 + 1));
                    eprintln!("smoke-debug: link opens -> {at:?}");
                }
                t.dispatch(Action::Stop, w, cx);
            });
            until!(|s: Option<State>| s == Some(State::Ended));
            // A new session reached a stop or its end (the previous one is Ended too).
            macro_rules! settled {
                () => {{
                    let mut st = None;
                    for _ in 0..300 {
                        wait(100).await;
                        let (pending, s) = this.update(cx, |t, _| (t.debug.pending, t.debug.state())).unwrap_or((false, None));
                        if !pending && matches!(s, Some(State::Paused(_) | State::Ended)) {
                            st = s;
                            break;
                        }
                    }
                    st
                }};
            }
            // View Breakpoints: a disabled breakpoint is skipped; "on raised" stops on a handled exception.
            let raise = dir.join("dbg_raise.py");
            let _ = std::fs::write(&raise, "def f():\n    try:\n        raise ValueError(\"handled\")\n    except ValueError:\n        pass\n    x = 1\n    return x\n\n\nf()\n");
            let _ = this.update_in(cx, |t, w, cx| {
                eprintln!("smoke-debug: after stop exception={:?}", t.debug.exception);
                t.open_file(&raise, w, cx);
                if let Some(ed) = t.active_editor().cloned() {
                    ed.update(cx, |e, cx| e.toggle_breakpoint(5, cx));
                }
                t.dispatch(Action::ViewBreakpoints, w, cx);
                let rows: Vec<String> = t
                    .bp_rows(cx)
                    .iter()
                    .map(|r| match r {
                        BpRow::Raised => format!("raised={}", t.debug.exc_raised),
                        BpRow::Uncaught => format!("uncaught={}", t.debug.exc_uncaught),
                        BpRow::Class(_, c) => format!("class {}", c.name),
                        BpRow::Line(p, l, b) => format!("{}:{} enabled={}", p.file_name().unwrap_or_default().to_string_lossy(), l + 1, b.enabled),
                    })
                    .collect();
                eprintln!("smoke-debug: view breakpoints open={} rows {rows:?}", t.debug.bp_dialog.is_some());
                t.bp_set_enabled(&raise, 5, false, cx);
                t.set_exception_breakpoints(false, true, cx);
                let sent = t.source_breakpoints(&raise, cx).len();
                let shown = t.active_editor().map(|e| e.read(cx).breakpoint_list().into_iter().map(|(l, b)| (l + 1, b.enabled)).collect::<Vec<_>>());
                eprintln!("smoke-debug: disabled -> editor {shown:?}, sent to debugpy {sent}");
                t.debug.bp_dialog = None;
                t.dispatch(Action::DebugContext, w, cx);
            });
            let s1 = settled!();
            let _ = this.update_in(cx, |t, w, cx| {
                eprintln!("smoke-debug: disabled bp + raised off -> {s1:?}");
                t.set_exception_breakpoints(true, true, cx);
                t.dispatch(Action::DebugContext, w, cx);
            });
            let s2 = settled!();
            wait(800).await;
            let _ = this.update_in(cx, |t, w, cx| {
                let saved = std::fs::read_to_string(exc_path()).unwrap_or_default().replace(char::is_whitespace, "");
                eprintln!(
                    "smoke-debug: raised on -> {s2:?} at {:?} header={:?} saved={}",
                    t.debug.frames.first().map(|f| format!("{}:{}", f.name, f.line + 1)),
                    t.debug.exception,
                    saved.contains("\"raised\":true")
                );
                t.dispatch(Action::Stop, w, cx);
                t.set_exception_breakpoints(false, true, cx);
                t.bp_set_condition(&raise, 5, Some("x > 0".into()), cx);
                let c = t.bp_rows(cx).into_iter().find_map(|r| match r {
                    BpRow::Line(_, 5, b) => Some((b.condition, b.enabled)),
                    _ => None,
                });
                t.bp_remove(&raise, 5, cx);
                eprintln!("smoke-debug: condition {c:?}; after remove rows={}", t.bp_rows(cx).len());
            });
            until!(|s: Option<State>| s == Some(State::Ended));
            // pytest: one test (caret inside it), then the whole file; a breakpoint in the test.
            let test = dir.join("test_dbg_calc.py");
            let _ = std::fs::write(&test, "def double(x):\n    return x * 2\n\n\ndef test_double():\n    value = double(21)\n    assert value == 42\n\n\ndef test_other():\n    assert double(1) == 2\n");
            for (caret, what) in [(5usize, "one test"), (0, "file")] {
                let _ = this.update_in(cx, |t, w, cx| {
                    t.open_file(&test, w, cx);
                    if let Some(ed) = t.active_editor().cloned() {
                        ed.update(cx, |e, cx| {
                            if !e.breakpoint_list().iter().any(|(l, _)| *l == 6) {
                                e.toggle_breakpoint(6, cx);
                            }
                            e.go_to_line(caret, cx);
                        });
                    }
                    t.dispatch(Action::DebugContext, w, cx);
                });
                let s = settled!();
                wait(800).await;
                let _ = this.update_in(cx, |t, w, cx| {
                    let cmd = t.debug.console.first().map(|(l, _)| l.trim_end().rsplit(' ').take(2).collect::<Vec<_>>().join(" ")).unwrap_or_default();
                    let value = t.debug.vars.iter().find(|r| r.var.name == "value").map(|r| r.var.value.clone());
                    eprintln!("smoke-debug: pytest {what}: {s:?} at {:?} value={value:?} cmd tail {cmd:?}", t.debug.frames.first().map(|f| format!("{}:{}", f.name, f.line + 1)));
                    t.dispatch(Action::Resume, w, cx);
                });
                until!(|s: Option<State>| s == Some(State::Ended));
                wait(300).await;
                let _ = this.update_in(cx, |t, _, _| {
                    let summary = t.debug.console.iter().rev().map(|(l, _)| l.trim()).find(|l| l.contains("passed") || l.contains("failed"));
                    eprintln!("smoke-debug: pytest {what} result {summary:?}");
                });
            }
            let thr = dir.join("dbg_threads.py");
            let _ = std::fs::write(&thr, "import threading\nev = threading.Event()\ndef work(n):\n    x = n * 2\n    ev.set()\n    return x\ndef main():\n    t = threading.Thread(target=work, args=(5,), name=\"Worker\")\n    t.start()\n    t.join()\nmain()\n");
            let _ = this.update_in(cx, |t, w, cx| {
                t.open_file(&thr, w, cx);
                if let Some(ed) = t.active_editor().cloned() {
                    ed.update(cx, |e, cx| e.toggle_breakpoint(4, cx));
                }
                t.dispatch(Action::DebugContext, w, cx);
            });
            let s = settled!();
            let mut threads = Vec::new();
            for _ in 0..50 {
                wait(100).await;
                threads = this.update(cx, |t, _| t.debug.threads.clone()).unwrap_or_default();
                if !threads.is_empty() {
                    break;
                }
            }
            let main_id = threads.iter().find(|t| t.1 == "MainThread").map(|t| t.0);
            let _ = this.update_in(cx, |t, w, cx| {
                let shown = t.render_threads(cx).is_some();
                eprintln!(
                    "smoke-debug: threads {s:?} at {:?} threads={:?} chooser={shown}",
                    t.debug.frames.first().map(|f| format!("{}:{}", f.name, f.line + 1)),
                    threads.iter().map(|t| t.1.as_str()).collect::<Vec<_>>()
                );
                if let Some(id) = main_id {
                    t.select_thread(id, w, cx);
                }
            });
            wait(1500).await;
            let _ = this.update_in(cx, |t, w, cx| {
                let frames: Vec<String> = t.debug.frames.iter().map(|f| format!("{}:{}", f.name, f.line + 1)).collect();
                eprintln!("smoke-debug: main thread -> paused={:?} frames has main:10 {}", t.debug.paused_thread() == main_id, frames.iter().any(|f| f == "main:10"));
                t.dispatch(Action::Resume, w, cx);
            });
            until!(|s: Option<State>| s == Some(State::Ended));
            let _ = this.update_in(cx, |t, _, cx| {
                eprintln!("smoke-debug: threads after end {:?} {:?}", t.debug.threads, t.debug.console.iter().rev().take(3).map(|l| l.0.trim().to_string()).collect::<Vec<_>>());
                let _ = std::fs::remove_file(&thr);
                let _ = cx;
            });
            let smart = dir.join("dbg_smart.py");
            let _ = std::fs::write(&smart, "def a(x):\n    return x + 1\n\n\ndef b(x):\n    return x * 2\n\n\nr = a(b(3))\nprint(r)\n");
            let _ = this.update_in(cx, |t, w, cx| {
                t.open_file(&smart, w, cx);
                if let Some(ed) = t.active_editor().cloned() {
                    ed.update(cx, |e, cx| e.toggle_breakpoint(8, cx));
                }
                t.dispatch(Action::DebugContext, w, cx);
            });
            let s = settled!();
            wait(800).await;
            let _ = this.update_in(cx, |t, w, cx| {
                eprintln!("smoke-debug: smart step {s:?} at {:?}", t.debug.frames.first().map(|f| format!("{}:{}", f.name, f.line + 1)));
                t.dispatch(Action::SmartStepInto, w, cx);
            });
            let mut labels = Vec::new();
            for _ in 0..50 {
                wait(100).await;
                labels = this.update(cx, |t, _| t.debug.chooser.as_ref().map(|s| s.targets.iter().map(|t| t.1.clone()).collect::<Vec<_>>())).ok().flatten().unwrap_or_default();
                if !labels.is_empty() {
                    break;
                }
            }
            let _ = this.update_in(cx, |t, w, cx| {
                let i = labels.iter().position(|l| l.starts_with("a")).unwrap_or(0);
                eprintln!("smoke-debug: smart step targets {labels:?} -> choose {:?}", labels.get(i));
                t.chooser_go(i, w, cx);
            });
            let s = settled!();
            wait(800).await;
            let _ = this.update_in(cx, |t, w, cx| {
                let x = t.debug.vars.iter().find(|r| r.var.name == "x").map(|r| r.var.value.clone());
                eprintln!("smoke-debug: smart step into a -> {s:?} at {:?} x={x:?}", t.debug.frames.first().map(|f| format!("{}:{}", f.name, f.line + 1)));
                t.dispatch(Action::Resume, w, cx);
            });
            until!(|s: Option<State>| s == Some(State::Ended));
            // Exception breakpoint for one class: a handled KeyError passes, a handled ValueError stops.
            let excf = dir.join("dbg_exc_class.py");
            let _ = std::fs::write(&excf, "def f():\n    try:\n        raise KeyError(\"k\")\n    except KeyError:\n        pass\n    try:\n        raise ValueError(\"v\")\n    except ValueError:\n        pass\n    return 1\n\n\nf()\n");
            let _ = this.update_in(cx, |t, w, cx| {
                t.open_file(&excf, w, cx);
                t.dispatch(Action::ViewBreakpoints, w, cx);
                t.bp_add_class(cx);
                for ch in ["V", "a", "l", "u", "e", "E", "r", "r", "o", "r"] {
                    let ks = gpui::Keystroke { modifiers: gpui::Modifiers::default(), key: ch.to_lowercase(), key_char: Some(ch.into()) };
                    t.bp_dialog_key(&gpui::KeyDownEvent { keystroke: ks, is_held: false }, w, cx);
                }
                let ks = gpui::Keystroke { modifiers: gpui::Modifiers::default(), key: "enter".into(), key_char: None };
                t.bp_dialog_key(&gpui::KeyDownEvent { keystroke: ks, is_held: false }, w, cx);
                let saved = std::fs::read_to_string(exc_path()).unwrap_or_default().replace(char::is_whitespace, "");
                eprintln!(
                    "smoke-debug: exception class {:?} args {} saved={}",
                    t.debug.exc_classes.iter().map(|c| (c.name.clone(), c.raised, c.uncaught)).collect::<Vec<_>>(),
                    t.debug.exception_args(),
                    saved.contains("\"name\":\"ValueError\"")
                );
                t.debug.bp_dialog = None;
                t.dispatch(Action::DebugContext, w, cx);
            });
            let s = settled!();
            wait(800).await;
            let _ = this.update_in(cx, |t, w, cx| {
                eprintln!("smoke-debug: class ValueError -> {s:?} at {:?} header={:?}", t.debug.frames.first().map(|f| format!("{}:{}", f.name, f.line + 1)), t.debug.exception);
                if let Some(i) = t.debug.exc_classes.iter().position(|c| c.name == "ValueError") {
                    t.bp_remove_class(i, cx);
                }
                t.dispatch(Action::Resume, w, cx);
            });
            until!(|s: Option<State>| s == Some(State::Ended));
            let _ = std::fs::remove_file(&excf);
            // Jump to Cursor: back to `x = 1` (x stays 2 until stepped), then into another frame (refused).
            let jump = dir.join("dbg_jump.py");
            let _ = std::fs::write(&jump, "def f():\n    x = 1\n    x = 2\n    x = 3\n    return x\n\n\nr = f()\nprint(\"r\", r)\n");
            let _ = this.update_in(cx, |t, w, cx| {
                t.open_file(&jump, w, cx);
                if let Some(ed) = t.active_editor().cloned() {
                    ed.update(cx, |e, cx| e.toggle_breakpoint(3, cx));
                }
                t.dispatch(Action::DebugContext, w, cx);
            });
            let s = settled!();
            wait(800).await;
            let var = |t: &Workspace, n: &str| t.debug.vars.iter().find(|r| r.var.name == n).map(|r| r.var.value.clone());
            let top = |t: &Workspace| t.debug.frames.first().map(|f| format!("{}:{}", f.name, f.line + 1));
            let _ = this.update_in(cx, |t, w, cx| {
                eprintln!("smoke-debug: jump {s:?} at {:?} x={:?}", top(t), var(t, "x"));
                if let Some(ed) = t.active_editor().cloned() {
                    ed.update(cx, |e, cx| e.go_to_line(1, cx));
                }
                t.dispatch(Action::SetNextStatement, w, cx);
            });
            let mut at = None;
            for _ in 0..50 {
                wait(100).await;
                at = this.update(cx, |t, _| top(t)).ok().flatten();
                if at.as_deref() == Some("f:2") {
                    break;
                }
            }
            wait(600).await;
            let _ = this.update_in(cx, |t, w, cx| {
                eprintln!("smoke-debug: jumped -> {at:?} x={:?}", var(t, "x"));
                t.dispatch(Action::StepOver, w, cx);
            });
            let s = settled!();
            wait(800).await;
            let _ = this.update_in(cx, |t, _, cx| {
                eprintln!("smoke-debug: after step {s:?} at {:?} x={:?}", top(t), var(t, "x"));
                if let Some(ed) = t.active_editor().cloned() {
                    t.set_next_statement(ed, 8, cx);
                }
            });
            wait(2000).await;
            let _ = this.update_in(cx, |t, w, cx| {
                eprintln!("smoke-debug: jump out of f -> at {:?} console {:?}", top(t), t.debug.console.last().map(|l| l.0.trim().to_string()));
                if let Some(ed) = t.active_editor().cloned() {
                    ed.update(cx, |e, cx| e.toggle_breakpoint(3, cx));
                }
                let _ = w;
            });
            // The removed breakpoint reaches debugpy first.
            wait(500).await;
            let _ = this.update_in(cx, |t, w, cx| t.dispatch(Action::Resume, w, cx));
            let mut states = Vec::new();
            for _ in 0..60 {
                wait(100).await;
                let st = this.update(cx, |t, _| t.debug.state()).ok().flatten();
                if states.last() != Some(&st) {
                    states.push(st);
                }
                if st == Some(State::Ended) {
                    break;
                }
            }
            wait(1000).await;
            let _ = this.update(cx, |t, cx| {
                let _ = cx;
                eprintln!("smoke-debug: jump states {states:?} output {:?}", t.debug.console.iter().rev().map(|l| l.0.trim()).find(|l| l.starts_with("r ")));
            });
            let _ = std::fs::remove_file(&jump);
            let spin2 = dir.join("dbg_attach_target.py");
            let _ = std::fs::write(&spin2, "import time\n\nn = 0\nwhile True:\n    n += 1\n    time.sleep(0.05)\n");
            let python = this.update(cx, |t, _| t.runs.interpreter(&t.root).map(|i| i.path)).ok().flatten();
            let child = python.and_then(|p| std::process::Command::new(p).arg(&spin2).spawn().ok());
            wait(1500).await;
            if let Some(mut child) = child {
                let pid = child.id();
                let _ = this.update_in(cx, |t, w, cx| t.dispatch(Action::AttachToProcess, w, cx));
                let mut listed = Vec::new();
                for _ in 0..100 {
                    wait(100).await;
                    listed = this.update(cx, |t, _| t.debug.chooser.as_ref().map(|c| c.targets.iter().map(|x| x.0).collect::<Vec<_>>())).ok().flatten().unwrap_or_default();
                    if !listed.is_empty() {
                        break;
                    }
                }
                let _ = this.update_in(cx, |t, w, cx| {
                    eprintln!("smoke-debug: attach list has target {} (of {})", listed.contains(&(pid as i64)), listed.len());
                    t.open_file(&spin2, w, cx);
                    if let Some(ed) = t.active_editor().cloned() {
                        ed.update(cx, |e, cx| e.toggle_breakpoint(4, cx));
                    }
                    if let Some(i) = listed.iter().position(|x| *x == pid as i64) {
                        t.chooser_go(i, w, cx);
                    } else {
                        t.debug.chooser = None;
                    }
                });
                let s = settled!();
                wait(800).await;
                let _ = this.update_in(cx, |t, w, cx| {
                    let n = t.debug.vars.iter().find(|r| r.var.name == "n").map(|r| r.var.value.clone());
                    eprintln!("smoke-debug: attached -> {s:?} at {:?} n={n:?}", t.debug.frames.first().map(|f| format!("{}:{}", f.name, f.line + 1)));
                    if let Some(ed) = t.active_editor().cloned() {
                        ed.update(cx, |e, cx| e.toggle_breakpoint(4, cx));
                    }
                    t.dispatch(Action::Stop, w, cx);
                });
                wait(2500).await;
                let alive = matches!(child.try_wait(), Ok(None));
                let _ = this.update(cx, |t, _| eprintln!("smoke-debug: detach -> state={:?} process alive={alive} console {:?}", t.debug.state(), t.debug.console.last().map(|l| l.0.trim().to_string())));
                // Rerun attaches to the same process; once it has exited, a message.
                let _ = this.update_in(cx, |t, w, cx| {
                    if let Some(ed) = t.active_editor().cloned() {
                        ed.update(cx, |e, cx| e.toggle_breakpoint(4, cx));
                    }
                    t.dispatch(Action::RerunDebug, w, cx);
                });
                let mut again = None;
                for _ in 0..150 {
                    wait(100).await;
                    again = this.update(cx, |t, _| t.debug.session.as_ref().filter(|s| s.attached && matches!(s.state, State::Paused(_))).map(|s| s.name.clone())).ok().flatten();
                    if again.is_some() {
                        break;
                    }
                }
                let _ = this.update_in(cx, |t, w, cx| {
                    eprintln!("smoke-debug: rerun after attach -> {again:?} (pid {pid}) at {:?}", t.debug.frames.first().map(|f| format!("{}:{}", f.name, f.line + 1)));
                    if let Some(ed) = t.active_editor().cloned() {
                        ed.update(cx, |e, cx| e.toggle_breakpoint(4, cx));
                    }
                    t.dispatch(Action::Stop, w, cx);
                });
                wait(1500).await;
                let _ = child.kill();
                let _ = child.wait();
                wait(500).await;
                let _ = this.update_in(cx, |t, w, cx| t.dispatch(Action::RerunDebug, w, cx));
                wait(4000).await;
                let _ = this.update(cx, |t, _| {
                    eprintln!("smoke-debug: rerun after exit -> live={} console {:?}", t.debug.is_live(), t.debug.console.last().map(|l| l.0.trim().to_string()));
                });
            }
            let _ = this.update_in(cx, |_, _, cx| {
                let _ = std::fs::remove_file(&spin2);
                let _ = std::fs::remove_file(&smart);
                let _ = std::fs::remove_file(&raise);
                let _ = std::fs::remove_file(&test);
                let _ = std::fs::remove_file(&exc);
                let _ = std::fs::remove_file(&spin);
                eprintln!("smoke-debug: done");
                cx.quit();
            });
        })
        .detach();
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::collections::HashMap;

    #[test]
    fn inline_values_of_a_function() {
        let lines: Vec<String> = ["x = 1", "def main(n):", "    items = [n, 2]  # n", "    s = \"items\"", "    total = add(items[0], obj.items)"].map(String::from).to_vec();
        let var = |n: &str, v: &str| Variable { name: n.into(), value: v.into(), type_name: String::new(), reference: 0 };
        let vars = [var("n", "5"), var("items", "[5, 2]"), var("x", "1")];
        let v = inline_values(&lines, 4, &vars);
        // From the def line on; not inside strings, comments, or after a dot.
        assert_eq!(v.get(&1).map(String::as_str), Some("n: 5"));
        assert_eq!(v.get(&2).map(String::as_str), Some("items: [5, 2], n: 5"));
        assert_eq!(v.get(&3), None);
        assert_eq!(v.get(&4).map(String::as_str), Some("items: [5, 2]"));
        assert_eq!(v.get(&0), None);
    }

    #[test]
    fn process_targets() {
        assert_eq!(process_target(r#""C:\Py 3\python.exe" -u C:/x/app.py --port 1"#), "app.py");
        assert_eq!(process_target("python -m http.server 8765"), "-m http.server");
        assert_eq!(process_target("python"), "");
    }

    #[test]
    fn smart_step_targets_of_the_line() {
        let t = |v: &[&str]| v.iter().enumerate().map(|(i, s)| (i as i64, s.to_string())).collect::<Vec<_>>();
        let names = |v: Vec<(i64, String)>| v.into_iter().map(|x| x.1).collect::<Vec<_>>();
        let lines: Vec<String> = ["r = a(b(3))", "print(r)"].map(String::from).to_vec();
        let st = statement_text(&lines, 0);
        assert_eq!(names(targets_on_statement(&st, t(&["b(3)", "a(b(3))", "print(r)"]))), ["b(3)", "a(b(3))"]);
        // A call spread over lines; `(call 2)`; a method; a longer name is not a match.
        let lines: Vec<String> = ["x = f(", "    g(1),", "    f(2))  # h(3)", "h(4)"].map(String::from).to_vec();
        let st = statement_text(&lines, 0);
        assert_eq!(names(targets_on_statement(&st, t(&["g(1)", "f(2)", "f (call 2)", "h(4)"]))), ["g(1)", "f(2)", "f (call 2)"]);
        let st = statement_text(&["y = obj.run(xa(1))".to_string()], 0);
        assert_eq!(names(targets_on_statement(&st, t(&["obj.run(xa(1))", "a(2)", "xa(1)"]))), ["obj.run(xa(1))", "xa(1)"]);
        // Nothing matches (labels debugpy shortened): keep all.
        assert_eq!(targets_on_statement("z = 1\n", t(&["q()"])).len(), 1);
    }

    #[test]
    fn exception_breakpoint_args() {
        let c = |n: &str, raised: bool| ExcClass { name: n.into(), raised, uncaught: true, enabled: true };
        assert_eq!(exception_args(false, false, true, &[]), json!({"filters": ["uncaught"]}));
        assert_eq!(exception_args(true, true, true, &[c("ValueError", true)]), json!({"filters": []}));
        let v = exception_args(false, false, true, &[c("ValueError", true), c("KeyError", false), c("", true)]);
        let opts = v["exceptionOptions"].as_array().unwrap();
        assert_eq!(opts.len(), 3);
        assert_eq!(opts[0], json!({"path": [{"names": ["Python Exceptions"]}], "breakMode": "unhandled"}));
        assert_eq!(opts[1], json!({"path": [{"names": ["Python Exceptions"]}, {"names": ["ValueError"]}], "breakMode": "always"}));
        assert_eq!(opts[2]["breakMode"], "unhandled");
        // Nothing for all exceptions: only the classes.
        let v = exception_args(false, false, false, &[ExcClass { enabled: false, ..c("A", true) }, c("B", true)]);
        assert_eq!(v["exceptionOptions"].as_array().unwrap().len(), 1);
    }

    #[test]
    fn console_completion() {
        assert_eq!(completion_start("items.ap"), 6);
        assert_eq!(completion_start("len(ite"), 4);
        assert_eq!(completion_start("x + "), 4);
        assert_eq!(common_prefix(["append", "apply"]), "app");
        assert_eq!(common_prefix(["a"]), "a");
        let mut v = vec![("__len__".to_string(), String::new()), ("_x".into(), String::new()), ("append".into(), String::new())];
        order_completions(&mut v);
        assert_eq!(v.iter().map(|i| i.0.as_str()).collect::<Vec<_>>(), ["append", "_x", "__len__"]);
        let c = ConsoleCompletion { items: v, start: 6, sel: 0 };
        assert_eq!(c.shown("items.ap").len(), 1);
        assert_eq!(c.shown("items.").len(), 3);
    }

    #[test]
    fn return_value_labels() {
        assert_eq!(return_label("(return) add").as_deref(), Some("↩ add()"));
        assert_eq!(return_label("total"), None);
    }

    #[test]
    fn hover_expressions() {
        let l = "    total = obj.items[0] + add(x)  # items here";
        let at = |s: &str| l.find(s).unwrap();
        assert_eq!(hover_expression(l, at("total") + 2).as_deref(), Some("total"));
        assert_eq!(hover_expression(l, at("items") + 1).as_deref(), Some("obj.items"));
        assert_eq!(hover_expression(l, at("add")).as_deref(), Some("add"));
        assert_eq!(hover_expression(l, at("here")), None);
        assert_eq!(hover_expression("s = \"x y\" if a else b", 5), None);
        assert_eq!(hover_expression("s = \"x y\" if a else b", 10), None);
        assert_eq!(hover_expression("s = \"x y\" if a else b", 13).as_deref(), Some("a"));
    }

    #[test]
    fn traceback_in_python_order() {
        let s = "  File \"a.py\", line 2, in f\n    return 1 / x\n  File \"a.py\", line 5, in <module>\n    f(0)\nZeroDivisionError: division by zero\n";
        assert_eq!(python_order(s), "  File \"a.py\", line 5, in <module>\n    f(0)\n  File \"a.py\", line 2, in f\n    return 1 / x");
    }

    #[test]
    fn launch_from_command() {
        let cmd = brisk_run::Command { program: "py".into(), args: vec!["-X".into(), "utf8".into(), "-m".into(), "pkg.main".into(), "--n".into(), "3".into()], cwd: "c".into(), env: HashMap::new() };
        let l = launch_for(&cmd);
        // "-X utf8": the value of an option is not the target.
        assert!(l.module);
        assert_eq!(l.target, "pkg.main");
        assert_eq!(l.args, vec!["--n", "3"]);
        let cmd = brisk_run::Command { program: "py".into(), args: vec!["C:/p/a.py".into(), "x".into()], cwd: "c".into(), env: HashMap::new() };
        let l = launch_for(&cmd);
        assert!(!l.module);
        assert_eq!((l.target.as_str(), l.args.clone()), ("C:/p/a.py", vec!["x".to_string()]));
    }
}
