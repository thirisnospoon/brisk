//! Debugging with debugpy, as in PyCharm: Shift+F9 / Ctrl+Shift+F9 start, breakpoints
//! (Ctrl+F8 or a click on the line numbers; right click for a condition), F8 / F7 / Shift+F8
//! step, F9 resume, Alt+F9 run to cursor, Ctrl+F2 stop, Alt+F8 evaluate. The Debug window
//! shows frames, variables (expandable) and the console with an evaluate line.

use super::{BottomTool, PopupItem, PopupKind, Tab, Target, Workspace};
use crate::data_view::{viewable, DataView};
use crate::editor::breakpoints::Breakpoint;
use crate::editor::Editor;
use crate::theme::ui_px;
use crate::theme::{mono_font, Theme};
use brisk_dap::attach::{process_lines, process_target};
use brisk_dap::console::{common_prefix, completion_start, order_completions};
use brisk_dap::exceptions::exception_args;
use brisk_dap::exceptions::ExcClass;
use brisk_dap::source::{hover_expression, inline_values, statement_text, targets_on_statement};
use brisk_dap::values::{decode_hex_repr, full_value_expression, python_order, return_label};
use brisk_dap::{Client, Event, Frame, Launch, SourceBreakpoint, Variable};
use brisk_keymap::Action;
use brisk_run::RunConfig;
use futures::StreamExt;
use gpui::prelude::*;
use gpui::{div, list, px, uniform_list, Context, Entity, FocusHandle, ListAlignment, ListOffset, ListState, Task, UniformListScrollHandle, Window};
use serde_json::{json, Value};
use std::path::{Path, PathBuf};
use std::sync::Arc;

mod breakpoints;
mod console;
mod session;
#[cfg(feature = "smoke")]
mod smoke;
mod variables;
mod window;

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
    /// The Debug Console output, virtualized (`console` holds up to 5000 lines; only the
    /// lines in view are built, with their link checks). See `console_changed`.
    console_out: ListState,
    console_dropped: usize,
    console_dirty: bool,
    console_follow: bool,
    /// Variables / Watches: a uniform_list (an expanded dict or DataFrame has thousands of
    /// rows); ↑/↓ keep the selection in view.
    vars_scroll: UniformListScrollHandle,
    vars_bar: crate::scrollbar::BarState,
    /// Lines with exec marks, to clear them.
    marked: Vec<PathBuf>,
    /// The configuration of the last session (Rerun).
    pub last: Option<RunConfig>,
    /// The adapter is being started; no session yet.
    pub pending: bool,
    /// Bumped by every start and stop: a start that finishes late is dropped.
    gen: u64,
    /// When the last session was started or attached (Ctrl+F2 stops the newer of it and a run).
    started: Option<std::time::Instant>,
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
            console_out: ListState::new(0, ListAlignment::Bottom, px(300.)),
            console_dropped: 0,
            console_dirty: false,
            console_follow: false,
            vars_scroll: UniformListScrollHandle::new(),
            vars_bar: Default::default(),
            marked: Vec::new(),
            last: None,
            pending: false,
            gen: 0,
            started: None,
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
            self.console_dropped += 1000;
        }
        self.console_changed(true);
    }

    /// `console` changed (lines added at the end, `follow`: and the view sticks to the end;
    /// or the whole of it replaced or cleared, `follow` false).
    pub(super) fn console_changed(&mut self, follow: bool) {
        self.console_dirty = true;
        self.console_follow |= follow;
    }

    /// Bring the list state in line with `console` before a frame.
    fn sync_console(&mut self) {
        if !std::mem::take(&mut self.console_dirty) {
            return;
        }
        let dropped = std::mem::take(&mut self.console_dropped);
        if dropped > 0 {
            let count = self.console_out.item_count();
            self.console_out.splice(0..dropped.min(count), 0);
        }
        let (count, n) = (self.console_out.item_count(), self.console.len());
        if count > n {
            self.console_out.reset(n);
        } else {
            // The last line may have grown by a chunk: measure it again.
            let from = count.saturating_sub(1);
            self.console_out.splice(from..count, n - from);
        }
        if std::mem::take(&mut self.console_follow) {
            self.console_out.scroll_to(ListOffset { item_ix: n, offset_in_item: px(0.) });
        }
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
        let head = Variable { name: "Watches".into(), value: String::new(), type_name: String::new(), reference: 0, evaluate_name: String::new() };
        let mut v = vec![VarRow { depth: 0, var: head, expanded: true, kind: RowKind::WatchHeader, parent: 0 }];
        for (i, w) in self.watches.iter().enumerate() {
            let var = match vals.as_ref().and_then(|v| v.get(i)) {
                Some(Ok(x)) => Variable { name: w.clone(), evaluate_name: w.clone(), ..x.clone() },
                Some(Err(e)) => Variable { name: w.clone(), value: format!("<error: {e}>"), type_name: String::new(), reference: -1, evaluate_name: w.clone() },
                None => Variable { name: w.clone(), value: String::new(), type_name: String::new(), reference: 0, evaluate_name: w.clone() },
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
        self.console_changed(true);
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
    let list = v
        .as_ref()
        .and_then(|v| v.get(root.to_string_lossy().to_lowercase()))
        .and_then(|p| p.get("classes"))
        .and_then(Value::as_array)
        .cloned()
        .unwrap_or_default();
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
    v.get(root.to_string_lossy().to_lowercase())
        .and_then(Value::as_array)
        .map(|a| a.iter().filter_map(Value::as_str).map(str::to_string).collect())
        .unwrap_or_default()
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

#[cfg(test)]
mod tests {
    use super::*;
    use std::collections::HashMap;

    #[test]
    fn console_completion_filter() {
        let v = vec![("append".to_string(), String::new()), ("_x".into(), String::new()), ("__len__".into(), String::new())];
        let c = ConsoleCompletion { items: v, start: 6, sel: 0 };
        assert_eq!(c.shown("items.ap").len(), 1);
        assert_eq!(c.shown("items.").len(), 3);
    }

    #[test]
    fn launch_from_command() {
        let cmd = brisk_run::Command {
            program: "py".into(),
            args: vec!["-X".into(), "utf8".into(), "-m".into(), "pkg.main".into(), "--n".into(), "3".into()],
            cwd: "c".into(),
            env: HashMap::new(),
        };
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
