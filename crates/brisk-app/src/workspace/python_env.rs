//! The project's Python environment: the interpreters found (project venvs at once, the rest
//! of the machine in the background), the project interpreter chosen in the status bar,
//! the SSH interpreters added to the project, and creating or syncing an environment (Add
//! Interpreter, uv / Poetry) with the balloon offering to install requirements.
//!
//! `run.json` keeps the choice (`interpreter`, `sshInterpreters`) next to the run
//! configurations; [`PythonEnv`] owns it while the project is open, and `Workspace::save_runs`
//! writes it back into the file.
//!
//! [`PythonEnv`] never sees `Workspace`. It reports with [`EnvEvent`]; the one that matters
//! most is [`EnvEvent::InterpreterChanged`], which ty, ruff, the terminal, Python Packages and
//! the Python Console follow (`Workspace::on_env_event`). Runs and the debugger ask for the
//! interpreter when they start, so they need no event.

use super::venv_ui::{track, OfferKind, VenvOffer};
use super::Workspace;
use crate::theme::{ui_px, Theme};
use brisk_run::env::manager as py_manager;
use brisk_run::env::venv as py_venv;
use brisk_run::pyproj::{self, Manager};
use brisk_run::ssh::{self, HostInfo, SshInterpreter};
use brisk_run::{InterpKind, Interpreter};
use gpui::prelude::*;
use gpui::{div, px, Context, Entity, EventEmitter, Window};
use std::path::{Path, PathBuf};

/// What [`PythonEnv`] asks the workspace to do.
pub enum EnvEvent {
    /// The local project interpreter is another one, or its environment was just created or
    /// synced. ty, ruff, the terminal, Packages and the console follow it.
    InterpreterChanged,
    /// The choice `run.json` keeps changed here: write the file.
    ChoiceChanged,
    /// Discovery of the machine's interpreters finished.
    Discovered,
    /// Packages were installed into the project interpreter.
    PackagesChanged,
    Status(String),
    /// A balloon; `true` for an error.
    Toast(String, bool),
}

/// The choice (`selected`, `ssh`) is private: it changes only through the methods, which
/// send the events. The other fields are read directly.
pub struct PythonEnv {
    root: PathBuf,
    /// The project interpreter as `run.json` keeps it: a path (relative to the project when
    /// inside it) or an `ssh://` URI. `None` picks the project's virtualenv automatically.
    selected: Option<String>,
    /// SSH interpreters added to the project.
    ssh: Vec<SshInterpreter>,
    /// Project virtualenvs, known instantly from `pyvenv.cfg`.
    pub venvs: Vec<Interpreter>,
    /// Everything found on the machine; `None` while discovery runs in the background.
    pub all: Option<Vec<Interpreter>>,
    /// A project venv is being created or synced (Add Interpreter, uv / Poetry).
    pub busy: bool,
    /// The offer to install requirements into a new venv, or to sync after a pyproject change.
    pub offer: Option<VenvOffer>,
    /// `pyproj::deps_fingerprint` of pyproject.toml when the project opened or was last synced.
    pyproject_fp: Option<u64>,
    /// Add SSH Interpreter: the host just probed and what it has.
    pub ssh_found: Option<(String, HostInfo)>,
}

impl EventEmitter<EnvEvent> for PythonEnv {}

impl PythonEnv {
    /// `selected` and `ssh` as read from `run.json`.
    pub fn new(root: PathBuf, selected: Option<String>, ssh: Vec<SshInterpreter>) -> Self {
        let venvs = brisk_run::project_venvs(&root);
        PythonEnv { root, selected, ssh, venvs, all: None, busy: false, offer: None, pyproject_fp: None, ssh_found: None }
    }

    /// The project interpreter as `run.json` keeps it.
    pub fn selected(&self) -> Option<&str> {
        self.selected.as_deref()
    }

    /// The SSH interpreters added to the project.
    pub fn ssh_interpreters(&self) -> &[SshInterpreter] {
        &self.ssh
    }

    /// Every interpreter known so far: project venvs first.
    pub fn known(&self) -> impl Iterator<Item = &Interpreter> {
        self.venvs.iter().chain(self.all.iter().flatten())
    }

    /// The interpreter at `path`: a known one, else what its folder says.
    pub fn lookup(&self, path: &Path) -> Interpreter {
        self.known().find(|i| i.path == path).cloned().unwrap_or_else(|| {
            let venv = Interpreter::venv_dir_of(path);
            Interpreter {
                path: path.to_path_buf(),
                version: venv.as_deref().map(brisk_run::venv_version).unwrap_or_default(),
                kind: if venv.is_some() { InterpKind::Venv } else { InterpKind::System },
            }
        })
    }

    /// The project interpreter: the one chosen in the status bar, else the project venv,
    /// else the first interpreter found on the machine.
    /// With an SSH interpreter selected this is the local fallback that code insight, the
    /// console and the debugger use ([`Self::ssh`] is what runs use).
    pub fn interpreter(&self) -> Option<Interpreter> {
        match &self.selected {
            Some(p) if !p.starts_with(ssh::SCHEME) => Some(self.lookup(&brisk_run::resolve(&self.root, p))),
            _ => self.venvs.first().or_else(|| self.all.as_ref().and_then(|a| a.first())).cloned(),
        }
    }

    fn local(&self) -> Option<PathBuf> {
        self.interpreter().map(|i| i.path)
    }

    /// The project's SSH interpreter, when one is selected.
    pub fn ssh(&self) -> Option<SshInterpreter> {
        let uri = self.selected.as_deref()?;
        self.ssh_by_uri(uri)
    }

    /// The SSH interpreter with this URI, selected or not.
    pub fn ssh_by_uri(&self, uri: &str) -> Option<SshInterpreter> {
        self.ssh.iter().find(|s| s.uri() == uri).cloned()
    }

    /// A local interpreter is configured, or the project has a venv: known before discovery
    /// of the rest of the machine ends.
    pub fn local_known(&self) -> bool {
        self.selected.as_deref().is_some_and(|p| !p.starts_with(ssh::SCHEME)) || !self.venvs.is_empty()
    }

    /// What the status bar shows.
    pub fn label(&self) -> String {
        if let Some(s) = self.ssh() {
            return s.label();
        }
        match self.interpreter() {
            Some(i) => i.label(&self.root),
            None if self.all.is_none() => "Detecting interpreter…".into(),
            None => "<No interpreter>".into(),
        }
    }

    // --- Changes ------------------------------------------------------------------------

    /// Looks for every Python on the machine in the background.
    pub fn start_discovery(&mut self, cx: &mut Context<Self>) {
        let root = self.root.clone();
        let task = cx.background_executor().spawn(async move { brisk_run::discover_all(&root) });
        cx.spawn(async move |this, cx| {
            let all = task.await;
            let _ = this.update(cx, |this, cx| {
                this.all = Some(all);
                cx.emit(EnvEvent::Discovered);
                cx.notify();
            });
        })
        .detach();
    }

    /// Makes `selected` (a path, relative to the project when inside it, or an `ssh://` URI)
    /// the project interpreter. A local one picked again still counts as a change: that is
    /// how a ty that exited is started again.
    pub fn select(&mut self, selected: Option<String>, cx: &mut Context<Self>) {
        let before = self.local();
        let local = selected.as_deref().is_some_and(|p| !p.starts_with(ssh::SCHEME));
        self.selected = selected;
        cx.emit(EnvEvent::ChoiceChanged);
        if local {
            cx.emit(EnvEvent::InterpreterChanged);
        } else {
            self.changed_since(before, cx);
        }
        cx.notify();
    }

    /// Makes the interpreter at `path` the project interpreter.
    pub fn select_path(&mut self, path: &Path, cx: &mut Context<Self>) {
        self.select(Some(brisk_run::relativize(&self.root, path)), cx);
    }

    /// Adds an SSH interpreter (replacing one with the same URI) and makes it the project
    /// interpreter.
    pub fn add_ssh(&mut self, s: SshInterpreter, cx: &mut Context<Self>) {
        let uri = s.uri();
        self.ssh.retain(|x| x.uri() != uri);
        self.ssh.push(s);
        self.select(Some(uri), cx);
    }

    /// The choice as `run.json` has it after a change on disk; it is not written back.
    pub fn set_choice(&mut self, (selected, ssh): super::run_ui::EnvChoice, cx: &mut Context<Self>) {
        if selected == self.selected && ssh == self.ssh {
            return;
        }
        let before = self.local();
        self.selected = selected;
        self.ssh = ssh;
        self.changed_since(before, cx);
        cx.notify();
    }

    fn changed_since(&self, before: Option<PathBuf>, cx: &mut Context<Self>) {
        if self.local() != before {
            cx.emit(EnvEvent::InterpreterChanged);
        }
    }

    fn tell(&self, msg: String, error: bool, cx: &mut Context<Self>) {
        cx.emit(EnvEvent::Status(msg.clone()));
        cx.emit(EnvEvent::Toast(msg, error));
    }

    /// Remembers the dependency part of `pyproject.toml`, so a later save can tell whether
    /// the dependencies changed.
    pub fn remember_pyproject(&mut self) {
        self.pyproject_fp = std::fs::read_to_string(self.root.join("pyproject.toml")).ok().map(|t| pyproj::deps_fingerprint(&t));
    }

    /// After a save of `pyproject.toml` whose dependencies changed while the lock file is older:
    /// the balloon offering `uv sync` / `poetry lock && install`.
    pub fn pyproject_saved(&mut self, saved: &[PathBuf], cx: &mut Context<Self>) {
        let file = self.root.join("pyproject.toml");
        if !saved.contains(&file) {
            return;
        }
        let fp = std::fs::read_to_string(&file).ok().map(|t| pyproj::deps_fingerprint(&t));
        let changed = fp != self.pyproject_fp;
        self.pyproject_fp = fp;
        let Some(m) = pyproj::detect(&self.root) else { return };
        if !changed || !pyproj::lock_outdated(&self.root, m) || self.busy || self.offer.is_some() {
            return;
        }
        let python = self.local().unwrap_or_default();
        self.offer = Some(VenvOffer { python, what: OfferKind::Sync(m), installing: false });
        cx.notify();
    }

    /// `uv sync` / `poetry install` in the background, then make the environment the project
    /// interpreter, as after creating a venv.
    pub fn sync_env(&mut self, m: Manager, cx: &mut Context<Self>) {
        if self.busy {
            return;
        }
        let tool = match py_manager::tool(m, self.local().as_deref()) {
            Ok(t) => t,
            Err(e) => return self.tell(e, true, cx),
        };
        let what = m.sync_args().join(" ");
        let what = format!("{} {what}", m.name().to_lowercase());
        self.busy = true;
        cx.emit(EnvEvent::Status(format!("{what}…")));
        let root = self.root.clone();
        let task = cx.background_executor().spawn(async move { py_manager::sync(m, &tool, &root) });
        cx.spawn(async move |this, cx| {
            let res = task.await;
            let _ = this.update(cx, |this, cx| {
                this.busy = false;
                match res {
                    Ok(python) => this.use_new_environment(python, format!("{what}: done"), cx),
                    Err(e) => this.tell(format!("{what} failed: {e}"), true, cx),
                }
                cx.notify();
            });
        })
        .detach();
        cx.notify();
    }

    /// `base -m venv .venv` in the background, then make it the project interpreter and
    /// offer to install the project's requirements into it.
    pub fn create_venv(&mut self, base: PathBuf, cx: &mut Context<Self>) {
        if self.busy {
            return;
        }
        let Some(dir) = py_venv::free_dir(&self.root) else {
            cx.emit(EnvEvent::Toast("The project already has .venv and venv".into(), true));
            return;
        };
        let name = dir.file_name().map(|n| n.to_string_lossy().into_owned()).unwrap_or_default();
        let base_label = self.lookup(&base).label(&self.root);
        self.busy = true;
        cx.emit(EnvEvent::Status(format!("Creating {name} with {base_label}…")));
        let root = self.root.clone();
        let task = cx.background_executor().spawn(async move {
            let python = py_venv::create(&base, &dir)?;
            Ok::<_, String>((python, py_venv::requirements(&root)))
        });
        cx.spawn(async move |this, cx| {
            let res = task.await;
            let _ = this.update(cx, |this, cx| {
                this.busy = false;
                match res {
                    Ok((python, req)) => {
                        this.use_new_environment(python.clone(), format!("Created {name}"), cx);
                        this.offer = req.map(|req| VenvOffer { python, what: OfferKind::Install(req), installing: false });
                    }
                    Err(e) => this.tell(format!("Cannot create {name}: {e}"), true, cx),
                }
                cx.notify();
            });
        })
        .detach();
        cx.notify();
    }

    /// Makes a freshly created or synced environment the project interpreter. Its packages
    /// are new even when the path is the same, so everything that follows the interpreter
    /// is told it changed.
    fn use_new_environment(&mut self, python: PathBuf, msg: String, cx: &mut Context<Self>) {
        self.remember_pyproject();
        self.venvs = brisk_run::project_venvs(&self.root);
        self.selected = Some(brisk_run::relativize(&self.root, &python));
        cx.emit(EnvEvent::ChoiceChanged);
        cx.emit(EnvEvent::InterpreterChanged);
        self.start_discovery(cx);
        let label = self.lookup(&python).label(&self.root);
        self.tell(format!("{msg}; the project interpreter is {label}"), false, cx);
    }

    /// The balloon's primary button: install the requirements (or sync with uv / Poetry).
    pub fn install_requirements(&mut self, cx: &mut Context<Self>) {
        let Some(o) = self.offer.as_mut().filter(|o| !o.installing) else { return };
        let req = match &o.what {
            OfferKind::Install(req) => req.clone(),
            OfferKind::Sync(m) => {
                let m = *m;
                self.offer = None;
                self.sync_env(m, cx);
                return;
            }
        };
        o.installing = true;
        let (python, root) = (o.python.clone(), self.root.clone());
        cx.emit(EnvEvent::Status(format!("Installing requirements from {}…", req.label())));
        let task = cx.background_executor().spawn(async move { py_venv::install(&python, &root, &req) });
        cx.spawn(async move |this, cx| {
            let res = task.await;
            let _ = this.update(cx, |this, cx| {
                this.offer = None;
                match res {
                    Ok(()) => this.tell("Requirements installed".into(), false, cx),
                    Err(e) => this.tell(format!("Installing requirements failed: {e}"), true, cx),
                }
                cx.emit(EnvEvent::PackagesChanged);
                cx.notify();
            });
        })
        .detach();
        // The balloon swaps its buttons for "Installing…" at once.
        cx.notify();
    }

    /// The balloon's Not now.
    pub fn dismiss_offer(&mut self, cx: &mut Context<Self>) {
        self.offer = None;
        cx.notify();
    }

    /// Smokes: show this offer.
    #[cfg(feature = "smoke")]
    pub fn set_offer(&mut self, offer: Option<VenvOffer>, cx: &mut Context<Self>) {
        self.offer = offer;
        cx.notify();
    }

    /// The balloon offering to install requirements into the new environment, drawn by the
    /// workspace over everything.
    pub fn render_offer(&mut self, cx: &mut Context<Self>) -> Option<gpui::AnyElement> {
        let o = self.offer.as_ref()?;
        let button = |id: &'static str, label: &'static str, primary: bool| {
            div()
                .id(id)
                .relative()
                .h(ui_px(24.))
                .px(px(10.))
                .flex()
                .items_center()
                .rounded(px(5.))
                .cursor_pointer()
                .when(primary, |d| d.bg(Theme::accent()).text_color(gpui::white()))
                .when(!primary, |d| d.border_1().border_color(Theme::border()))
                .map(|d| crate::ui_kit::button_states(d, primary))
                .child(label)
                .child(track(if primary { 0 } else { 1 }))
        };
        Some(
            div()
                .id("venv-offer")
                .occlude()
                .absolute()
                .bottom(ui_px(28.) + px(60.))
                .right(super::toast_right())
                .w(ui_px(420.))
                .p(px(12.))
                .flex()
                .flex_col()
                .gap(px(8.))
                .rounded(px(8.))
                .bg(Theme::panel())
                .border_1()
                .border_color(Theme::divider())
                .map(crate::ui_kit::floating)
                .text_size(ui_px(13.))
                .text_color(Theme::text())
                .child(div().child(o.text()))
                .child(if o.installing {
                    div().text_color(Theme::text_muted()).child("Installing… (progress in the status bar)").into_any_element()
                } else {
                    div()
                        .flex()
                        .gap(px(8.))
                        .child(button("venv-install", o.button(), true).on_click(cx.listener(|this, _, _, cx| this.install_requirements(cx))))
                        .child(button("venv-dismiss", "Not now", false).on_click(cx.listener(|this, _, _, cx| this.dismiss_offer(cx))))
                        .into_any_element()
                })
                .into_any_element(),
        )
    }
}

/// What the interpreter popups list, taken from [`PythonEnv`] when one of them opens: popup
/// rows are built where the entity cannot be read.
#[derive(Clone, Default)]
pub struct EnvView {
    /// Project venvs, then the rest of the machine.
    pub known: Vec<Interpreter>,
    /// The machine's interpreters; `None` while discovery runs.
    pub machine: Option<Vec<Interpreter>>,
    /// The local project interpreter.
    pub current: Option<PathBuf>,
    /// The project interpreter as `run.json` keeps it (a path or an `ssh://` URI).
    pub selected: Option<String>,
    /// An SSH interpreter is the project interpreter.
    pub ssh_selected: bool,
    pub ssh: Vec<SshInterpreter>,
    pub ssh_found: Option<(String, HostInfo)>,
    pub busy: bool,
}

impl PythonEnv {
    pub fn view(&self) -> EnvView {
        EnvView {
            known: self.known().cloned().collect(),
            machine: self.all.clone(),
            current: self.local(),
            selected: self.selected.clone(),
            ssh_selected: self.ssh().is_some(),
            ssh: self.ssh.clone(),
            ssh_found: self.ssh_found.clone(),
            busy: self.busy,
        }
    }

    /// The background tasks the status bar lists.
    pub fn tasks(&self) -> Vec<&'static str> {
        let mut v = Vec::new();
        if self.busy {
            v.push("Creating virtual environment…");
        }
        if self.offer.as_ref().is_some_and(|o| o.installing) {
            v.push("Installing requirements…");
        }
        if self.all.is_none() {
            v.push("Detecting interpreters…");
        }
        v
    }
}

impl Workspace {
    /// Creates the model at startup (the status bar shows the interpreter in the first
    /// frame) and turns its events into the workspace's reactions.
    pub(super) fn new_env(root: &Path, (selected, ssh): super::run_ui::EnvChoice, window: &mut Window, cx: &mut Context<Self>) -> Entity<PythonEnv> {
        let env = cx.new(|_| PythonEnv::new(root.to_path_buf(), selected, ssh));
        cx.subscribe_in(&env, window, |this: &mut Self, _, ev: &EnvEvent, _, cx| this.on_env_event(ev, cx)).detach();
        cx.observe(&env, |_, _, cx| cx.notify()).detach();
        env
    }

    fn on_env_event(&mut self, ev: &EnvEvent, cx: &mut Context<Self>) {
        match ev {
            EnvEvent::InterpreterChanged => {
                // ty resolves imports from the interpreter; ruff is looked for next to it.
                self.restart_ty(cx);
                self.relint_all(cx);
                self.sync_terminal_env(cx);
                if self.packages.is_some() {
                    self.refresh_packages(cx);
                }
                self.console_interpreter_changed(cx);
            }
            EnvEvent::ChoiceChanged => self.save_runs(cx),
            EnvEvent::Discovered => {
                self.relint_all(cx);
                if self.bottom == Some(super::BottomTool::Problems) && self.problems.tab == super::problems_ui::ProblemsTab::Project {
                    self.check_project(cx);
                }
            }
            EnvEvent::PackagesChanged => {
                if self.packages.is_some() {
                    self.refresh_packages(cx);
                }
            }
            EnvEvent::Status(s) => self.status = s.clone(),
            EnvEvent::Toast(msg, error) => self.show_toast(msg.clone(), *error, cx),
        }
        cx.notify();
    }

    /// Opens a popup that lists `PythonEnv` state (Interpreters, Add Interpreter, SSH
    /// Pythons, background tasks) on a fresh [`EnvView`].
    pub(super) fn open_env_popup(&mut self, kind: super::PopupKind, cx: &gpui::App) {
        self.env_view = self.env.read(cx).view();
        self.open_popup(kind);
    }

    // The workspace's entry points into the model.

    pub(super) fn start_interpreter_discovery(&mut self, cx: &mut Context<Self>) {
        self.env.update(cx, |e, cx| e.start_discovery(cx));
    }

    pub(super) fn remember_pyproject(&mut self, cx: &mut gpui::App) {
        self.env.update(cx, |e, _| e.remember_pyproject());
    }

    pub(super) fn pyproject_saved(&mut self, saved: &[PathBuf], cx: &mut Context<Self>) {
        self.env.update(cx, |e, cx| e.pyproject_saved(saved, cx));
    }

    pub(super) fn sync_env(&mut self, m: Manager, cx: &mut Context<Self>) {
        self.env.update(cx, |e, cx| e.sync_env(m, cx));
    }

    pub(super) fn create_venv(&mut self, base: PathBuf, cx: &mut Context<Self>) {
        self.env.update(cx, |e, cx| e.create_venv(base, cx));
    }

    #[cfg(feature = "smoke")]
    pub(super) fn install_requirements(&mut self, cx: &mut Context<Self>) {
        self.env.update(cx, |e, cx| e.install_requirements(cx));
    }

    pub(super) fn render_venv_offer(&self, cx: &mut Context<Self>) -> Option<gpui::AnyElement> {
        self.env.update(cx, |e, cx| e.render_offer(cx))
    }
}
