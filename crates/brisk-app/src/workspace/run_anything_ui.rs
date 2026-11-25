//! Run Anything (double Ctrl): the popup and the run. Parsing lives in `run_anything`.

use super::{PopupItem, PopupKind, Target, Workspace};
use crate::run_anything::{self, Parsed};
use brisk_run::RunConfig;
use gpui::{Context, Window};

impl Workspace {
    pub(super) fn open_run_anything(&mut self) {
        self.reload_runs();
        self.ra_history = run_anything::load_history(&self.root);
        self.open_popup(PopupKind::RunAnything);
    }

    /// The typed command first, then matching history and run configurations.
    pub(super) fn run_anything_items(&self, q: &str) -> Vec<PopupItem> {
        let q = q.trim();
        let ql = q.to_lowercase();
        let item =
            |label: &str, detail: String| PopupItem { label: label.to_string(), detail, hint: String::new(), target: Target::RunAnything(label.to_string()) };
        let mut v = Vec::new();
        if !q.is_empty() {
            if self.runs.file.get(q).is_some() {
                v.push(item(q, "run configuration".into()));
            } else if let Some(p) = run_anything::parse(&self.root, q) {
                v.push(item(q, p.describe().into()));
            }
        }
        for h in self.ra_history.iter().filter(|h| h.as_str() != q && h.to_lowercase().contains(&ql)) {
            v.push(item(h, "recent".into()));
        }
        for c in self.runs.file.configurations.iter().filter(|c| c.name != q && c.name.to_lowercase().contains(&ql)) {
            v.push(item(&c.name, "run configuration".into()));
        }
        if v.is_empty() {
            v.push(PopupItem {
                label: "Type a command: python x.py, pytest …, pip install …".into(),
                detail: "or the name of a run configuration".into(),
                hint: String::new(),
                target: Target::Cancel,
            });
        }
        v
    }

    /// Run a typed line in the Run window and remember it.
    pub(super) fn run_anything(&mut self, line: String, window: &mut Window, cx: &mut Context<Self>) {
        let line = line.trim().to_string();
        if line.is_empty() {
            return;
        }
        run_anything::remember(&self.root, &mut self.ra_history, &line);
        self.focus_editor(window, cx);
        if let Some(cfg) = self.runs.file.get(&line).cloned() {
            self.run_config(cfg, window, cx);
            return;
        }
        match run_anything::parse(&self.root, &line) {
            Some(Parsed::Config(cfg)) => self.run_config(cfg, window, cx),
            Some(Parsed::Shell(cmdline)) => {
                self.save_all(cx);
                let env = self.terminal_activation().map(|a| a.env).unwrap_or_default();
                let cmd = run_anything::shell_command(&self.root, &cmdline, env);
                // The configuration only names the tab and lets Rerun repeat the command.
                let cfg = RunConfig { name: line.clone(), target: cmdline, ..RunConfig::default() };
                self.run_command(cfg, cmd, window, cx);
            }
            None => {}
        }
    }

    // --- Smoke --------------------------------------------------------------------------

    /// BRISK_SMOKE_RUNANYTHING=1|hold on an empty folder: writes `hello.py`, `test_ra.py` and
    /// a run configuration, opens Run Anything by double Ctrl and runs a script, pytest,
    /// pip, a shell command and the configuration; checks the history and that Ctrl, Ctrl+↑
    /// (clone caret) does not open it.
    #[cfg(feature = "smoke")]
    pub(super) fn run_anything_smoke(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let root = self.root.clone();
        let _ = std::fs::write(root.join("hello.py"), "import sys\nprint('argv:', sys.argv[1:])\n");
        let _ = std::fs::write(root.join("test_ra.py"), "def test_one():\n    assert True\n\ndef test_two():\n    assert 1 == 2\n");
        let _ = std::fs::create_dir_all(root.join(".brisk"));
        let _ = std::fs::write(
            root.join(brisk_run::CONFIG_FILE),
            "{\n  \"configurations\": [{\"name\": \"Hello cfg\", \"target\": \"hello.py\", \"parameters\": \"cfg\"}]\n}\n",
        );
        let _ = std::fs::remove_file(root.join(run_anything::HISTORY_FILE));
        self.reload_runs();
        self.open_file(&root.join("hello.py"), window, cx);
        cx.spawn_in(window, async move |this, cx| {
            let bg = cx.background_executor().clone();
            let pause = |ms| bg.timer(std::time::Duration::from_millis(ms));
            for _ in 0..100 {
                pause(100).await;
                if this.update(cx, |t, _| t.runs.interpreter(&t.root).is_some()).unwrap_or(false) {
                    break;
                }
            }
            let mods = |control: bool| gpui::ModifiersChangedEvent { modifiers: gpui::Modifiers { control, ..Default::default() }, ..Default::default() };
            let steps: [(&str, &str); 5] = [
                ("python hello.py a \"b c\"", "argv: ['a', 'b c']"),
                ("pytest -q test_ra.py", "1 failed, 1 passed"),
                ("pip --version", " from "),
                ("echo brisk-%OS%", "brisk-Windows_NT"),
                ("Hello cfg", "argv: ['cfg']"),
            ];
            for (i, (line, expect)) in steps.iter().enumerate() {
                let _ = this.update_in(cx, |this, window, cx| {
                    for down in [true, false, true, false] {
                        this.on_modifiers(&mods(down), window, cx);
                    }
                    let open = this.popup.as_ref().map(|p| p.kind) == Some(PopupKind::RunAnything);
                    if i == 0 || i == 4 {
                        let labels: Vec<String> =
                            this.popup.as_ref().map(|p| p.items.iter().map(|i| format!("{} | {}", i.label, i.detail)).collect()).unwrap_or_default();
                        eprintln!("smoke-runanything: double ctrl -> open {open}, items {labels:?}");
                    }
                    if let Some(p) = this.popup.as_mut() {
                        p.query = line.to_string();
                        p.selected = 0;
                    }
                    this.refresh_popup();
                    let first = this.popup.as_ref().and_then(|p| p.items.first()).map(|i| i.detail.clone());
                    eprintln!("smoke-runanything: {line:?} -> {first:?}");
                    this.smoke_press("enter", window, cx);
                });
                let output = |t: &Workspace, cx: &gpui::App| t.runs.panel.as_ref().and_then(|p| p.read(cx).active_text()).unwrap_or_default();
                let mut waited = 0;
                while waited < 60_000 {
                    pause(100).await;
                    waited += 100;
                    if this.update(cx, |t, cx| output(t, cx).contains(expect)).unwrap_or(false) {
                        break;
                    }
                }
                let _ = this.update(cx, |this, cx| {
                    let out = output(this, cx);
                    eprintln!("smoke-runanything: {line:?} output has {expect:?}: {} (after {waited} ms)", out.contains(expect));
                    if !out.contains(expect) {
                        eprintln!("smoke-runanything: output was {:?}, status {:?}", out.chars().take(600).collect::<String>(), this.status);
                    }
                });
                pause(300).await;
            }
            let hold = std::env::var("BRISK_SMOKE_RUNANYTHING").as_deref() == Ok("hold");
            let _ = this.update_in(cx, |this, window, cx| {
                let tabs = this.runs.panel.as_ref().map(|p| p.read(cx).run_tabs().into_iter().map(|t| t.0).collect::<Vec<_>>()).unwrap_or_default();
                eprintln!("smoke-runanything: run tabs {tabs:?}");
                eprintln!("smoke-runanything: history file {:?}", run_anything::load_history(&this.root));
                // Ctrl, then Ctrl held with ↑ (clone caret): not Run Anything.
                this.on_modifiers(&mods(true), window, cx);
                this.on_modifiers(&mods(false), window, cx);
                this.on_modifiers(&mods(true), window, cx);
                this.smoke_press("ctrl-up", window, cx);
                this.on_modifiers(&mods(false), window, cx);
                eprintln!("smoke-runanything: ctrl, ctrl+up -> popup {:?}", this.popup.as_ref().map(|p| p.kind));
                if hold {
                    this.open_run_anything();
                    if let Some(p) = this.popup.as_mut() {
                        p.query = "py".into();
                    }
                    this.refresh_popup();
                    cx.notify();
                } else {
                    cx.quit();
                }
            });
        })
        .detach();
    }
}
