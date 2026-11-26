//! The project's Python environment: the interpreters found (project venvs at once, the rest
//! of the machine in the background), the project interpreter chosen in the status bar,
//! the SSH interpreters added to the project, and the state of creating or syncing an
//! environment (Add Interpreter, uv / Poetry).
//!
//! `run.json` keeps the choice (`interpreter`, `sshInterpreters`) next to the run
//! configurations; [`PythonEnv`] owns it while the project is open, and `Workspace::save_runs`
//! writes it back into the file.

use super::venv_ui::VenvOffer;
use brisk_run::ssh::{self, HostInfo, SshInterpreter};
use brisk_run::{InterpKind, Interpreter};
use std::path::{Path, PathBuf};

pub struct PythonEnv {
    root: PathBuf,
    /// The project interpreter as `run.json` keeps it: a path (relative to the project when
    /// inside it) or an `ssh://` URI. `None` picks the project's virtualenv automatically.
    pub selected: Option<String>,
    /// SSH interpreters added to the project.
    pub ssh: Vec<SshInterpreter>,
    /// Project virtualenvs, known instantly from `pyvenv.cfg`.
    pub venvs: Vec<Interpreter>,
    /// Everything found on the machine; `None` while discovery runs in the background.
    pub all: Option<Vec<Interpreter>>,
    /// A project venv is being created or synced (Add Interpreter, uv / Poetry).
    pub busy: bool,
    /// The offer to install requirements into a new venv, or to sync after a pyproject change.
    pub offer: Option<VenvOffer>,
    /// `pyproj::deps_fingerprint` of pyproject.toml when the project opened or was last synced.
    pub pyproject_fp: Option<u64>,
    /// Add SSH Interpreter: the host just probed and what it has.
    pub ssh_found: Option<(String, HostInfo)>,
}

impl PythonEnv {
    /// `selected` and `ssh` as read from `run.json`.
    pub fn new(root: PathBuf, selected: Option<String>, ssh: Vec<SshInterpreter>) -> Self {
        let venvs = brisk_run::project_venvs(&root);
        PythonEnv { root, selected, ssh, venvs, all: None, busy: false, offer: None, pyproject_fp: None, ssh_found: None }
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

    /// The project's SSH interpreter, when one is selected.
    pub fn ssh(&self) -> Option<SshInterpreter> {
        let uri = self.selected.as_deref()?;
        self.ssh.iter().find(|s| s.uri() == uri).cloned()
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
    /// [`PythonEnv::tasks`].
    pub tasks: Vec<&'static str>,
}

impl PythonEnv {
    pub fn view(&self) -> EnvView {
        EnvView {
            known: self.known().cloned().collect(),
            machine: self.all.clone(),
            current: self.interpreter().map(|i| i.path),
            selected: self.selected.clone(),
            ssh_selected: self.ssh().is_some(),
            ssh: self.ssh.clone(),
            ssh_found: self.ssh_found.clone(),
            busy: self.busy,
            tasks: self.tasks(),
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

impl super::Workspace {
    /// Opens a popup that lists `PythonEnv` state (Interpreters, Add Interpreter, SSH
    /// Pythons, background tasks) on a fresh [`EnvView`].
    pub(super) fn open_env_popup(&mut self, kind: super::PopupKind, cx: &gpui::App) {
        self.env_view = self.env.read(cx).view();
        self.open_popup(kind);
    }
}
