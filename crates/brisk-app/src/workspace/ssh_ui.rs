//! SSH interpreters in the UI: Add SSH Interpreter (a host from `~/.ssh/config` or typed,
//! then one of the Pythons found there), choosing one in the interpreter popup, and runs
//! that upload the project and execute on the host (see `brisk_run::ssh`).

use super::{PopupItem, PopupKind, Target, Workspace};
use brisk_run::ssh::{self, HostInfo, SshInterpreter};
use brisk_run::{Command, RunConfig};
use gpui::{Context, Window};

impl Workspace {
    /// The SSH interpreter a configuration runs with: its own, else the project's.
    pub(super) fn ssh_for(&self, cfg: &RunConfig, cx: &gpui::App) -> Option<SshInterpreter> {
        match cfg.interpreter.as_deref() {
            Some(p) if p.starts_with(ssh::SCHEME) => self.env.read(cx).ssh_by_uri(p),
            Some(_) => None,
            None => self.env.read(cx).ssh(),
        }
    }

    /// Debug, coverage and profiling run locally: with an SSH interpreter they say so instead
    /// of quietly using a local Python. True when blocked.
    pub(super) fn remote_blocks(&mut self, cfg: &RunConfig, what: &str, cx: &mut Context<Self>) -> bool {
        let Some(s) = self.ssh_for(cfg, cx) else { return false };
        self.show_toast(
            format!("{what} is not available with an SSH interpreter yet ({}): run it, or pick a local interpreter in the status bar", s.label()),
            true,
            cx,
        );
        true
    }

    /// Run with an SSH interpreter: the same command as locally, wrapped in `ssh`. The upload
    /// happens in [`Self::run_remote_command`], so a Rerun uploads too.
    pub(super) fn run_remote(&mut self, cfg: RunConfig, remote: SshInterpreter, window: &mut Window, cx: &mut Context<Self>) {
        if !self.env_files_ok(&cfg, cx) {
            return;
        }
        let local = brisk_run::build(&self.root, &cfg, std::path::Path::new(&remote.python));
        let cmd = ssh::wrap(&self.root, &remote, &cfg, &local);
        self.run_command(cfg, cmd, window, cx);
    }

    /// A remote command: upload what changed, then start it. False for a local command.
    pub(super) fn run_remote_command(&mut self, cfg: RunConfig, cmd: Command, tab: Option<u64>, window: &mut Window, cx: &mut Context<Self>) -> bool {
        let Some((host, local, rr)) = ssh::sync_target(&cmd) else { return false };
        self.status = format!("Uploading the project to {host}…");
        cx.notify();
        let task = cx.background_executor().spawn(async move { ssh::sync(&host, &local, &rr).map(|s| (host, s)) });
        cx.spawn_in(window, async move |this, cx| {
            let res = task.await;
            let _ = this.update_in(cx, |this, window, cx| {
                match res {
                    Ok((host, s)) => {
                        this.run_command_now(cfg, cmd, tab, window, cx);
                        if s.uploaded + s.deleted > 0 {
                            this.status = format!("Uploaded {} file(s) ({} KB) to {host}, removed {}; running", s.uploaded, s.bytes.div_ceil(1024), s.deleted);
                        }
                    }
                    Err(e) => {
                        this.status = format!("Upload failed: {e}");
                        this.show_toast(format!("Cannot upload the project: {e}"), true, cx);
                    }
                }
                cx.notify();
            });
        })
        .detach();
        true
    }

    /// The interpreter popup's rows for SSH interpreters, and "Add SSH Interpreter…".
    pub(super) fn ssh_interpreter_items(&self, ql: &str) -> Vec<PopupItem> {
        let current = self.env_view.selected.clone();
        let mut v: Vec<PopupItem> = self
            .env_view
            .ssh
            .iter()
            .filter(|s| s.label().to_lowercase().contains(ql) || s.uri().to_lowercase().contains(ql))
            .map(|s| PopupItem {
                label: s.label(),
                detail: format!("{} · project in {}", s.uri(), s.remote_root),
                hint: if current.as_deref() == Some(s.uri().as_str()) { "✓".into() } else { String::new() },
                target: Target::SshUse(s.uri()),
            })
            .collect();
        if "add ssh interpreter".contains(ql) {
            v.push(PopupItem {
                label: "Add SSH Interpreter…".into(),
                detail: "a Python on another machine, through ssh".into(),
                hint: String::new(),
                target: Target::AddSsh,
            });
        }
        v
    }

    /// Hosts: `~/.ssh/config` aliases, and what is typed (`user@host` or `user@host:port`).
    pub(super) fn ssh_host_items(&self, q: &str) -> Vec<PopupItem> {
        let ql = q.trim().to_lowercase();
        let hosts = ssh::config_hosts();
        let mut v: Vec<PopupItem> = hosts
            .iter()
            .filter(|h| h.to_lowercase().contains(&ql))
            .map(|h| PopupItem { label: h.clone(), detail: "~/.ssh/config".into(), hint: String::new(), target: Target::SshHost(h.clone()) })
            .collect();
        let typed = q.trim();
        if !typed.is_empty() && !hosts.iter().any(|h| h == typed) && !typed.contains(char::is_whitespace) {
            v.insert(
                0,
                PopupItem {
                    label: format!("Connect to {typed}"),
                    detail: "user@host or user@host:port".into(),
                    hint: "Enter".into(),
                    target: Target::SshHost(typed.to_string()),
                },
            );
        }
        if v.is_empty() {
            v.push(PopupItem {
                label: "Type user@host".into(),
                detail: "keys and the ssh agent are used; passwords are not supported".into(),
                hint: String::new(),
                target: Target::Cancel,
            });
        }
        v
    }

    pub(super) fn ssh_python_items(&self, q: &str) -> Vec<PopupItem> {
        let ql = q.to_lowercase();
        let Some((host, info)) = &self.env_view.ssh_found else { return Vec::new() };
        let mut v: Vec<PopupItem> = info
            .pythons
            .iter()
            .filter(|(p, _)| p.to_lowercase().contains(&ql))
            .map(|(p, ver)| PopupItem {
                label: format!("Python {ver}"),
                detail: p.clone(),
                hint: String::new(),
                target: Target::SshPython(host.clone(), p.clone(), ver.clone()),
            })
            .collect();
        if v.is_empty() {
            v.push(PopupItem {
                label: format!("No Python found on {host}"),
                detail: "install python3 there".into(),
                hint: String::new(),
                target: Target::Cancel,
            });
        }
        v
    }

    /// A picked host: connect and list its Pythons.
    pub(super) fn ssh_pick_host(&mut self, host: String, window: &mut Window, cx: &mut Context<Self>) {
        self.popup = None;
        self.status = format!("Connecting to {host}…");
        let h = host.clone();
        let task = cx.background_executor().spawn(async move { ssh::probe(&h) });
        cx.spawn_in(window, async move |this, cx| {
            let res: Result<HostInfo, String> = task.await;
            let _ = this.update_in(cx, |this, _, cx| {
                match res {
                    Ok(info) => {
                        this.status = format!("{host}: {} Python interpreter(s)", info.pythons.len());
                        this.env.update(cx, |e, _| e.ssh_found = Some((host, info)));
                        this.open_env_popup(PopupKind::SshPythons, cx);
                    }
                    Err(e) => {
                        this.status = format!("Cannot connect to {host}: {e}");
                        this.show_toast(format!("Cannot connect to {host}: {e}"), true, cx);
                    }
                }
                cx.notify();
            });
        })
        .detach();
        cx.notify();
    }

    /// A picked Python: becomes the project interpreter, with the project folder on the host
    /// at `~/brisk-projects/<name>`.
    pub(super) fn ssh_pick_python(&mut self, host: String, python: String, version: String, window: &mut Window, cx: &mut Context<Self>) {
        self.popup = None;
        let home = self.env.update(cx, |e, _| e.ssh_found.take()).map(|(_, i)| i.home).unwrap_or_default();
        self.reload_runs(cx);
        let existing = self.env.read(cx).ssh_interpreters().iter().find(|s| s.host == host).map(|s| s.remote_root.clone());
        let remote_root = existing.unwrap_or_else(|| ssh::default_remote_root(&home, &self.root));
        let s = SshInterpreter { host, python, version, remote_root };
        self.env.update(cx, |e, cx| e.add_ssh(s.clone(), cx));
        self.status = format!("Interpreter: {}", s.label());
        self.show_toast(format!("Runs use {} on {}. The project is uploaded to {} before each run.", s.label(), s.host, s.remote_root), false, cx);
        self.focus_editor(window, cx);
        cx.notify();
    }

    pub(super) fn ssh_use(&mut self, uri: String, window: &mut Window, cx: &mut Context<Self>) {
        self.popup = None;
        self.env.update(cx, |e, cx| e.select(Some(uri), cx));
        self.status = format!("Interpreter: {}", self.interpreter_label(cx));
        self.focus_editor(window, cx);
        cx.notify();
    }
}
