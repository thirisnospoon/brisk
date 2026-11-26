//! Before launch, like PyCharm's: other configurations and a shell command run first, one
//! after another, each in its own Run tab; a non-zero exit (or Stop) cancels the launch.

use super::Workspace;
#[cfg(feature = "smoke")]
use brisk_keymap::Action;
use brisk_run::{BeforeStep, RunConfig};
use gpui::{Context, Window};
use std::collections::VecDeque;
use std::path::PathBuf;

/// What starts once every step passed.
#[derive(Clone, Debug)]
pub enum Then {
    Run,
    Debug,
    Coverage(PathBuf),
    Profile,
    /// Rerun: the tab's own command (coverage / profile wrapping included).
    /// A Rerun of Run tab `.1`.
    Rerun(brisk_run::Command, u64),
}

pub struct Chain {
    cfg: RunConfig,
    then: Then,
    steps: VecDeque<BeforeStep>,
    /// The Run tab whose exit decides the next step.
    waiting: Option<String>,
}

/// The Run tab name of a configuration's Before launch command.
pub fn command_tab(cfg: &RunConfig) -> String {
    format!("Before launch: {}", cfg.name)
}

impl Workspace {
    /// Start `cfg`'s Before launch steps; `true` means the launch waits for them (or was
    /// refused) and the caller must not start it now.
    pub(super) fn before_launch(&mut self, cfg: &RunConfig, then: Then, window: &mut Window, cx: &mut Context<Self>) -> bool {
        if self.before_passed.as_deref() == Some(cfg.name.as_str()) {
            self.before_passed = None;
            return false;
        }
        if cfg.before_run.trim().is_empty() && cfg.before_command.trim().is_empty() {
            return false;
        }
        if let Some(c) = &self.before {
            self.show_toast(format!("Before launch of '{}' is still running", c.cfg.name), true, cx);
            return true;
        }
        self.reload_runs(cx);
        let steps = match self.runs.file.before_steps(cfg) {
            Ok(s) => s,
            Err(e) => {
                self.show_toast(e, true, cx);
                return true;
            }
        };
        self.before = Some(Chain { cfg: cfg.clone(), then, steps: steps.into(), waiting: None });
        self.before_next(window, cx);
        true
    }

    fn before_next(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let Some(chain) = self.before.as_mut() else { return };
        let Some(step) = chain.steps.pop_front() else {
            // Every step passed: launch.
            let Some(Chain { cfg, then, .. }) = self.before.take() else { return };
            self.before_passed = Some(cfg.name.clone());
            match then {
                Then::Run => self.run_config(cfg, window, cx),
                Then::Debug => self.start_debug(cfg, window, cx),
                Then::Coverage(python) => self.start_coverage_run(cfg, python, window, cx),
                Then::Profile => self.profile_config(cfg, window, cx),
                Then::Rerun(cmd, tab) => self.run_command_in(cfg, cmd, Some(tab), window, cx),
            }
            // A launch refused before it got to `before_launch` must not skip the next one.
            self.before_passed = None;
            return;
        };
        let cfg = chain.cfg.clone();
        let tab = match step {
            BeforeStep::Run(c) => {
                // The member runs as it is, without its own Before launch.
                let name = c.name.clone();
                self.before_passed = Some(name.clone());
                self.run_config(*c, window, cx);
                self.before_passed = None;
                name
            }
            BeforeStep::Command(line) => {
                let python = self.config_python(&cfg, cx).unwrap_or_else(|| PathBuf::from("python"));
                let cmd = brisk_run::shell_command(&self.root, &cfg, &python, &line);
                let tab = command_tab(&cfg);
                let tab_cfg = RunConfig { name: tab.clone(), target: line, ..Default::default() };
                self.run_command(tab_cfg, cmd, window, cx);
                tab
            }
        };
        // A step that did not start (no interpreter, a bad .env file…) cancels the launch.
        let started = self.runs.panel.as_ref().is_some_and(|p| p.read(cx).run_tabs().iter().any(|(n, running)| *n == tab && *running));
        if !started {
            self.before = None;
            self.status = format!("Before launch: '{tab}' did not start, '{}' was not started", cfg.name);
            return;
        }
        if let Some(c) = self.before.as_mut() {
            c.waiting = Some(tab.clone());
        }
        self.status = format!("Before launch of '{}': running '{tab}'", cfg.name);
    }

    /// A Run tab finished: the next Before launch step, or the end of the chain.
    pub(super) fn before_finished(&mut self, name: &str, code: Option<i32>, window: &mut Window, cx: &mut Context<Self>) {
        if self.before.as_ref().and_then(|c| c.waiting.as_deref()) != Some(name) {
            return;
        }
        if code == Some(0) {
            self.before_next(window, cx);
            return;
        }
        let Some(c) = self.before.take() else { return };
        let why = match code {
            Some(n) => format!("exited with code {n}"),
            None => "was stopped".into(),
        };
        let msg = format!("Before launch: '{name}' {why}, '{}' was not started", c.cfg.name);
        self.show_toast(msg.clone(), true, cx);
        self.status = msg;
    }

    // --- Smoke --------------------------------------------------------------------------

    /// BRISK_SMOKE_BEFORE=1 on an empty folder: `app` runs `prep` and a command first (both
    /// pass, then app starts and sees what they wrote); `bad` has a failing command and
    /// must not start; a Compound or missing name before launch is refused.
    #[cfg(feature = "smoke")]
    pub(super) fn before_smoke(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let root = self.root.clone();
        let w = |n: &str, s: &str| {
            let _ = std::fs::write(root.join(n), s);
        };
        w("prep.py", "open('prep.txt', 'w').write('prepared')\nprint('prep done')\n");
        w("app.py", "import os\nprint('app sees', open('prep.txt').read(), os.path.exists('cmd.txt'))\n");
        w("bad.py", "print('bad must not run')\n");
        let echo = if cfg!(windows) { "echo cmd> cmd.txt" } else { "echo cmd > cmd.txt" };
        let fail = if cfg!(windows) { "echo failing & exit /b 3" } else { "echo failing; exit 3" };
        let c = |name: &str, target: &str| RunConfig { name: name.into(), target: target.into(), ..Default::default() };
        let mut app = c("app", "app.py");
        app.before_run = "prep".into();
        app.before_command = echo.into();
        let mut bad = c("bad", "bad.py");
        bad.before_command = fail.into();
        let mut loops = c("loops", "app.py");
        loops.before_run = "both".into();
        let mut both = c("both", "prep;app");
        both.kind = brisk_run::RunKind::Compound;
        let file = brisk_run::RunFile { configurations: vec![c("prep", "prep.py"), app, bad, loops, both], ..Default::default() };
        let _ = brisk_run::save(&root, &file);
        self.reload_runs(cx);
        cx.spawn_in(window, async move |this, cx| {
            let bg = cx.background_executor().clone();
            let pause = |ms| bg.timer(std::time::Duration::from_millis(ms));
            for _ in 0..100 {
                pause(100).await;
                if this.update(cx, |t, cx| t.interpreter(cx).is_some()).unwrap_or(false) {
                    break;
                }
            }
            let tabs = |this: &Workspace, cx: &gpui::App| this.runs.panel.as_ref().map(|p| p.read(cx).run_tabs()).unwrap_or_default();
            let start = |name: &'static str| {
                move |this: &mut Workspace, window: &mut Window, cx: &mut Context<Workspace>| {
                    if let Some(cfg) = this.runs.file.get(name).cloned() {
                        this.run_config(cfg, window, cx);
                    }
                }
            };
            let _ = this.update_in(cx, start("app"));
            let _ = this.update(cx, |this, cx| eprintln!("smoke-before: app started -> tabs {:?}, status {:?}", tabs(this, cx), this.status));
            for _ in 0..200 {
                pause(100).await;
                if this.update(cx, |t, cx| t.before.is_none() && !tabs(t, cx).iter().any(|(_, r)| *r)).unwrap_or(false) {
                    break;
                }
            }
            let _ = this.update(cx, |this, cx| {
                let out = this.runs.panel.as_ref().and_then(|p| p.read(cx).active_text()).unwrap_or_default();
                let line = out.lines().find(|l| l.contains("app sees")).unwrap_or("-").to_string();
                eprintln!("smoke-before: after chain -> tabs {:?}, app output {line:?}", tabs(this, cx));
            });
            // Rerun (Ctrl+F5) on the app tab runs the Before launch steps again.
            let _ = std::fs::remove_file(root.join("prep.txt"));
            let _ = std::fs::remove_file(root.join("cmd.txt"));
            let _ = this.update_in(cx, |this, window, cx| {
                this.dispatch(Action::Rerun, window, cx);
                eprintln!("smoke-before: rerun app -> chain {}, status {:?}", this.before.is_some(), this.status);
            });
            for _ in 0..200 {
                pause(100).await;
                if this.update(cx, |t, cx| t.before.is_none() && !tabs(t, cx).iter().any(|(_, r)| *r)).unwrap_or(false) {
                    break;
                }
            }
            let _ = this.update(cx, |this, cx| {
                let out = this.runs.panel.as_ref().and_then(|p| p.read(cx).active_text()).unwrap_or_default();
                let line = out.lines().find(|l| l.contains("app sees")).unwrap_or("-").to_string();
                eprintln!("smoke-before: after rerun -> files {} {}, app output {line:?}", root.join("prep.txt").exists(), root.join("cmd.txt").exists());
            });
            let _ = this.update_in(cx, start("bad"));
            for _ in 0..100 {
                pause(100).await;
                if this.update(cx, |t, _| t.before.is_none()).unwrap_or(false) {
                    break;
                }
            }
            pause(300).await;
            let _ = this.update(cx, |this, cx| eprintln!("smoke-before: bad -> tabs {:?}, status {:?}", tabs(this, cx), this.status));
            let _ = this.update_in(cx, |this, window, cx| {
                start("loops")(this, window, cx);
                eprintln!("smoke-before: loops -> chain {}, toast {:?}", this.before.is_some(), this.toast.as_ref().map(|t| t.0.clone()));
                cx.quit();
            });
        })
        .detach();
    }
}
