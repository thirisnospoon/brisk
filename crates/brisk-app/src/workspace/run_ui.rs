//! Run: interpreter selection, run configurations (toolbar widget, Alt+Shift+F10), the Run
//! tool window (Alt+4) and PyCharm's Shift+F10 / Ctrl+Shift+F10 / Ctrl+F5 / Ctrl+F2.

use super::{BottomTool, PopupItem, PopupKind, Target, Workspace};
use super::before_launch_ui;
use crate::terminal_view::{RunEvent, TerminalPanel};
use brisk_keymap::Action;
use brisk_run::{InterpKind, Interpreter, RunConfig, RunFile, RunKind};
use gpui::prelude::*;
use gpui::{Context, Entity, Focusable, Window};
use std::path::{Path, PathBuf};

pub(super) struct Runs {
    pub file: RunFile,
    /// Project virtualenvs, known instantly from `pyvenv.cfg`.
    pub venvs: Vec<Interpreter>,
    /// Everything found on the machine; `None` while discovery runs in the background.
    pub all: Option<Vec<Interpreter>>,
    pub panel: Option<Entity<TerminalPanel>>,
    /// Edit Configurations dialog, when open.
    pub dialog: Option<super::run_config_ui::RunDialog>,
}

impl Runs {
    /// Returns the state and how many configurations were imported from `.idea`.
    pub fn new(root: &Path) -> (Self, usize) {
        let (file, imported) = brisk_run::load(root);
        if imported > 0 {
            let _ = brisk_run::save(root, &file);
        }
        (Runs { file, venvs: brisk_run::project_venvs(root), all: None, panel: None, dialog: None }, imported)
    }

    fn known(&self) -> impl Iterator<Item = &Interpreter> {
        self.venvs.iter().chain(self.all.iter().flatten())
    }

    pub(super) fn lookup(&self, path: &Path) -> Interpreter {
        self.known().find(|i| i.path == path).cloned().unwrap_or_else(|| Interpreter {
            path: path.to_path_buf(),
            version: String::new(),
            kind: if Interpreter::venv_dir_of(path).is_some() { InterpKind::Venv } else { InterpKind::System },
        })
    }

    /// The project interpreter: the one chosen in the status bar, else the project venv,
    /// else the first interpreter found on the machine.
    pub fn interpreter(&self, root: &Path) -> Option<Interpreter> {
        match &self.file.interpreter {
            Some(p) => Some(self.lookup(&brisk_run::resolve(root, p))),
            None => self.venvs.first().or_else(|| self.all.as_ref().and_then(|a| a.first())).cloned(),
        }
    }
}

impl Workspace {
    pub(super) fn start_interpreter_discovery(&mut self, cx: &mut Context<Self>) {
        let root = self.root.clone();
        let task = cx.background_executor().spawn(async move { brisk_run::discover_all(&root) });
        cx.spawn(async move |this, cx| {
            let all = task.await;
            let _ = this.update(cx, |this, cx| {
                this.runs.all = Some(all);
                this.relint_all(cx);
                if this.bottom == Some(BottomTool::Problems) && this.problems.tab == super::problems_ui::ProblemsTab::Project {
                    this.check_project(cx);
                }
                cx.notify();
            });
        })
        .detach();
    }

    pub(super) fn interpreter_label(&self) -> String {
        match self.runs.interpreter(&self.root) {
            Some(i) => i.label(&self.root),
            None if self.runs.all.is_none() => "Detecting interpreter…".into(),
            None => "<No interpreter>".into(),
        }
    }

    pub(super) fn run_widget_label(&self) -> String {
        self.runs.file.selected_config().map(|c| c.name.clone()).unwrap_or_else(|| "Current File".into())
    }

    pub(super) fn reload_runs(&mut self) {
        if let Ok(s) = std::fs::read_to_string(self.root.join(brisk_run::CONFIG_FILE)) {
            match serde_json::from_str::<RunFile>(&s) {
                Ok(f) => self.runs.file = f,
                Err(e) => self.status = format!("{}: {e}", brisk_run::CONFIG_FILE),
            }
        }
    }

    pub(super) fn save_runs(&mut self) {
        if let Err(e) = brisk_run::save(&self.root, &self.runs.file) {
            self.status = format!("Cannot save {}: {e}", brisk_run::CONFIG_FILE);
        }
    }

    pub(super) fn run_panel_focused(&self, window: &Window, cx: &gpui::App) -> bool {
        self.runs.panel.as_ref().is_some_and(|t| t.read(cx).focus_handle(cx).is_focused(window))
    }

    fn ensure_run_panel(&mut self, window: &mut Window, cx: &mut Context<Self>) -> Entity<TerminalPanel> {
        if let Some(p) = &self.runs.panel {
            return p.clone();
        }
        let root = self.root.clone();
        let p = cx.new(|cx| TerminalPanel::new_run(root, cx));
        cx.subscribe_in(&p, window, |this: &mut Self, panel, ev: &RunEvent, window, cx| match ev {
            RunEvent::Open(path, line) => this.open_at(&path.clone(), *line, window, cx),
            RunEvent::OpenUrl(url) => {
                // A server bound to all interfaces is reached through localhost.
                let url = url.replacen("://0.0.0.0", "://127.0.0.1", 1);
                this.status = format!("Opening {url}");
                if std::env::var_os("BRISK_SMOKE_WEB").is_none() {
                    cx.open_url(&url);
                }
                this.last_url = Some(url);
            }
            RunEvent::Finished { name, code, tests } => {
                if let Some(t) = tests {
                    this.take_test_results(&t.results, cx);
                }
                this.coverage_finished(name, cx);
                this.profile_finished(name, cx);
                this.status = match (tests, code) {
                    (Some(t), _) => t.summary.clone(),
                    (None, Some(c)) => format!("'{name}' finished with exit code {c}"),
                    (None, None) => format!("'{name}' terminated"),
                };
                this.before_finished(name, *code, window, cx);
                // PyCharm shows a balloon for failed tests when the Run window is hidden.
                if let Some(t) = tests.as_ref().filter(|t| t.failed && this.bottom != Some(BottomTool::Run)) {
                    this.show_toast(t.summary.clone(), true, cx);
                }
                cx.notify();
            }
            RunEvent::RerunFailedRequested => {
                this.save_all(cx);
                match panel.update(cx, |p, cx| {
                    cx.notify();
                    p.rerun_failed()
                }) {
                    Ok(Some(_)) => {}
                    Ok(None) => this.status = "No failed tests to rerun".into(),
                    Err(e) => this.show_toast(e, true, cx),
                }
            }
            RunEvent::RerunRequested => this.rerun_active(window, cx),
        })
        .detach();
        self.runs.panel = Some(p.clone());
        p
    }

    /// A clicked console link: open the file at the (1-based) line.
    fn open_at(&mut self, path: &Path, line: usize, window: &mut Window, cx: &mut Context<Self>) {
        self.open_file(path, window, cx);
        if let Some(ed) = self.active_editor().cloned() {
            ed.update(cx, |e, cx| e.go_to_line(line.saturating_sub(1), cx));
        }
    }

    /// What Ctrl+Shift+F10 runs, like PyCharm's context configuration: the file selected in
    /// the Project view when it has focus (a folder runs pytest in it), else the active
    /// editor. Test files run with pytest, narrowed to the test or class at the caret.
    pub(super) fn context_config(&self, window: &Window, cx: &mut gpui::App) -> Option<RunConfig> {
        let is_py = |p: &Path| p.extension().is_some_and(|e| e == "py" || e == "pyw");
        let for_path = |p: &Path, scope: &[String]| {
            if brisk_run::is_test_file(p) {
                RunConfig::for_tests(&self.root, p, scope)
            } else if let Some(c) = web_config(&self.root, p) {
                c
            } else {
                RunConfig::for_file(&self.root, p)
            }
        };
        if self.tree_focus.is_focused(window) {
            if let Some(n) = self.tree_selected.and_then(|id| self.tree.nodes.get(id)) {
                if n.is_dir {
                    return Some(RunConfig::for_tests(&self.root, &n.path, &[]));
                }
                if is_py(&n.path) {
                    return Some(for_path(&n.path, &[]));
                }
            }
        }
        let ed = self.active_editor()?.clone();
        let path = ed.read(cx).path.clone().filter(|p| is_py(p))?;
        let scope = if brisk_run::is_test_file(&path) {
            let (outline, (line, _)) = ed.update(cx, |e, _| (e.outline(), e.caret_line_col()));
            test_scope(&outline, line)
        } else {
            Vec::new()
        };
        Some(for_path(&path, &scope))
    }

    pub(super) fn run_config(&mut self, cfg: RunConfig, window: &mut Window, cx: &mut Context<Self>) {
        if cfg.kind == RunKind::Compound {
            self.run_compound(&cfg, window, cx);
            return;
        }
        if self.before_launch(&cfg, before_launch_ui::Then::Run, window, cx) {
            return;
        }
        self.save_all(cx);
        let python = match &cfg.interpreter {
            Some(p) => Some(brisk_run::resolve(&self.root, p)),
            None => self.runs.interpreter(&self.root).map(|i| i.path),
        };
        let Some(python) = python else {
            let msg = if self.runs.all.is_none() { "Still looking for Python interpreters, try again in a moment" } else { "No Python interpreter configured: create a .venv or install Python" };
            self.show_toast(msg.into(), true, cx);
            return;
        };
        if cfg.kind == RunKind::Pytest && brisk_run::pytest::has_package(&python, "pytest") == Some(false) {
            let label = self.runs.lookup(&python).label(&self.root);
            self.show_toast(format!("pytest is not installed in {label}: run `pip install pytest` in the Terminal"), true, cx);
            return;
        }
        if let Some(pkg) = brisk_run::web::server_package(cfg.kind).filter(|p| brisk_run::pytest::has_package(&python, p) == Some(false)) {
            let label = self.runs.lookup(&python).label(&self.root);
            self.show_toast(format!("{pkg} is not installed in {label}: run `pip install {pkg}` in the Terminal"), true, cx);
            return;
        }
        if !self.env_files_ok(&cfg, cx) {
            return;
        }
        let cmd = brisk_run::build(&self.root, &cfg, &python);
        self.run_command(cfg, cmd, window, cx);
    }

    /// Rerun (Ctrl+F5 / ↻): the active tab's command again, after its Before launch steps.
    pub(super) fn rerun_active(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let Some((cfg, cmd)) = self.runs.panel.as_ref().and_then(|p| p.read(cx).active_run()) else { return };
        self.save_all(cx);
        if self.before_launch(&cfg, before_launch_ui::Then::Rerun(cmd.clone()), window, cx) {
            return;
        }
        self.run_command(cfg, cmd, window, cx);
    }

    /// A Compound: every member starts in its own Run tab.
    fn run_compound(&mut self, cfg: &RunConfig, window: &mut Window, cx: &mut Context<Self>) {
        let members = match self.runs.file.compound_members(cfg) {
            Ok(m) if m.is_empty() => {
                self.show_toast(format!("'{}' has no configurations to run", cfg.name), true, cx);
                return;
            }
            Ok(m) => m,
            Err(e) => {
                self.show_toast(e, true, cx);
                return;
            }
        };
        let n = members.len();
        for m in members {
            self.run_config(m, window, cx);
        }
        self.status = format!("Running '{}': {n} configurations", cfg.name);
    }

    /// A configuration's `.env` files must be readable (PyCharm refuses to start otherwise).
    pub(super) fn env_files_ok(&mut self, cfg: &RunConfig, cx: &mut Context<Self>) -> bool {
        match brisk_run::envfile::load(&self.root, cfg) {
            Ok(_) => true,
            Err(e) => {
                self.show_toast(format!("'{}': {e}", cfg.name), true, cx);
                false
            }
        }
    }

    /// Start a prepared process in the Run window (a tab named after `cfg`).
    pub(super) fn run_command(&mut self, cfg: RunConfig, cmd: brisk_run::Command, window: &mut Window, cx: &mut Context<Self>) {
        let panel = self.ensure_run_panel(window, cx);
        let name = cfg.name.clone();
        let res = panel.update(cx, |p, cx| {
            cx.notify();
            p.run(cfg, cmd)
        });
        match res {
            Ok(()) => {
                self.bottom = Some(BottomTool::Run);
                self.status = format!("Running '{name}'");
                // PyCharm shows the Run window but keeps the caret in the editor.
                if self.active.is_none() {
                    let f = panel.read(cx).focus_handle(cx);
                    window.focus(&f);
                }
            }
            Err(e) => self.show_toast(e, true, cx),
        }
    }

    pub(super) fn run_action(&mut self, action: Action, window: &mut Window, cx: &mut Context<Self>) {
        use Action as A;
        match action {
            A::Run => {
                self.reload_runs();
                if let Some(cfg) = self.runs.file.selected_config().cloned() {
                    self.run_config(cfg, window, cx);
                } else if let Some(cfg) = self.context_config(window, cx) {
                    self.run_config(cfg, window, cx);
                } else {
                    self.status = "Nothing to run: open a Python file or select a configuration".into();
                }
            }
            A::RunContext => {
                let Some(ctx) = self.context_config(window, cx) else {
                    self.status = "Run: open a Python file first".into();
                    return;
                };
                self.reload_runs();
                let name = self.runs.file.use_for_file(ctx);
                self.save_runs();
                if let Some(cfg) = self.runs.file.get(&name).cloned() {
                    self.run_config(cfg, window, cx);
                }
            }
            A::Rerun => self.rerun_active(window, cx),
            A::Stop => {
                let stopped = self.runs.panel.as_ref().is_some_and(|p| p.update(cx, |p, cx| {
                    cx.notify();
                    p.stop()
                }));
                if !stopped {
                    self.status = "Nothing to stop".into();
                }
            }
            A::SelectRunConfiguration => {
                self.reload_runs();
                self.open_popup(PopupKind::RunConfigs(true));
            }
            A::EditRunConfigurations => self.edit_run_configurations(window, cx),
            A::ToggleRunWindow => {
                if self.bottom == Some(BottomTool::Run) && self.run_panel_focused(window, cx) {
                    self.bottom = None;
                    self.focus_editor(window, cx);
                } else {
                    let p = self.ensure_run_panel(window, cx);
                    self.bottom = Some(BottomTool::Run);
                    let f = p.read(cx).focus_handle(cx);
                    window.focus(&f);
                }
            }
            _ => {}
        }
    }

    /// The Run/Debug Configurations dialog (`run_config_ui`).
    fn edit_run_configurations(&mut self, _window: &mut Window, cx: &mut Context<Self>) {
        self.popup = None;
        self.open_run_dialog(cx);
    }

    pub(super) fn run_popup_items(&self, kind: PopupKind, q: &str) -> Vec<PopupItem> {
        let ql = q.to_lowercase();
        match kind {
            PopupKind::RunConfigs(run) => {
                let mut v = Vec::new();
                let sel = self.runs.file.selected.clone();
                let hint = |on: bool| if on { "✓".to_string() } else { String::new() };
                if "current file".contains(&ql) {
                    v.push(PopupItem { label: "Current File".into(), detail: "runs the file in the editor".into(), hint: hint(sel.is_none()), target: Target::RunConfig(None, run) });
                }
                for c in self.runs.file.configurations.iter().filter(|c| c.name.to_lowercase().contains(&ql)) {
                    let mut detail = match (c.kind, c.module) {
                        (RunKind::Pytest, _) => format!("pytest {}", c.target),
                        (RunKind::Compound, _) => format!("compound: {}", brisk_run::compound_names(&c.target).join(", ")),
                        (RunKind::FastApi, _) => format!("uvicorn {}", c.target),
                        (RunKind::Flask, _) => format!("flask --app {} run", c.target),
                        (RunKind::Django, _) => format!("{} runserver", c.target),
                        (RunKind::DjangoTests, _) => format!("{} test", c.target),
                        (_, true) => format!("-m {}", c.target),
                        _ => c.target.clone(),
                    };
                    if !c.parameters.is_empty() {
                        detail = format!("{detail} {}", c.parameters);
                    }
                    if c.temporary {
                        detail = format!("{detail} · temporary");
                    }
                    v.push(PopupItem { label: c.name.clone(), detail, hint: hint(sel.as_deref() == Some(c.name.as_str())), target: Target::RunConfig(Some(c.name.clone()), run) });
                }
                // PyCharm: "Save 'x' Configuration" for the selected temporary one.
                if let Some(c) = self.runs.file.selected_config().filter(|c| c.temporary) {
                    let label = format!("Save '{}' Configuration", c.name);
                    if label.to_lowercase().contains(&ql) {
                        v.push(PopupItem { label, detail: "keep it: temporary ones are replaced after five newer".into(), hint: String::new(), target: Target::SaveRunConfig(c.name.clone()) });
                    }
                }
                if "edit configurations".contains(&ql) {
                    v.push(PopupItem { label: "Edit Configurations…".into(), detail: brisk_run::CONFIG_FILE.into(), hint: String::new(), target: Target::EditRunConfigs });
                }
                v
            }
            PopupKind::Interpreters => {
                let current = self.runs.interpreter(&self.root).map(|i| i.path);
                let mut v: Vec<PopupItem> = self
                    .runs
                    .known()
                    .filter(|i| i.path.to_string_lossy().to_lowercase().contains(&ql) || i.label(&self.root).to_lowercase().contains(&ql))
                    .map(|i| PopupItem {
                        label: i.label(&self.root),
                        detail: i.path.display().to_string(),
                        hint: if current.as_deref() == Some(i.path.as_path()) { "✓".into() } else { String::new() },
                        target: Target::Interpreter(i.path.clone()),
                    })
                    .collect();
                v.dedup_by(|a, b| a.detail == b.detail);
                if self.runs.all.is_none() {
                    v.push(PopupItem { label: "Detecting interpreters…".into(), detail: String::new(), hint: String::new(), target: Target::Cancel });
                }
                if "add interpreter".contains(&ql) && !self.venv_busy {
                    v.push(PopupItem { label: "Add Interpreter…".into(), detail: "a new .venv in the project".into(), hint: String::new(), target: Target::AddInterpreter });
                }
                v
            }
            _ => Vec::new(),
        }
    }

    pub(super) fn run_popup_pick(&mut self, target: Target, window: &mut Window, cx: &mut Context<Self>) {
        match target {
            Target::RunConfig(name, run) => {
                self.runs.file.selected = name;
                self.save_runs();
                self.focus_editor(window, cx);
                if run {
                    self.run_action(Action::Run, window, cx);
                }
            }
            Target::EditRunConfigs => self.edit_run_configurations(window, cx),
            Target::SaveRunConfig(name) => {
                self.reload_runs();
                if let Some(c) = self.runs.file.configurations.iter_mut().find(|c| c.name == name) {
                    c.temporary = false;
                    self.save_runs();
                    self.status = format!("Saved run configuration '{name}'");
                }
                self.focus_editor(window, cx);
            }
            Target::Interpreter(path) => {
                self.runs.file.interpreter = Some(brisk_run::relativize(&self.root, &path));
                self.save_runs();
                self.status = format!("Interpreter: {}", self.interpreter_label());
                self.restart_ty(cx);
                self.sync_terminal_env(cx);
                self.focus_editor(window, cx);
            }
            _ => {}
        }
    }

    /// `BRISK_SMOKE_RUN=<file>`: Ctrl+Shift+F10 on the file, wait for it, print the console
    /// and follow the first traceback link.
    pub(super) fn run_smoke(&mut self, file: PathBuf, window: &mut Window, cx: &mut Context<Self>) {
        let t0 = std::time::Instant::now();
        self.open_file(&file, window, cx);
        let bg = cx.background_executor().clone();
        cx.spawn_in(window, async move |this, cx| {
            // Wait for interpreter discovery when there is no project venv.
            for _ in 0..100 {
                let ready = this.update(cx, |this, _| this.runs.interpreter(&this.root).is_some() || this.runs.all.is_some()).unwrap_or(true);
                if ready {
                    break;
                }
                bg.timer(std::time::Duration::from_millis(100)).await;
            }
            let _ = this.update_in(cx, |this, window, cx| this.dispatch(Action::RunContext, window, cx));
            for _ in 0..300 {
                bg.timer(std::time::Duration::from_millis(100)).await;
                let done = this.update(cx, |this, cx| this.runs.panel.as_ref().is_some_and(|p| !p.read(cx).is_running())).unwrap_or(true);
                if done {
                    break;
                }
            }
            let _ = this.update_in(cx, |this, window, cx| {
                let Some(p) = this.runs.panel.clone() else {
                    eprintln!("smoke-run: nothing ran ({})", this.status);
                    return;
                };
                eprintln!("smoke-run: interpreter {}", this.interpreter_label());
                eprintln!("smoke-run: console:\n{}", p.read(cx).active_text().unwrap_or_default());
                eprintln!("smoke-run: status '{}' after {:.1?}", this.status, t0.elapsed());
                if let Some(tree) = p.read(cx).tests_tree_text() {
                    eprintln!("smoke-run: {}\n{tree}", p.read(cx).tests_summary().unwrap_or_default());
                    if let Some((name, Some((path, line)))) = p.read(cx).selected_test() {
                        eprintln!("smoke-run: selected {name}");
                        this.open_at(&path, line, window, cx);
                        let at = this.active_editor().map(|e| (e.read(cx).path.clone(), e.read(cx).caret_line_col()));
                        eprintln!("smoke-run: test source -> {at:?}");
                    }
                    if std::env::var_os("BRISK_SMOKE_RERUN_FAILED").is_some() {
                        let r = p.update(cx, |p, cx| {
                            cx.notify();
                            p.rerun_failed()
                        });
                        eprintln!("smoke-run: rerun failed -> {r:?}");
                    }
                    return;
                }
                if let Some((path, line)) = p.read(cx).first_link() {
                    this.open_at(&path, line, window, cx);
                    let at = this.active_editor().map(|e| (e.read(cx).path.clone(), e.read(cx).caret_line_col()));
                    eprintln!("smoke-run: link -> {at:?}");
                }
                cx.notify();
            });
        })
        .detach();
    }
}

impl Workspace {
    /// `BRISK_SMOKE_RUNWIN=1` (on a scratch project): writes `ask.py` (input()), `main.py`,
    /// `sleep.py` and `w1.py`…`w5.py` into the project root and checks stdin,
    /// save-before-run, tabs, toggles, Clear All and saving a temporary configuration.
    pub(super) fn run_window_smoke(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let bg = cx.background_executor().clone();
        let root = self.root.clone();
        let mut files = vec![
            ("ask.py".to_string(), "name = input(\"name? \")\nprint(\"hello\", name)\n".to_string()),
            ("main.py".to_string(), "print(\"main\")\n".to_string()),
            ("sleep.py".to_string(), "import time\nprint(\"sleeping\")\ntime.sleep(60)\n".to_string()),
        ];
        for i in 1..=5 {
            files.push((format!("w{i}.py"), format!("print(\"w{i} \" + \"x\" * 150)\n")));
        }
        for (name, text) in files {
            if let Err(e) = std::fs::write(root.join(&name), text) {
                eprintln!("smoke-runwin: cannot write {name}: {e}");
            }
        }
        let _ = std::fs::remove_file(root.join(brisk_run::CONFIG_FILE));
        self.reload_runs();
        self.runs.file = Default::default();
        cx.spawn_in(window, async move |this, cx| {
            let wait_idle = async |this: &gpui::WeakEntity<Workspace>, cx: &mut gpui::AsyncWindowContext, what: &str| {
                for _ in 0..200 {
                    bg.timer(std::time::Duration::from_millis(100)).await;
                    let ok = this.update(cx, |this, cx| this.runs.panel.as_ref().is_some_and(|p| !p.read(cx).run_tabs().iter().any(|(n, r)| *r && n != "sleep"))).unwrap_or(true);
                    if ok {
                        return;
                    }
                }
                eprintln!("smoke-runwin: timeout waiting for {what}");
            };
            let text = |this: &gpui::WeakEntity<Workspace>, cx: &mut gpui::AsyncWindowContext| {
                this.update(cx, |this, cx| this.runs.panel.as_ref().and_then(|p| p.read(cx).active_text()).unwrap_or_default()).unwrap_or_default()
            };
            // Wait for an interpreter.
            for _ in 0..100 {
                let ready = this.update(cx, |this, _| this.runs.interpreter(&this.root).is_some()).unwrap_or(true);
                if ready {
                    break;
                }
                bg.timer(std::time::Duration::from_millis(100)).await;
            }
            // 1. stdin: type into the console while input() waits.
            let _ = this.update_in(cx, |this, window, cx| {
                this.open_file(&root.join("ask.py"), window, cx);
                this.dispatch(Action::RunContext, window, cx);
            });
            bg.timer(std::time::Duration::from_millis(1500)).await;
            let _ = this.update_in(cx, |this, window, cx| {
                let Some(p) = this.runs.panel.clone() else { return };
                window.focus(&p.read(cx).focus_handle(cx));
                for ch in "Bob".chars() {
                    let k = gpui::Keystroke { modifiers: gpui::Modifiers::default(), key: ch.to_ascii_lowercase().to_string(), key_char: Some(ch.to_string()) };
                    p.update(cx, |p, cx| p.key(&k, cx));
                }
                let k = gpui::Keystroke { modifiers: gpui::Modifiers::default(), key: "enter".into(), key_char: Some("\r".into()) };
                p.update(cx, |p, cx| p.key(&k, cx));
            });
            wait_idle(&this, cx, "ask.py").await;
            eprintln!("smoke-runwin: stdin console:\n{}", text(&this, cx));
            // 2. Save all before launch: an unsaved edit in main.py runs.
            let _ = this.update_in(cx, |this, window, cx| {
                this.open_file(&root.join("main.py"), window, cx);
                if let Some(ed) = this.active_editor().cloned() {
                    ed.update(cx, |e, _| {
                        e.buffer.select_all();
                        e.buffer.insert("print('edited before run')\n");
                    });
                }
                eprintln!("smoke-runwin: main.py dirty -> {:?}", this.active_editor().map(|e| e.read(cx).buffer.is_dirty()));
                this.dispatch(Action::RunContext, window, cx);
            });
            wait_idle(&this, cx, "main.py").await;
            eprintln!("smoke-runwin: main.py console:\n{}", text(&this, cx));
            eprintln!("smoke-runwin: main.py on disk -> {:?}", std::fs::read_to_string(root.join("main.py")).unwrap_or_default());
            // 3. Tabs: a long run plus five more; at most five tabs, the running one stays.
            for f in ["sleep.py", "w1.py", "w2.py", "w3.py", "w4.py", "w5.py"] {
                let _ = this.update_in(cx, |this, window, cx| {
                    this.open_file(&root.join(f), window, cx);
                    this.dispatch(Action::RunContext, window, cx);
                });
                bg.timer(std::time::Duration::from_millis(300)).await;
            }
            wait_idle(&this, cx, "w*.py").await;
            let _ = this.update_in(cx, |this, window, cx| {
                let Some(p) = this.runs.panel.clone() else { return };
                eprintln!("smoke-runwin: tabs -> {:?}", p.read(cx).run_tabs());
                // Rerun (Ctrl+F5) the active tab.
                this.dispatch(Action::Rerun, window, cx);
                eprintln!("smoke-runwin: ctrl+f5 -> tabs {:?} status {:?}", p.read(cx).run_tabs(), this.status);
            });
            wait_idle(&this, cx, "rerun").await;
            let _ = this.update_in(cx, |this, window, cx| {
                let Some(p) = this.runs.panel.clone() else { return };
                // 4. Toggles and Clear All.
                let cols = p.read(cx).columns();
                p.update(cx, |p, _| p.toggle_soft_wrap());
                window.refresh();
                eprintln!("smoke-runwin: rerun console {:?}", p.read(cx).active_text());
                eprintln!("smoke-runwin: soft wrap {} cols {cols}", p.read(cx).soft_wrap);
                p.update(cx, |p, _| p.toggle_scroll_to_end());
                eprintln!("smoke-runwin: scroll to end -> {}", p.read(cx).scroll_to_end);
                let hold = std::env::var_os("BRISK_SMOKE_RUNWIN_HOLD").is_some();
                if !hold {
                    p.update(cx, |p, _| p.clear());
                    eprintln!("smoke-runwin: clear -> {:?}", p.read(cx).active_text());
                }
                if hold {
                    return;
                }
                // Stop the long run.
                this.dispatch(Action::Stop, window, cx);
                // 5. The temporary configuration can be saved from the chooser.
                this.dispatch(Action::SelectRunConfiguration, window, cx);
                let items: Vec<String> = this.popup.as_ref().map(|p| p.items.iter().map(|i| i.label.clone()).collect()).unwrap_or_default();
                eprintln!("smoke-runwin: chooser -> {items:?}");
                if let Some(i) = items.iter().position(|l| l.starts_with("Save '")) {
                    if let Some(p) = this.popup.as_mut() {
                        p.selected = i;
                    }
                    this.smoke_press("enter", window, cx);
                }
                let f = brisk_run::load(&this.root).0;
                eprintln!("smoke-runwin: saved -> {:?} status {:?}", f.selected_config().map(|c| (c.name.clone(), c.temporary)), this.status);
            });
            bg.timer(std::time::Duration::from_millis(800)).await;
            let _ = this.update(cx, |this, cx| {
                eprintln!("smoke-runwin: after stop -> {:?}", this.runs.panel.as_ref().map(|p| p.read(cx).run_tabs()));
                eprintln!("smoke-runwin: columns with soft wrap off -> {:?}", this.runs.panel.as_ref().map(|p| p.read(cx).columns()));
                eprintln!("smoke-runwin: done");
                if std::env::var_os("BRISK_SMOKE_RUNWIN_HOLD").is_none() {
                    cx.quit();
                }
            });
        })
        .detach();
    }
}

/// The pytest node path at a caret line: `["TestA", "test_b"]` inside a test method,
/// `["TestA"]` elsewhere in a test class, `[]` outside tests.
/// A file that defines a FastAPI / Flask application and has no `__main__` block runs as a
/// server (PyCharm offers its FastAPI / Flask configuration for it).
fn web_config(root: &Path, file: &Path) -> Option<RunConfig> {
    let src = std::fs::read_to_string(file).ok()?;
    // Django's manage.py runs the development server (PyCharm's Django Server).
    if file.file_name().is_some_and(|n| n == "manage.py") && src.contains("django") {
        return Some(RunConfig::for_django(root, RunKind::Django, file, true));
    }
    if src.contains("__name__ == \"__main__\"") || src.contains("__name__ == '__main__'") {
        return None;
    }
    let (kind, target) = brisk_run::web::app_target(root, file, &src)?;
    Some(RunConfig::for_web(kind, file, target))
}

pub(super) fn test_scope(outline: &[brisk_syntax::OutlineItem], line: usize) -> Vec<String> {
    let mut enclosing: Vec<&brisk_syntax::OutlineItem> = outline.iter().filter(|o| o.line <= line && line <= o.end_line).collect();
    enclosing.sort_by_key(|o| o.depth);
    let mut scope = Vec::new();
    for o in enclosing {
        let ok = match o.kind {
            brisk_syntax::OutlineKind::Class => o.name.starts_with("Test"),
            _ => o.name.starts_with("test"),
        };
        if !ok || o.depth != scope.len() {
            break;
        }
        scope.push(o.name.clone());
        if o.kind != brisk_syntax::OutlineKind::Class {
            break;
        }
    }
    scope
}

#[cfg(test)]
mod tests {
    use super::test_scope;

    #[test]
    fn caret_picks_test_or_class() {
        let src = "import pytest\n\ndef helper():\n    pass\n\ndef test_top():\n    assert helper() is None\n\nclass TestA:\n    x = 1\n\n    @pytest.mark.slow\n    def test_b(self):\n        assert True\n\n    def util(self):\n        pass\n";
        let mut s = brisk_syntax::SyntaxTree::new(brisk_syntax::Lang::Python);
        s.parse(src);
        let o = s.outline(src);
        let at = |line: usize| test_scope(&o, line);
        assert!(at(0).is_empty());
        assert!(at(3).is_empty());
        assert_eq!(at(6), ["test_top"]);
        assert_eq!(at(9), ["TestA"]);
        assert_eq!(at(13), ["TestA", "test_b"]);
        assert_eq!(at(16), ["TestA"]);
    }
}
