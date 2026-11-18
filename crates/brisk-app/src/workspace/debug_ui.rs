//! Debugging with debugpy, as in PyCharm: Shift+F9 / Ctrl+Shift+F9 start, breakpoints
//! (Ctrl+F8 or a click on the line numbers; right click for a condition), F8 / F7 / Shift+F8
//! step, F9 resume, Alt+F9 run to cursor, Ctrl+F2 stop, Alt+F8 evaluate. The Debug window
//! shows frames, variables (expandable) and the console with an evaluate line.

use super::{BottomTool, PopupItem, PopupKind, Target, Workspace};
use crate::editor::breakpoints::Breakpoint;
use crate::editor::Editor;
use crate::theme::{Theme, MONO_FONT};
use brisk_dap::{Client, Event, Frame, Launch, SourceBreakpoint, Variable};
use brisk_keymap::Action;
use brisk_run::{RunConfig, RunKind};
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
}

impl Debug {
    pub fn new(root: &Path, cx: &mut gpui::App) -> Self {
        Self {
            session: None,
            stored: load(root),
            console: Vec::new(),
            input: String::new(),
            history: Vec::new(),
            history_pos: 0,
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
        }
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
            })
        })
        .collect()
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

/// `python [opts] (-m module | script) args` → what debugpy launches.
fn launch_for(cmd: &brisk_run::Command) -> Launch {
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
            let bp = (b.line, Breakpoint { condition: b.condition.clone(), verified: None });
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
                list.push(json!({"path": rel, "line": line, "condition": b.condition}));
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
            self.debug.stored.iter().filter(|b| b.path == p).map(|b| (b.line, Breakpoint { condition: b.condition.clone(), verified: None })).collect();
        if mine.is_empty() {
            return;
        }
        self.debug.stored.retain(|b| b.path != p);
        ed.update(cx, |e, cx| {
            e.set_breakpoints(&mine);
            cx.notify();
        });
    }

    /// An editor closes: keep its breakpoints.
    pub(super) fn stash_breakpoints(&mut self, ed: &Entity<Editor>, cx: &mut Context<Self>) {
        let Some(p) = ed.read(cx).path.clone() else { return };
        let list = ed.read(cx).breakpoint_list();
        self.debug.stored.retain(|b| b.path != p);
        self.debug.stored.extend(list.into_iter().map(|(line, b)| StoredBp { path: p.clone(), line, condition: b.condition }));
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
            .filter(|(p, _)| p == path)
            .flat_map(|(_, l)| l.into_iter().map(|(line, b)| SourceBreakpoint { line, condition: b.condition }))
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

    /// Ctrl+Shift+F8: all breakpoints; Enter goes there.
    pub(super) fn breakpoint_items(&self, q: &str, cx: &gpui::App) -> Vec<PopupItem> {
        let ql = q.trim().to_lowercase();
        let mut v = Vec::new();
        for (p, bps) in self.all_breakpoints(cx) {
            let rel = p.strip_prefix(&self.root).unwrap_or(&p).to_string_lossy().replace('\\', "/");
            for (line, b) in bps {
                let label = format!("{rel}:{}", line + 1);
                let detail = b.condition.clone().map(|c| format!("if {c}")).unwrap_or_default();
                if ql.is_empty() || label.to_lowercase().contains(&ql) || detail.to_lowercase().contains(&ql) {
                    v.push(PopupItem { label, detail, hint: String::new(), target: Target::Bookmark(p.clone(), line) });
                }
            }
        }
        v
    }

    // --- Session ----------------------------------------------------------------------

    pub(super) fn debug_action(&mut self, action: Action, window: &mut Window, cx: &mut Context<Self>) -> bool {
        use Action as A;
        match action {
            A::ToggleBreakpoint => self.toggle_breakpoint_here(window, cx),
            A::ViewBreakpoints => {
                self.structure = self.breakpoint_items("", cx);
                self.open_popup(PopupKind::Breakpoints);
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
            A::RerunDebug => match self.debug.last.clone() {
                Some(cfg) => self.start_debug(cfg, window, cx),
                None => self.status = "Rerun: nothing has been debugged yet".into(),
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

    fn start_debug(&mut self, cfg: RunConfig, window: &mut Window, cx: &mut Context<Self>) {
        self.save_all(cx);
        if self.debug.is_live() {
            self.stop_debug(cx);
        }
        self.debug.last = Some(cfg.clone());
        let python = match &cfg.interpreter {
            Some(p) => Some(brisk_run::resolve(&self.root, p)),
            None => self.runs.interpreter(&self.root).map(|i| i.path),
        };
        let Some(python) = python else {
            self.show_toast("No Python interpreter configured: create a .venv or install Python".into(), true, cx);
            return;
        };
        let mut cmd = brisk_run::build(&self.root, &cfg, &python);
        if cfg.kind == RunKind::Pytest {
            // No live test tree while debugging: plain pytest output in the console.
            cmd.args.retain(|a| a != "-p");
        }
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
                    Ok((client, events)) => this.attach_session(client, events, name, launch, window, cx),
                }
                cx.notify();
            });
        })
        .detach();
        cx.notify();
    }

    fn attach_session(&mut self, client: Arc<Client>, mut events: futures::channel::mpsc::UnboundedReceiver<Event>, name: String, launch: Launch, window: &mut Window, cx: &mut Context<Self>) {
        let task = cx.spawn_in(window, async move |this, cx| {
            while let Some(ev) = events.next().await {
                if this.update_in(cx, |this, window, cx| this.debug_event(ev, window, cx)).is_err() {
                    break;
                }
            }
        });
        self.debug.session = Some(Session { client: client.clone(), name, state: State::Starting, _events: task, temp: None, exit_code: None });
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
            let res = client.request("launch", brisk_dap::launch_args(&launch)).await;
            if let Ok(Err(e)) = res {
                let _ = this.update(cx, |this, cx| {
                    this.debug.info(format!("Cannot launch: {e}"));
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
            let rx = s.client.request("disconnect", json!({"terminateDebuggee": true}));
            let client = s.client.clone();
            cx.spawn(async move |_, cx| {
                let _ = futures::future::select(rx, cx.background_executor().timer(std::time::Duration::from_secs(2))).await;
                client.kill();
            })
            .detach();
        }
        self.debug.info("Stopped");
        self.end_session(cx);
    }

    fn end_session(&mut self, cx: &mut Context<Self>) {
        if let Some(s) = self.debug.session.as_mut() {
            s.state = State::Ended;
        }
        self.debug.frames.clear();
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
                self.debug_request("setExceptionBreakpoints", json!({"filters": ["uncaught"]}));
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
            let first = match scopes.first() {
                Some(s) => client.request("variables", json!({"variablesReference": s.reference})).await.ok().and_then(Result::ok).map(|v| brisk_dap::parse_variables(&v)),
                None => None,
            };
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
        self.debug.history.retain(|h| *h != expr);
        self.debug.history.push(expr.clone());
        self.debug.history_pos = self.debug.history.len();
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
        match k.key.as_str() {
            "enter" => self.evaluate_input(cx),
            "backspace" => {
                self.debug.input.pop();
            }
            "escape" => self.debug.input.clear(),
            "up" => {
                if self.debug.history_pos > 0 {
                    self.debug.history_pos -= 1;
                    self.debug.input = self.debug.history[self.debug.history_pos].clone();
                }
            }
            "down" => {
                if self.debug.history_pos + 1 < self.debug.history.len() {
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
        let can_rerun = d.last.is_some();
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
        let tool = |id: &'static str, glyph: &'static str, tip: &'static str, on: bool, color: gpui::Hsla, action: Action, cx: &mut Context<Self>| {
            div()
                .id(id)
                .w(px(26.))
                .h(px(24.))
                .flex()
                .items_center()
                .justify_center()
                .rounded(px(5.))
                .text_color(if on { color } else { Theme::text_dim() })
                .when(on, |x| x.cursor_pointer().hover(|s| s.bg(Theme::hover_row())).active(|s| s.bg(Theme::press())))
                .child(glyph)
                .tooltip(move |_, cx| cx.new(|_| super::Tip(tip.into())).into())
                .on_click(cx.listener(move |this, _, w, cx| {
                    if on {
                        this.dispatch(action, w, cx);
                    }
                }))
        };
        let green: gpui::Hsla = gpui::rgb(0x5fb865).into();
        let toolbar = div()
            .flex()
            .items_center()
            .gap(px(2.))
            .px(px(6.))
            .h(px(30.))
            .border_b_1()
            .border_color(Theme::divider())
            .child(tool("dbg-rerun", "↻", "Rerun Debug Session", can_rerun, green, Action::RerunDebug, cx))
            .child(tool("dbg-resume", "▶", "Resume Program  F9", paused && !d.pending, green, Action::Resume, cx))
            .child(tool("dbg-pause", "‖", "Pause Program", running, Theme::accent(), Action::PauseProgram, cx))
            .child(tool("dbg-stop", "■", "Stop  Ctrl+F2", live, Theme::error(), Action::Stop, cx))
            .child(div().w(px(1.)).h(px(16.)).mx(px(4.)).bg(Theme::divider()))
            .child(tool("dbg-over", "↷", "Step Over  F8", paused, Theme::accent(), Action::StepOver, cx))
            .child(tool("dbg-into", "↓", "Step Into  F7", paused, Theme::accent(), Action::StepInto, cx))
            .child(tool("dbg-out", "↑", "Step Out  Shift+F8", paused, Theme::accent(), Action::StepOut, cx))
            .child(tool("dbg-cursor", "⇥", "Run to Cursor  Alt+F9", paused, Theme::accent(), Action::RunToCursor, cx))
            .child(tool("dbg-eval", "=", "Evaluate Expression  Alt+F8", paused, Theme::accent(), Action::EvaluateExpression, cx))
            .child(div().w(px(1.)).h(px(16.)).mx(px(4.)).bg(Theme::divider()))
            .child(tool("dbg-bps", "●", "View Breakpoints  Ctrl+Shift+F8", true, Theme::breakpoint(), Action::ViewBreakpoints, cx))
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
        let d = &self.debug;
        let frames = div()
            .id("dbg-frames")
            .w(px(300.))
            .h_full()
            .flex_shrink_0()
            .overflow_y_scroll()
            .border_r_1()
            .border_color(Theme::divider())
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
            let arrow = if r.var.reference > 0 { if r.expanded { "▾" } else { "▸" } } else { " " };
            let header = matches!(r.kind, RowKind::Scope | RowKind::WatchHeader);
            let watch = matches!(r.kind, RowKind::Watch(_));
            let editing_name = matches!((r.kind, edit.as_ref().map(|e| e.target)), (RowKind::Watch(w), Some(EditTarget::Watch(x))) if w == x);
            let editing_value = edit.as_ref().is_some_and(|e| e.target == EditTarget::Value(i));
            let error = r.var.value.starts_with("<error: ") && watch;
            let name_color = if header { Theme::text_muted() } else if watch { Theme::accent() } else { Theme::syntax(brisk_syntax::Highlight::Property) };
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
                    .child(div().w(px(12.)).text_color(Theme::text_dim()).child(arrow))
                    .child(if editing_name {
                        input(&edit.as_ref().map(|e| e.text.clone()).unwrap_or_default(), focused).into_any_element()
                    } else {
                        div().text_color(name_color).child(r.var.name.clone()).into_any_element()
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
            .child(div().id("dbg-vars-list").flex_1().min_h_0().overflow_y_scroll().font_family(MONO_FONT).text_size(px(12.5)).children(rows));
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
            .font_family(MONO_FONT)
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
            .font_family(MONO_FONT)
            .text_size(px(12.5))
            .child(div().text_color(Theme::accent()).child(">>>"))
            .child(if d.input.is_empty() && !focused {
                div().text_color(Theme::text_dim()).child(if can_eval { "Evaluate an expression in the selected frame (Alt+F8)" } else { "Expressions can be evaluated while the program is paused" }).into_any_element()
            } else {
                div().flex().items_center().child(d.input.clone()).when(focused, |x| x.child(div().w(px(1.5)).h(px(15.)).bg(Theme::caret()))).into_any_element()
            });
        div().size_full().flex().flex_col().child(lines).child(input).into_any_element()
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
                            "state={:?} top={:?} bps={bps:?} popup={} tab={:?} sel={:?} edit={:?} watches={:?}",
                            t.debug.state(),
                            t.debug.frames.first().map(|f| f.line + 1),
                            t.popup.is_some(),
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
            let _ = this.update_in(cx, |t, _, _| {
                let tail: Vec<String> = t.debug.console.iter().rev().take(2).map(|(l, _)| l.trim_end().to_string()).collect();
                eprintln!("smoke-debug: evaluate -> {tail:?}");
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
            let _ = this.update_in(cx, |t, _, cx| {
                eprintln!("smoke-debug: after stop exception={:?}", t.debug.exception);
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
