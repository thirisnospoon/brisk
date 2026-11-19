//! Terminal tool window (Alt+F12): tabs of shell sessions, grid rendering, PyCharm key behavior.

use crate::theme::{Theme, mono_font};
use brisk_terminal::{encode_key, KeyMods, SelectKind, Session, Snapshot, SpawnOptions, TermEvent};
use futures::channel::mpsc;
use futures::StreamExt;
use gpui::prelude::*;
use gpui::{
    div, fill, font, point, px, rgb, size, App, Bounds, ClipboardItem, ContentMask, Context, Element, ElementId, Entity,
    FocusHandle, Focusable, FontStyle, FontWeight, GlobalElementId, Hsla, LayoutId, MouseButton, MouseDownEvent,
    MouseMoveEvent, Pixels, Point, ScrollWheelEvent, ShapedLine, SharedString, Style, TextRun, UnderlineStyle, Window,
};
use std::path::PathBuf;

mod tests_view;
mod console_find;
mod console_prefs;
mod scroll_bar;
mod dashboard;
pub use dashboard::RunInfo;
pub use tests_view::TestsOutcome;

const CELL_H: f32 = 18.0;
const FONT_SIZE: f32 = 13.0;
/// The Run window keeps at most this many tabs: the oldest finished ones close first.
pub const MAX_RUN_TABS: usize = 5;
/// Columns of a Run console with soft wrap off (longer lines still wrap).
const NOWRAP_COLS: usize = 500;

struct Tab {
    id: u64,
    session: Session,
    title: String,
    number: usize,
    /// Set for Run window tabs.
    run: Option<RunTab>,
}

struct RunTab {
    name: String,
    cfg: brisk_run::RunConfig,
    /// As built from the configuration, before the test plugin is attached.
    cmd: brisk_run::Command,
    running: bool,
    code: Option<i32>,
    /// pytest runs: the live test tree.
    tests: Option<tests_view::TestsView>,
    /// Services shows how long a run took.
    started: std::time::Instant,
    ended: Option<std::time::Instant>,
    /// Pin Tab: a new run of the configuration opens its own tab instead of replacing this one.
    pinned: bool,
    /// The user stopped it (■ / Ctrl+F2 / Services): on Windows a killed process exits with
    /// code 1, so the exit code alone cannot tell.
    stopped: bool,
}

impl Tab {
    /// Stop the run's process at the user's request.
    fn stop_run(&mut self) {
        if let Some(r) = self.run.as_mut() {
            r.stopped = true;
        }
        self.session.kill();
    }
}

/// Events from the Run window.
pub enum RunEvent {
    /// A traceback / `file.py:12` link was clicked.
    Open(PathBuf, usize),
    /// A `file.py:12:5` link: line and column (1-based).
    OpenAt(PathBuf, usize, usize),
    /// An `http(s)://` link was clicked (a server's address).
    OpenUrl(String),
    Finished { name: String, code: Option<i32>, tests: Option<TestsOutcome> },
    /// The Rerun button: the workspace saves files first, then calls [`TerminalPanel::rerun`].
    RerunRequested,
    /// "Open in Terminal": run this command in the project terminal.
    OpenInTerminal(brisk_run::Command),
    /// "Rerun Failed Tests": then [`TerminalPanel::rerun_failed`].
    RerunFailedRequested,
    /// ▶ on a node of the test tree: run it as a temporary configuration of its own.
    RunTests(Box<brisk_run::RunConfig>),
}

impl gpui::EventEmitter<RunEvent> for TerminalPanel {}

const LINK: [u8; 3] = [0x56, 0x8a, 0xf2];
/// Shortest gap between two redraws caused by process output (Windows timers tick every 15.6 ms, so 8 ms
/// waits come out as about one frame at 60 Hz).
const FRAME: std::time::Duration = std::time::Duration::from_millis(8);
/// Redraw requests from process output and renders of the panel (the flood smoke reads them).
pub(crate) static PUMP_NOTIFIES: std::sync::atomic::AtomicUsize = std::sync::atomic::AtomicUsize::new(0);
pub(crate) static PANEL_RENDERS: std::sync::atomic::AtomicUsize = std::sync::atomic::AtomicUsize::new(0);

/// A grid row as text plus the cell column of every character.
fn row_text(cells: &[brisk_terminal::CellView]) -> (String, Vec<usize>) {
    let mut s = String::new();
    let mut idx = Vec::new();
    for (c, cell) in cells.iter().enumerate() {
        if cell.c != '\0' {
            s.push(cell.c);
            idx.push(c);
        }
    }
    (s, idx)
}

pub struct TerminalPanel {
    root: PathBuf,
    tabs: Vec<Tab>,
    active: usize,
    next_id: u64,
    focus: FocusHandle,
    events: mpsc::UnboundedSender<(u64, TermEvent)>,
    grid: Option<GridGeom>,
    selecting: bool,
    pub error: Option<String>,
    /// The project interpreter's environment for new shells.
    activation: Option<crate::terminal_env::Activation>,
    /// Run tool window (process output) instead of Terminal (shells).
    run_mode: bool,
    /// Run window toggles: wrap long lines at the window edge / follow new output.
    pub soft_wrap: bool,
    pub scroll_to_end: bool,
    /// Soft wrap off: first visible column.
    h_scroll: usize,
    /// The two toggles above, per run configuration.
    prefs: console_prefs::ConsolePrefs,
    /// Ctrl+F in the Run console.
    search: console_find::ConsoleSearch,
    /// "Rerun Failed Tests First": pytest runs start with the last run's failures.
    pub failed_first: bool,
    /// The Test History list is open.
    pub history_open: bool,
    bar: crate::scrollbar::BarState,
}

#[derive(Clone, Copy)]
struct GridGeom {
    bounds: Bounds<Pixels>,
    cell_w: Pixels,
}

fn hsla(c: [u8; 3]) -> Hsla {
    // The terminal's default colors follow the theme.
    match c {
        [0xdf, 0xe1, 0xe5] => return Theme::text(),
        [0x1e, 0x1f, 0x22] => return Theme::editor_bg(),
        _ => {}
    }
    rgb(((c[0] as u32) << 16) | ((c[1] as u32) << 8) | c[2] as u32).into()
}

impl TerminalPanel {
    /// The Terminal tool window; shells start with `activation` (the project interpreter's
    /// environment) when it is known, else with the project's `.venv` on PATH.
    pub fn new(root: PathBuf, activation: Option<crate::terminal_env::Activation>, cx: &mut Context<Self>) -> Self {
        let mut panel = Self::with_mode(root, false, cx);
        panel.activation = activation;
        panel.new_tab();
        panel
    }

    /// The Run tool window: one tab per run configuration, no shell.
    pub fn new_run(root: PathBuf, cx: &mut Context<Self>) -> Self {
        Self::with_mode(root, true, cx)
    }

    fn with_mode(root: PathBuf, run_mode: bool, cx: &mut Context<Self>) -> Self {
        let (tx, mut rx) = mpsc::unbounded::<(u64, TermEvent)>();
        // Pump PTY events onto the UI thread, coalescing bursts of output into one redraw, at
        // most one per frame: a process printing 50k lines/s wakes the UI ~60 times a second.
        cx.spawn(async move |this, cx| {
            let mut last_notify: Option<std::time::Instant> = None;
            while let Some(first) = rx.next().await {
                if let Some(wait) = last_notify.and_then(|t| FRAME.checked_sub(t.elapsed())) {
                    cx.background_executor().timer(wait).await;
                }
                let mut batch = vec![first];
                while let Ok(ev) = rx.try_recv() {
                    batch.push(ev);
                }
                let alive = this.update(cx, |this, cx| {
                    for (id, ev) in batch {
                        match ev {
                            TermEvent::Title(t) => {
                                if let Some(tab) = this.tabs.iter_mut().find(|t| t.id == id) {
                                    tab.title = t;
                                }
                            }
                            TermEvent::ChildExit(code) => {
                                if let Some(r) = this.tabs.iter_mut().find(|t| t.id == id).and_then(|t| t.run.as_mut()) {
                                    r.code = code;
                                }
                            }
                            TermEvent::Exit if this.run_mode => {
                                if let Some(t) = this.tabs.iter_mut().find(|t| t.id == id) {
                                    if let Some(r) = t.run.as_mut() {
                                        r.running = false;
                                        r.ended = Some(std::time::Instant::now());
                                        if r.stopped {
                                            r.code = None;
                                        }
                                        let end = match r.code {
                                            Some(c) => format!("Process finished with exit code {c}"),
                                            None => "Process terminated".to_string(),
                                        };
                                        t.session.inject(format!("\r\n\x1b[0m{end}\r\n\x1b[?25l").as_bytes());
                                        let tests = r.tests.as_mut().map(|t| t.finish());
                                        if let Some(t) = r.tests.as_ref() {
                                            TerminalPanel::save_test_history(&this.root, &r.name, t);
                                        }
                                        cx.emit(RunEvent::Finished { name: r.name.clone(), code: r.code, tests });
                                    }
                                }
                            }
                            TermEvent::Exit => this.close_tab_id(id),
                            TermEvent::Wakeup if this.run_mode && this.scroll_to_end => {
                                if let Some(t) = this.tabs.iter().find(|t| t.id == id) {
                                    t.session.scroll(-(1 << 30));
                                }
                            }
                            TermEvent::Wakeup | TermEvent::Bell => {}
                        }
                    }
                    this.search_output_changed(cx);
                    PUMP_NOTIFIES.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
                    cx.notify();
                });
                last_notify = Some(std::time::Instant::now());
                if alive.is_err() {
                    break;
                }
            }
        })
        .detach();

        Self {
            root,
            tabs: Vec::new(),
            active: 0,
            next_id: 1,
            focus: cx.focus_handle(),
            events: tx,
            grid: None,
            selecting: false,
            error: None,
            run_mode,
            activation: None,
            soft_wrap: true,
            scroll_to_end: true,
            h_scroll: 0,
            prefs: Default::default(),
            search: Default::default(),
            failed_first: false,
            history_open: false,
            bar: Default::default(),
        }
    }

    /// Start (or restart, in its own tab) a run configuration.
    pub fn run(&mut self, cfg: brisk_run::RunConfig, cmd: brisk_run::Command) -> Result<(), String> {
        self.run_in(cfg, cmd, None)
    }

    /// Run in tab `into` (a Rerun of that tab, pinned or not), else in the configuration's
    /// unpinned tab, else in a new one.
    pub fn run_in(&mut self, cfg: brisk_run::RunConfig, cmd: brisk_run::Command, into: Option<u64>) -> Result<(), String> {
        let id = self.next_id;
        self.next_id += 1;
        let tx = self.events.clone();
        let (cols, rows) = self.tabs.get(self.active).map(|t| t.session.size()).unwrap_or((120, 12));
        let mut spawn = cmd.clone();
        let tests = if cfg.kind.is_tests() {
            let events = brisk_run::pytest::attach_runner(&mut spawn, cfg.kind).map_err(|e| format!("Cannot prepare the test runner: {e}"))?;
            if self.failed_first {
                let failed = brisk_run::pytest::history::last_failed(&self.root, &cfg.name);
                let list = events.with_extension("first");
                if !failed.is_empty() && std::fs::write(&list, failed.join("\n")).is_ok() {
                    spawn.env.insert("BRISK_TESTS_FIRST".into(), list.display().to_string());
                }
            }
            Some(tests_view::TestsView::start(events, id, self.events.clone()))
        } else {
            None
        };
        let on_event = move |e| {
            let _ = tx.unbounded_send((id, e));
        };
        let session = if cfg.emulate_terminal {
            // ConPTY drops the last output of a process that exits at once (the pipe closes
            // before it flushes): the interpreter runs the program as a child, waits for it,
            // gives the console a moment, then exits with its code.
            const WRAP: &str = "import subprocess, sys, time
r = subprocess.call(sys.argv[1:])
time.sleep(0.3)
sys.exit(r)";
            let mut args = vec!["-c".to_string(), WRAP.to_string(), spawn.program.display().to_string()];
            args.extend(spawn.args.iter().cloned());
            let opts = brisk_terminal::SpawnOptions { shell: Some((spawn.program.display().to_string(), args)), cwd: Some(spawn.cwd.clone()), env: spawn.env.clone() };
            Session::spawn_process_pty(opts, &cmd.display(), cols, rows, on_event)
        } else {
            Session::spawn_process(spawn.to_std(), &cmd.display(), cols, rows, on_event)
        }
        .map_err(|e| format!("Cannot run {}: {e}", cmd.program.display()))?;
        let name = cfg.name.clone();
        // "Allow multiple instances": a running tab of the configuration stays.
        let multiple = cfg.allow_multiple;
        let tab = Tab { id, session, title: String::new(), number: 0, run: Some(RunTab { name: name.clone(), cfg, cmd, running: true, code: None, tests, started: std::time::Instant::now(), ended: None, pinned: false, stopped: false }) };
        let reuse = into
            .and_then(|id| self.tabs.iter().position(|t| t.id == id && t.run.is_some()))
            .or_else(|| self.tabs.iter().position(|t| t.run.as_ref().is_some_and(|r| r.name == name && !r.pinned && !(multiple && r.running))));
        if let Some(i) = reuse {
            // PyCharm reuses the configuration's tab (stopping a run still in progress).
            self.tabs[i].session.kill();
            let pinned = self.tabs[i].run.as_ref().is_some_and(|r| r.pinned);
            self.tabs[i] = tab;
            if let Some(r) = self.tabs[i].run.as_mut() {
                r.pinned = pinned;
            }
            self.active = i;
        } else {
            self.tabs.push(tab);
            self.active = self.tabs.len() - 1;
            self.trim_run_tabs();
        }
        self.h_scroll = 0;
        Ok(())
    }

    /// Close the oldest finished tabs beyond [`MAX_RUN_TABS`]; running and pinned ones stay.
    fn trim_run_tabs(&mut self) {
        while self.tabs.len() > MAX_RUN_TABS {
            let active_id = self.tabs.get(self.active).map(|t| t.id);
            let Some(i) = self.tabs.iter().position(|t| Some(t.id) != active_id && !t.run.as_ref().is_some_and(|r| r.running || r.pinned)) else { break };
            self.tabs.remove(i);
            self.active = active_id.and_then(|id| self.tabs.iter().position(|t| t.id == id)).unwrap_or(0);
        }
    }

    /// Names of the Run window tabs and whether each is running (smoke tests).
    pub fn run_tabs(&self) -> Vec<(String, bool)> {
        self.tabs.iter().filter_map(|t| t.run.as_ref().map(|r| (r.name.clone(), r.running))).collect()
    }

    /// Clear All: empty the active console (screen and scrollback).
    pub fn clear(&mut self) {
        if let Some(s) = self.session() {
            s.clear_selection();
            // 2J moves the screen into the scrollback, 3J then drops the scrollback.
            s.inject(b"\x1b[0m\x1b[2J\x1b[3J\x1b[H");
        }
    }

    pub fn toggle_soft_wrap(&mut self) {
        self.soft_wrap = !self.soft_wrap;
        self.h_scroll = 0;
        self.store_console_prefs(true);
    }

    pub fn toggle_scroll_to_end(&mut self) {
        self.scroll_to_end = !self.scroll_to_end;
        self.store_console_prefs(false);
        if self.scroll_to_end {
            if let Some(s) = self.session() {
                s.scroll(-(1 << 30));
            }
        }
    }

    /// Columns of the active session (smoke tests: soft wrap off widens the grid).
    pub fn columns(&self) -> usize {
        self.session().map(|s| s.size().0).unwrap_or(0)
    }

    /// The active Run tab's configuration and command (what Rerun starts again).
    pub fn active_run(&self) -> Option<(brisk_run::RunConfig, brisk_run::Command, u64)> {
        self.tabs.get(self.active).and_then(|t| t.run.as_ref().map(|r| (r.cfg.clone(), r.cmd.clone(), t.id)))
    }

    /// "Copy command line" (Run toolbar): the active run's command, as its header shows it.
    pub fn copy_command_line(&self, cx: &mut Context<Self>) {
        if let Some((_, cmd, _)) = self.active_run() {
            cx.write_to_clipboard(gpui::ClipboardItem::new_string(cmd.display()));
        }
    }

    /// "Open in Terminal" (Run toolbar): asks the workspace to run the command in the terminal.
    pub fn open_in_terminal(&self, cx: &mut Context<Self>) {
        if let Some((_, cmd, _)) = self.active_run() {
            cx.emit(RunEvent::OpenInTerminal(cmd));
        }
    }

    /// Pin Tab / Unpin Tab on the active Run tab; returns whether it is pinned now.
    pub fn toggle_pin(&mut self) -> bool {
        match self.tabs.get_mut(self.active).and_then(|t| t.run.as_mut()) {
            Some(r) => {
                r.pinned = !r.pinned;
                r.pinned
            }
            None => false,
        }
    }

    fn active_pinned(&self) -> bool {
        self.tabs.get(self.active).and_then(|t| t.run.as_ref()).is_some_and(|r| r.pinned)
    }

    /// Run tabs as (name, pinned) and the lines each keeps above its screen (smoke tests).
    pub fn run_tabs_detail(&self) -> Vec<(String, bool, usize)> {
        self.tabs.iter().filter_map(|t| t.run.as_ref().map(|r| (r.name.clone(), r.pinned, t.session.history_lines()))).collect()
    }

    /// Stop every running process (smoke tests).
    pub fn stop_all(&mut self) {
        for t in self.tabs.iter_mut().filter(|t| t.run.as_ref().is_some_and(|r| r.running)) {
            t.stop_run();
        }
    }

    /// Ctrl+F2: stop the active tab's process (or any running one).
    pub fn stop(&mut self) -> bool {
        let running = |t: &Tab| t.run.as_ref().is_some_and(|r| r.running);
        let pick = self.tabs.get(self.active).filter(|t| running(t)).map(|_| self.active).or_else(|| self.tabs.iter().position(running));
        match pick {
            Some(i) => {
                self.tabs[i].stop_run();
                true
            }
            None => false,
        }
    }

    pub fn is_running(&self) -> bool {
        self.tabs.iter().any(|t| t.run.as_ref().is_some_and(|r| r.running))
    }

    /// The environment for shells opened from now on.
    pub fn set_activation(&mut self, a: Option<crate::terminal_env::Activation>) {
        self.activation = a;
    }

    /// Type a line into the active shell and press Enter (smoke tests).
    pub fn type_line(&self, line: &str) {
        if let Some(s) = self.session() {
            s.write(format!("{line}\r").into_bytes());
        }
    }

    /// Text of the active tab (smoke tests).
    pub fn active_text(&self) -> Option<String> {
        self.session().map(|s| s.text())
    }

    /// Resolve a console link against the run's working directory.
    fn link_target(&self, path: &str, line: usize) -> Option<(PathBuf, usize)> {
        let cwd = self.tabs.get(self.active).and_then(|t| t.run.as_ref()).map(|r| r.cmd.cwd.clone()).unwrap_or_else(|| self.root.clone());
        let p = PathBuf::from(path);
        let p = if p.is_absolute() { p } else { cwd.join(p) };
        p.is_file().then_some((p, line))
    }

    /// First link in the active tab that points to an existing file (smoke tests).
    pub fn first_link(&self) -> Option<(PathBuf, usize)> {
        let text = self.active_text()?;
        text.lines().flat_map(brisk_run::find_links).find_map(|l| self.link_target(&l.path, l.line))
    }

    /// First `http(s)://` link in the active tab (smoke tests).
    pub fn first_url(&self) -> Option<String> {
        self.active_text()?.lines().flat_map(brisk_run::find_urls).map(|(_, u)| u).next()
    }

    /// Open the first `http(s)://` link as a click would (smoke tests).
    pub fn click_first_url(&mut self, cx: &mut Context<Self>) -> bool {
        let Some(u) = self.first_url() else { return false };
        cx.emit(RunEvent::OpenUrl(u));
        true
    }

    /// The `http(s)://` link under a grid cell, if any.
    fn url_at(&self, row: usize, col: usize) -> Option<String> {
        let snap = self.session()?.snapshot();
        let cells = snap.cells.get(row * snap.cols..(row + 1) * snap.cols)?;
        let (text, idx) = row_text(cells);
        let ch = idx.iter().position(|&c| c >= col)?;
        brisk_run::find_urls(&text).into_iter().find(|(r, _)| r.contains(&ch)).map(|(_, u)| u)
    }

    /// The link under a grid cell, if any.
    fn link_at(&self, row: usize, col: usize) -> Option<(PathBuf, usize, usize)> {
        let snap = self.session()?.snapshot();
        let cells = snap.cells.get(row * snap.cols..(row + 1) * snap.cols)?;
        let (text, idx) = row_text(cells);
        let ch = idx.iter().position(|&c| c >= col)?;
        let l = brisk_run::find_links(&text).into_iter().find(|l| l.cols.contains(&ch))?;
        self.link_target(&l.path, l.line).map(|(p, line)| (p, line, l.col))
    }

    /// What an OSC 8 hyperlink opens: a `file://` URI (with `#L12` or `:12` for a line) in
    /// the editor, anything else in the browser.
    fn hyperlink_event(uri: &str) -> RunEvent {
        if let Some(rest) = uri.strip_prefix("file://") {
            // file:///C:/x.py, file://host/path: the host part is ignored.
            let path = rest.find('/').map(|i| &rest[i..]).unwrap_or(rest);
            let (path, line) = match path.rsplit_once("#L").or_else(|| path.rsplit_once('#')) {
                Some((p, l)) if l.parse::<usize>().is_ok() => (p, l.parse().unwrap_or(1)),
                _ => (path, 1),
            };
            let path = percent_decode(path);
            let path = if cfg!(windows) && path.as_bytes().get(2) == Some(&b':') { path[1..].to_string() } else { path };
            return RunEvent::Open(PathBuf::from(path), line);
        }
        RunEvent::OpenUrl(uri.to_string())
    }

    /// Foreground color of the first on-screen cell of `needle` (smoke tests).
    pub fn fg_of(&self, needle: &str) -> Option<[u8; 3]> {
        let snap = self.session()?.snapshot();
        for r in 0..snap.rows {
            let row = &snap.cells[r * snap.cols..(r + 1) * snap.cols];
            let (text, idx) = row_text(row);
            if let Some(b) = text.find(needle) {
                return Some(row[idx[text[..b].chars().count()]].fg);
            }
        }
        None
    }

    /// OSC 8 hyperlinks in the active tab's screen: their URIs (smoke tests).
    pub fn hyperlinks(&self) -> Vec<String> {
        let Some(s) = self.session() else { return Vec::new() };
        let snap = s.snapshot();
        let mut v: Vec<String> = Vec::new();
        for (i, c) in snap.cells.iter().enumerate() {
            if c.link {
                if let Some(u) = s.hyperlink_at(i / snap.cols, i % snap.cols) {
                    if !v.contains(&u) {
                        v.push(u);
                    }
                }
            }
        }
        v
    }

    /// Click the first OSC 8 hyperlink whose URI starts with `osc8` (or, with `None`, the
    /// first file link) on screen as the mouse would (smoke tests).
    pub fn click_first(&mut self, osc8: Option<&str>, cx: &mut Context<Self>) -> bool {
        let Some(s) = self.session() else { return false };
        let snap = s.snapshot();
        for r in 0..snap.rows {
            for c in 0..snap.cols {
                if let Some(prefix) = osc8 {
                    if let Some(u) = snap.cells[r * snap.cols + c].link.then(|| s.hyperlink_at(r, c)).flatten().filter(|u| u.starts_with(prefix)) {
                        cx.emit(Self::hyperlink_event(&u));
                        return true;
                    }
                } else {
                    if let Some((p, l, col)) = self.link_at(r, c) {
                        cx.emit(if col > 0 { RunEvent::OpenAt(p, l, col) } else { RunEvent::Open(p, l) });
                        return true;
                    }
                }
            }
        }
        false
    }

    pub fn is_empty(&self) -> bool {
        self.tabs.is_empty()
    }

    pub fn new_tab(&mut self) {
        let id = self.next_id;
        self.next_id += 1;
        let tx = self.events.clone();
        let (cols, rows) = self.tabs.get(self.active).map(|t| t.session.size()).unwrap_or((120, 12));
        let mut opts = SpawnOptions::for_project(&self.root);
        if let Some(a) = &self.activation {
            // The project interpreter decides, not whatever .venv sits in the root.
            opts.env.remove("PATH");
            opts.env.remove("VIRTUAL_ENV");
            opts.env.extend(a.env.clone());
            if let (true, Some(p)) = (cfg!(windows), &a.prompt) {
                opts.shell = Some(("powershell.exe".into(), crate::terminal_env::powershell_args(p)));
            }
        }
        match Session::spawn(opts, cols, rows, 8, CELL_H as u16, move |e| {
            let _ = tx.unbounded_send((id, e));
        }) {
            Ok(session) => {
                let number = (1..).find(|n| !self.tabs.iter().any(|t| t.number == *n)).unwrap();
                self.tabs.push(Tab { id, session, title: String::new(), number, run: None });
                self.active = self.tabs.len() - 1;
                self.error = None;
            }
            Err(e) => self.error = Some(format!("Cannot start shell: {e}")),
        }
    }

    fn close_tab_id(&mut self, id: u64) {
        if let Some(i) = self.tabs.iter().position(|t| t.id == id) {
            if self.tabs[i].run.is_some() {
                self.tabs[i].session.kill();
            }
            self.tabs.remove(i);
            if self.active >= self.tabs.len() {
                self.active = self.tabs.len().saturating_sub(1);
            }
        }
    }

    pub fn close_active(&mut self) {
        if let Some(t) = self.tabs.get(self.active) {
            let id = t.id;
            self.close_tab_id(id);
        }
    }

    fn session(&self) -> Option<&Session> {
        self.tabs.get(self.active).map(|t| &t.session)
    }

    pub fn alt_screen(&self) -> bool {
        self.session().is_some_and(|s| s.alt_screen())
    }

    /// Handle a key while the terminal is focused. Returns false if the key should go to the IDE.
    pub fn key(&mut self, k: &gpui::Keystroke, cx: &mut Context<Self>) -> bool {
        let m = k.modifiers;
        let key = k.key.to_ascii_lowercase();
        if self.run_mode && m.control && !m.shift && !m.alt && key == "f" {
            self.open_search();
            cx.notify();
            return true;
        }
        if self.search.open && self.search_key(k, cx) {
            return true;
        }
        // Tab management, PyCharm terminal shortcuts.
        match (m.control, m.shift, m.alt, key.as_str()) {
            (true, true, false, "t") if !self.run_mode => {
                self.new_tab();
                cx.notify();
                return true;
            }
            (true, false, false, "f4") => {
                self.close_active();
                cx.notify();
                return true;
            }
            (false, false, true, "right") | (false, false, true, "left") if self.tabs.len() > 1 => {
                let n = self.tabs.len();
                self.active = if key == "right" { (self.active + 1) % n } else { (self.active + n - 1) % n };
                cx.notify();
                return true;
            }
            _ => {}
        }
        if let Some(handled) = self.tests_key(&key, m, cx) {
            return handled;
        }
        let Some(s) = self.session() else { return false };
        // Copy / paste: Ctrl+C copies when there is a selection, otherwise it's SIGINT.
        let copy = (m.control && m.shift && key == "c") || (m.control && !m.shift && key == "c" && s.selection_text().is_some()) || (m.control && key == "insert");
        if copy {
            if let Some(t) = s.selection_text() {
                cx.write_to_clipboard(ClipboardItem::new_string(t));
                s.clear_selection();
                cx.notify();
            }
            return true;
        }
        if (m.control && key == "v") || (m.shift && key == "insert") {
            if let Some(t) = cx.read_from_clipboard().and_then(|c| c.text()) {
                s.paste(&t);
            }
            return true;
        }
        let mods = KeyMods { ctrl: m.control, alt: m.alt, shift: m.shift };
        match encode_key(&key, k.key_char.as_deref(), mods, s.app_cursor()) {
            Some(bytes) => {
                s.clear_selection();
                s.write(bytes);
                true
            }
            None => false,
        }
    }

    fn cell_at(&self, p: Point<Pixels>) -> Option<(usize, usize, bool)> {
        let g = self.grid?;
        let x = f32::from(p.x - g.bounds.left()).max(0.0);
        let y = f32::from(p.y - g.bounds.top()).max(0.0);
        let cw = f32::from(g.cell_w);
        let col = (x / cw) as usize + self.h_scroll;
        let right = (x / cw).fract() > 0.5;
        Some(((y / CELL_H) as usize, col, right))
    }

    fn on_mouse_down(&mut self, e: &MouseDownEvent, window: &mut Window, cx: &mut Context<Self>) {
        window.focus(&self.focus);
        self.search.focused = false;
        let Some((row, col, _)) = self.cell_at(e.position) else { return };
        if self.run_mode && e.click_count == 1 {
            if let Some(uri) = self.session().and_then(|s| s.hyperlink_at(row, col)) {
                cx.emit(Self::hyperlink_event(&uri));
                return;
            }
            if let Some((path, line, c)) = self.link_at(row, col) {
                cx.emit(if c > 0 { RunEvent::OpenAt(path, line, c) } else { RunEvent::Open(path, line) });
                return;
            }
            if let Some(url) = self.url_at(row, col) {
                cx.emit(RunEvent::OpenUrl(url));
                return;
            }
        }
        let Some(s) = self.session() else { return };
        let kind = match e.click_count {
            2 => SelectKind::Word,
            n if n >= 3 => SelectKind::Line,
            _ => SelectKind::Simple,
        };
        s.start_selection(row, col, kind);
        if kind != SelectKind::Simple {
            s.update_selection(row, col, true);
        }
        self.selecting = true;
        cx.notify();
    }

    fn on_mouse_move(&mut self, e: &MouseMoveEvent, _: &mut Window, cx: &mut Context<Self>) {
        if !self.selecting || e.pressed_button != Some(MouseButton::Left) {
            return;
        }
        if let (Some((row, col, right)), Some(s)) = (self.cell_at(e.position), self.session()) {
            s.update_selection(row, col, right);
            cx.notify();
        }
    }

    fn on_scroll(&mut self, e: &ScrollWheelEvent, _: &mut Window, cx: &mut Context<Self>) {
        let Some(s) = self.session() else { return };
        let d = e.delta.pixel_delta(px(CELL_H));
        if self.run_mode && !self.soft_wrap {
            let cw = self.grid.map(|g| f32::from(g.cell_w)).unwrap_or(8.0);
            let dx = if e.modifiers.shift { f32::from(d.y) } else { f32::from(d.x) };
            let cols = (dx / cw).round() as i64;
            if cols != 0 {
                self.h_scroll = (self.h_scroll as i64 - cols * 3).clamp(0, NOWRAP_COLS as i64) as usize;
                cx.notify();
                return;
            }
        }
        let lines = (f32::from(d.y) / CELL_H).round() as i32;
        if lines == 0 {
            return;
        }
        // Scrolling up stops following the output, like PyCharm's Scroll to End.
        let unfollow = self.run_mode && lines > 0;
        if s.alt_screen() {
            // Full-screen apps (less, vim, htop): wheel sends arrow keys.
            let seq: &[u8] = if lines > 0 { b"\x1bOA" } else { b"\x1bOB" };
            s.write(seq.repeat(lines.unsigned_abs() as usize));
        } else {
            s.scroll(lines);
        }
        if unfollow {
            self.scroll_to_end = false;
        } else if self.run_mode && lines < 0 && self.session().is_some_and(|s| s.snapshot().display_offset == 0) {
            // Back at the bottom: follow the output again.
            self.scroll_to_end = true;
        }
        cx.notify();
    }

    fn render_tabs(&self, cx: &mut Context<Self>) -> impl IntoElement {
        crate::ui_kit::tool_header()
            .gap(px(2.))
            .child(crate::ui_kit::tool_title(if self.run_mode { "Run" } else { "Terminal" }))
            .children(self.tabs.iter().enumerate().map(|(i, t)| {
                let active = i == self.active;
                let name = match &t.run {
                    Some(r) => r.name.clone(),
                    None if t.number == 1 => "Local".to_string(),
                    None => format!("Local ({})", t.number),
                };
                let state = t.run.as_ref().map(|r| match (r.running, r.code) {
                    (true, _) => ("play", Theme::run_icon()),
                    (false, Some(0)) => ("check", Theme::text_muted()),
                    (false, None) => ("stop-solid", Theme::text_muted()),
                    (false, _) => ("x", Theme::error()),
                });
                let id = t.id;
                div()
                    .id(("term-tab", t.id as usize))
                    .flex()
                    .items_center()
                    .gap(px(6.))
                    .px(px(8.))
                    .py(px(3.))
                    .rounded(px(5.))
                    .text_color(if active { Theme::text() } else { Theme::text_muted() })
                    .when(active, |d| d.bg(Theme::selection_row()))
                    .when(!active, |d| d.hover(|s| s.bg(Theme::hover_row())))
                    .cursor_pointer()
                    .on_click(cx.listener(move |this, _, window, cx| {
                        this.active = i;
                        window.focus(&this.focus);
                        cx.notify();
                    }))
                    .when_some(state, |d, (g, c)| d.child(crate::icons::icon(g, 12., c)))
                    .when(t.run.as_ref().is_some_and(|r| r.pinned), |d| d.child(crate::icons::icon("pin", 12., Theme::accent())))
                    .child(name)
                    .child(
                        div().id(("term-close", t.id as usize)).group("close-x").child(crate::icons::close_x("close-x", 12.)).on_click(
                            cx.listener(move |this, _, _, cx| {
                                cx.stop_propagation();
                                this.close_tab_id(id);
                                cx.notify();
                            }),
                        ),
                    )
            }))
            .when(!self.run_mode, |d| d.child(
                div()
                    .id("term-new")
                    .px(px(8.))
                    .py(px(2.))
                    .rounded(px(5.))
                    .text_color(Theme::text_muted())
                    .hover(|s| s.bg(Theme::hover_row()))
                    .cursor_pointer()
                    .child("+")
                    .on_click(cx.listener(|this, _, window, cx| {
                        this.new_tab();
                        window.focus(&this.focus);
                        cx.notify();
                    })),
            ))
            .when_some(self.tabs.get(self.active).filter(|t| !t.title.is_empty()), |d, t| {
                d.child(div().flex_1()).child(div().text_color(Theme::text_dim()).text_size(px(12.)).pr(px(8.)).child(t.title.clone()))
            })
    }
}

impl TerminalPanel {
    fn render_run_toolbar(&self, cx: &mut Context<Self>) -> impl IntoElement {
        let running = self.tabs.get(self.active).and_then(|t| t.run.as_ref()).is_some_and(|r| r.running);
        let button = |id: &'static str, glyph: &'static str, color: Hsla| {
            div()
                .id(id)
                .w(px(26.))
                .h(px(26.))
                .flex()
                .items_center()
                .justify_center()
                .rounded(px(5.))
                .hover(|s| s.bg(Theme::hover_row()))
                .active(|s| s.bg(Theme::press()))
                .cursor_pointer()
                .child(crate::icons::icon(glyph, 16., color))
        };
        let tip = |t: &'static str| move |_: &mut Window, cx: &mut App| cx.new(|_| crate::workspace::Tip(t.into())).into();
        let toggle = |id: &'static str, glyph: &'static str, on: bool| {
            div()
                .id(id)
                .relative()
                .w(px(26.))
                .h(px(26.))
                .flex()
                .items_center()
                .justify_center()
                .rounded(px(5.))
                .when(on, |d| d.bg(Theme::selection_row()))
                .when(!on, |d| d.hover(|s| s.bg(Theme::hover_row())))
                .active(|s| s.bg(Theme::press()))
                .cursor_pointer()
                .child(crate::icons::icon(glyph, 16., if on { Theme::text() } else { Theme::text_muted() }))
        };
        div()
            .w(px(34.))
            .h_full()
            .flex()
            .flex_col()
            .items_center()
            .gap(px(2.))
            .border_r_1()
            .border_color(Theme::border())
            .child(button("run-rerun", "rotate-cw", Theme::run_icon()).tooltip(tip("Rerun  Ctrl+F5")).on_click(cx.listener(|_, _, _, cx| cx.emit(RunEvent::RerunRequested))))
            .when(self.tests().is_some(), |d| {
                let any_failed = self.tests().is_some_and(|t| t.finished && t.tree().tree.counts().failed > 0);
                d.child(
                    button("run-rerun-failed", "rerun-failed", if any_failed { Theme::error() } else { Theme::text_dim().opacity(0.5) })
                        .on_click(cx.listener(|_, _, _, cx| cx.emit(RunEvent::RerunFailedRequested))),
                )
            })
            .child(
                button("run-stop", "stop-solid", if running { Theme::error() } else { Theme::text_dim().opacity(0.5) }).tooltip(tip("Stop  Ctrl+F2")).on_click(cx.listener(|this, _, _, cx| {
                    this.stop();
                    cx.notify();
                })),
            )
            .child(div().w(px(18.)).h(px(1.)).my(px(3.)).bg(Theme::divider()))
            .child(button("run-copy-cmd", "copy", Theme::text_muted()).tooltip(tip("Copy command line")).on_click(cx.listener(|this, _, _, cx| this.copy_command_line(cx))))
            .child(button("run-open-term", "square-terminal", Theme::text_muted()).tooltip(tip("Open in Terminal")).on_click(cx.listener(|this, _, _, cx| this.open_in_terminal(cx))))
            .child(toggle("run-pin", "pin", self.active_pinned()).tooltip(tip("Pin Tab: the next run opens a new tab")).child(pin_track()).on_click(cx.listener(|this, _, _, cx| {
                this.toggle_pin();
                cx.notify();
            })))
            .child(toggle("run-wrap", "wrap-text", self.soft_wrap).tooltip(tip("Soft-Wrap")).on_click(cx.listener(|this, _, _, cx| {
                this.toggle_soft_wrap();
                cx.notify();
            })))
            .child(toggle("run-follow", "arrow-down-to-line", self.scroll_to_end).tooltip(tip("Scroll to End")).on_click(cx.listener(|this, _, _, cx| {
                this.toggle_scroll_to_end();
                cx.notify();
            })))
            .child(button("run-clear", "trash", Theme::text_muted()).tooltip(tip("Clear All")).on_click(cx.listener(|this, _, _, cx| {
                this.clear();
                cx.notify();
            })))
    }
}

impl Focusable for TerminalPanel {
    fn focus_handle(&self, _: &App) -> FocusHandle {
        self.focus.clone()
    }
}

impl Render for TerminalPanel {
    fn render(&mut self, _: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        PANEL_RENDERS.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
        if self.run_mode {
            self.sync_console_prefs();
        }
        div()
            .id("terminal")
            .key_context("Terminal")
            .track_focus(&self.focus)
            .size_full()
            .flex()
            .flex_col()
            .child(self.render_tabs(cx))
            .when(self.run_mode && self.tabs.is_empty(), |d| {
                d.child(
                    div()
                        .flex_1()
                        .flex()
                        .items_center()
                        .justify_center()
                        .text_color(Theme::text_muted())
                        .text_size(px(13.))
                        .child("Nothing is running. Run a file with Ctrl+Shift+F10, the selected configuration with Shift+F10."),
                )
            })
            .when(!(self.run_mode && self.tabs.is_empty()), |d| d.child(
                div().flex_1().min_h_0().flex().when(self.run_mode, |d| d.child(self.render_run_toolbar(cx)))
                .when_some(self.tests(), |d, t| d.child(self.render_tests_tree(t, cx)))
                .when_some(self.tests().filter(|t| t.shows_details()), |d, t| d.child(self.render_test_details(t, cx)))
                .when(!self.tests().is_some_and(|t| t.shows_details()), |d| d.child(
                div().flex_1().min_w_0().h_full().flex().flex_col()
                .when(self.search.open, |d| d.child(self.render_search_bar(cx)))
                .when(self.search.open && self.search.filter, |d| d.child(self.render_filtered(cx)))
                .when(!(self.search.open && self.search.filter), |d| d.child(
                div().relative().flex_1().min_h_0().flex().child(
                div()
                    .id("terminal-grid")
                    .flex_1()
                    .min_w_0()
                    .min_h_0()
                    .ml(px(8.))
                    .mr(px(crate::scrollbar::BAR))
                    .mb(px(4.))
                    .cursor(gpui::CursorStyle::IBeam)
                    .on_mouse_down(MouseButton::Left, cx.listener(Self::on_mouse_down))
                    .on_mouse_move(cx.listener(Self::on_mouse_move))
                    .on_mouse_up(MouseButton::Left, cx.listener(|this, _, _, _| this.selecting = false))
                    .on_scroll_wheel(cx.listener(Self::on_scroll))
                    .when_some(self.error.clone(), |d, e| d.child(div().text_color(rgb(0xf05d5d)).child(e)))
                    .child(TerminalElement { panel: cx.entity() }),
                ).child(self.scrollbar(cx)),
            ))))))
    }
}

// --- Grid element --------------------------------------------------------------

struct TerminalElement {
    panel: Entity<TerminalPanel>,
}

struct Prepaint {
    rects: Vec<gpui::PaintQuad>,
    texts: Vec<(Point<Pixels>, ShapedLine)>,
    cursor: Option<Bounds<Pixels>>,
}

impl IntoElement for TerminalElement {
    type Element = Self;
    fn into_element(self) -> Self {
        self
    }
}

impl Element for TerminalElement {
    type RequestLayoutState = ();
    type PrepaintState = Prepaint;

    fn id(&self) -> Option<ElementId> {
        None
    }
    fn source_location(&self) -> Option<&'static core::panic::Location<'static>> {
        None
    }

    fn request_layout(&mut self, _: Option<&GlobalElementId>, _: Option<&gpui::InspectorElementId>, window: &mut Window, cx: &mut App) -> (LayoutId, ()) {
        let mut style = Style::default();
        style.size.width = gpui::relative(1.).into();
        style.size.height = gpui::relative(1.).into();
        (window.request_layout(style, [], cx), ())
    }

    fn prepaint(&mut self, _: Option<&GlobalElementId>, _: Option<&gpui::InspectorElementId>, bounds: Bounds<Pixels>, _: &mut (), window: &mut Window, cx: &mut App) -> Prepaint {
        let fs = px(FONT_SIZE);
        let mono = font(mono_font());
        let m = window.text_system().shape_line("M".into(), fs, &[TextRun { len: 1, font: mono.clone(), color: Theme::text(), background_color: None, underline: None, strikethrough: None }], None);
        let cell_w = m.width;
        let fit = (f32::from(bounds.size.width) / f32::from(cell_w)).floor().max(2.0) as usize;
        let rows = (f32::from(bounds.size.height) / CELL_H).floor().max(1.0) as usize;
        let mut h_off = 0;

        let mut marks: Vec<(i32, std::ops::Range<usize>, bool)> = Vec::new();
        let snap: Option<(Snapshot, bool)> = self.panel.update(cx, |p, _| {

            p.grid = Some(GridGeom { bounds, cell_w });
            let run_mode = p.run_mode;
            let nowrap = run_mode && !p.soft_wrap;
            let cols = if nowrap { NOWRAP_COLS.max(fit) } else { fit };
            p.h_scroll = if nowrap { p.h_scroll.min(cols.saturating_sub(fit)) } else { 0 };
            h_off = p.h_scroll;
            let tab = p.tabs.get_mut(p.active)?;
            tab.session.resize(cols, rows, f32::from(cell_w) as u16, CELL_H as u16);
            // After the resize: it re-wraps rows and moves lines into the history.
            p.refresh_search();
            if p.search.open {
                let current = p.search.current;
                marks = p.search.found.iter().enumerate().flat_map(|(i, f)| f.parts().map(move |(l, c)| (l, c, i == current))).collect();
            }
            let tab = p.tabs.get(p.active)?;
            Some((tab.session.snapshot(), run_mode))
        });
        let Some((mut snap, run_mode)) = snap else { return Prepaint { rects: vec![], texts: vec![], cursor: None } };
        if run_mode {
            // Traceback and `file.py:12` links: blue and underlined, like PyCharm.
            for r in 0..snap.rows {
                let row = &mut snap.cells[r * snap.cols..(r + 1) * snap.cols];
                for cell in row.iter_mut().filter(|c| c.link) {
                    cell.underline = true;
                }
                let (text, idx) = row_text(row);
                let urls = brisk_run::find_urls(&text).into_iter().map(|(r, _)| r);
                for cols in brisk_run::find_links(&text).into_iter().map(|l| l.cols).chain(urls) {
                    for ch in cols {
                        if let Some(&c) = idx.get(ch) {
                            row[c].fg = LINK;
                            row[c].underline = true;
                        }
                    }
                }
            }
        }

        let mut rects = Vec::new();
        let mut texts = Vec::new();
        let x_at = |c: usize| bounds.left() + cell_w * (c as f32 - h_off as f32);
        // Find matches on screen, under the text.
        for (line, cols, current) in marks {
            let r = line + snap.display_offset as i32;
            if r >= 0 && (r as usize) < snap.rows {
                let y = bounds.top() + px(r as f32 * CELL_H);
                let color = if current { Theme::search_current() } else { Theme::search_hit() };
                rects.push(fill(Bounds::from_corners(point(x_at(cols.start), y), point(x_at(cols.end), y + px(CELL_H))), color));
            }
        }
        for r in 0..snap.rows {
            let y = bounds.top() + px(r as f32 * CELL_H);
            let row = &snap.cells[r * snap.cols..(r + 1) * snap.cols];
            // Backgrounds and selection, merged into horizontal runs.
            let mut c = 0;
            while c < row.len() {
                let bg = if row[c].selected { Some(Theme::selection()) } else { row[c].bg.map(hsla) };
                let start = c;
                while c < row.len() && (if row[c].selected { Some(Theme::selection()) } else { row[c].bg.map(hsla) }) == bg {
                    c += 1;
                }
                if let Some(bg) = bg {
                    rects.push(fill(Bounds::from_corners(point(x_at(start), y), point(x_at(c), y + px(CELL_H))), bg));
                }
            }
            // Text runs: same style, painted at their cell column so the grid never drifts.
            let mut c = 0;
            while c < row.len() {
                if row[c].c == ' ' || row[c].c == '\0' {
                    c += 1;
                    continue;
                }
                let s0 = row[c];
                let start = c;
                let mut text = String::new();
                while c < row.len() {
                    let cell = row[c];
                    if cell.c == '\0' {
                        c += 1;
                        continue;
                    }
                    if cell.fg != s0.fg || cell.bold != s0.bold || cell.italic != s0.italic || cell.underline != s0.underline || cell.wide || (s0.wide && c > start) {
                        break;
                    }
                    text.push(cell.c);
                    c += 1;
                }
                if text.is_empty() {
                    // A wide char starts a run of its own.
                    text.push(row[c].c);
                    c += 1;
                }
                let trimmed = text.trim_end();
                if trimmed.is_empty() {
                    continue;
                }
                let mut f = mono.clone();
                if s0.bold {
                    f.weight = FontWeight::BOLD;
                }
                if s0.italic {
                    f.style = FontStyle::Italic;
                }
                let color = hsla(s0.fg);
                let run = TextRun {
                    len: trimmed.len(),
                    font: f,
                    color,
                    background_color: None,
                    underline: s0.underline.then(|| UnderlineStyle { color: Some(color), thickness: px(1.), wavy: false }),
                    strikethrough: None,
                };
                let shaped = window.text_system().shape_line(SharedString::from(trimmed.to_string()), fs, &[run], None);
                texts.push((point(x_at(start), y), shaped));
            }
        }
        let cursor = snap.cursor.map(|(r, c)| Bounds::new(point(x_at(c), bounds.top() + px(r as f32 * CELL_H)), size(cell_w, px(CELL_H))));
        Prepaint { rects, texts, cursor }
    }

    fn paint(&mut self, _: Option<&GlobalElementId>, _: Option<&gpui::InspectorElementId>, bounds: Bounds<Pixels>, _: &mut (), pp: &mut Prepaint, window: &mut Window, cx: &mut App) {
        let focused = self.panel.read(cx).focus.is_focused(window);
        window.with_content_mask(Some(ContentMask { bounds }), |window| {
            for r in pp.rects.drain(..) {
                window.paint_quad(r);
            }
            for (p, line) in pp.texts.drain(..) {
                let _ = line.paint(p, px(CELL_H), window, cx);
            }
            // Block cursor on top, translucent so the character under it stays readable.
            if let Some(cur) = pp.cursor {
                if focused {
                    window.paint_quad(fill(cur, Theme::caret().opacity(0.55)));
                } else {
                    window.paint_quad(gpui::outline(cur, Theme::text_muted(), gpui::BorderStyle::Solid));
                }
            }
        });
    }
}

thread_local! {
    /// Where the Pin Tab button was painted: the smoke clicks it for real.
    static PIN_BUTTON: std::cell::Cell<Option<gpui::Bounds<gpui::Pixels>>> = const { std::cell::Cell::new(None) };
}

fn pin_track() -> impl IntoElement {
    gpui::canvas(|b, _, _| PIN_BUTTON.with(|c| c.set(Some(b))), |_, _, _, _| {}).absolute().top_0().left_0().size_full()
}

/// Center of the Pin Tab button in window pixels (smoke tests).
pub fn pin_button_center() -> Option<(f32, f32)> {
    PIN_BUTTON.with(|c| c.get()).map(|b| (f32::from(b.center().x), f32::from(b.center().y)))
}

/// `%20` and friends in a `file://` URI.
fn percent_decode(s: &str) -> String {
    let b = s.as_bytes();
    let mut out = Vec::with_capacity(b.len());
    let mut i = 0;
    while i < b.len() {
        if b[i] == b'%' && i + 2 < b.len() {
            if let Ok(v) = u8::from_str_radix(&s[i + 1..i + 3], 16) {
                out.push(v);
                i += 3;
                continue;
            }
        }
        out.push(b[i]);
        i += 1;
    }
    String::from_utf8_lossy(&out).into_owned()
}
