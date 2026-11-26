//! The Terminal follows the project interpreter: new shells start with its venv or conda
//! environment activated (see `terminal_env`).

#[cfg(feature = "smoke")]
use super::BottomTool;
use super::Workspace;
#[cfg(feature = "smoke")]
use brisk_keymap::Action;
use brisk_run::env::terminal::{self as terminal_env, Activation};
use gpui::Context;
#[cfg(feature = "smoke")]
use gpui::Window;

#[cfg(feature = "smoke")]
fn terminal_text(t: &Workspace, cx: &gpui::App) -> String {
    t.terminal.as_ref().and_then(|p| p.read(cx).active_text()).unwrap_or_default()
}

/// Wait until the active terminal tab shows `needle` (up to `ms`).
#[cfg(feature = "smoke")]
async fn wait_text(this: &gpui::WeakEntity<Workspace>, cx: &mut gpui::AsyncWindowContext, needle: &str, ms: u32) -> bool {
    let mut waited = 0;
    while waited < ms {
        cx.background_executor().timer(std::time::Duration::from_millis(100)).await;
        waited += 100;
        if this.update(cx, |t, cx| terminal_text(t, cx).contains(needle)).unwrap_or(false) {
            return true;
        }
    }
    false
}

impl Workspace {
    /// The activation for new shells; `None` while the interpreter is unknown.
    pub(super) fn terminal_activation(&self) -> Option<Activation> {
        self.runs.interpreter(&self.root).map(|i| terminal_env::activation(&i.path, i.kind))
    }

    /// After the project interpreter changed: shells opened from now on use it.
    pub(super) fn sync_terminal_env(&mut self, cx: &mut Context<Self>) {
        let a = self.terminal_activation();
        if let Some(t) = &self.terminal {
            t.update(cx, |t, _| t.set_activation(a));
        }
    }

    // --- Smoke --------------------------------------------------------------------------

    /// BRISK_SMOKE_TERMVENV=1|hold on a folder with a `.venv` (made with the first base Python
    /// if missing): opens the Terminal and checks the prompt, VIRTUAL_ENV and which python
    /// runs; then switches the interpreter to a system one and checks a new tab is plain.
    #[cfg(feature = "smoke")]
    pub(super) fn terminal_env_smoke(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        cx.spawn_in(window, async move |this, cx| {
            let bg = cx.background_executor().clone();
            let pause = |ms| bg.timer(std::time::Duration::from_millis(ms));
            for _ in 0..150 {
                pause(100).await;
                if this.update(cx, |t, _| t.runs.all.is_some()).unwrap_or(false) {
                    break;
                }
            }
            // A .venv in the project.
            let (root, base) = this
                .update(cx, |t, _| (t.root.clone(), t.runs.all.iter().flatten().find(|i| i.kind != brisk_run::InterpKind::Venv).map(|i| i.path.clone())))
                .unwrap_or_default();
            if brisk_run::venv_python(&root.join(".venv")).is_none() {
                if let Some(base) = base.clone() {
                    let dir = root.join(".venv");
                    let r = bg.spawn(async move { brisk_run::env::venv::create(&base, &dir) }).await;
                    eprintln!("smoke-termvenv: created .venv {:?}", r.map(|p| p.display().to_string()));
                }
            }
            let _ = this.update_in(cx, |this, window, cx| {
                this.runs.venvs = brisk_run::project_venvs(&this.root);
                eprintln!("smoke-termvenv: interpreter {:?}", this.interpreter_label());
                eprintln!("smoke-termvenv: activation prompt {:?}", this.terminal_activation().and_then(|a| a.prompt));
                this.dispatch(Action::ToggleTerminalWindow, window, cx);
            });
            let prompt_ok = wait_text(&this, cx, "(.venv) PS ", 20_000).await;
            let _ = this.update(cx, |t, cx| {
                if let Some(p) = &t.terminal {
                    p.read(cx).type_line("python -c \"import sys; print('exe=' + sys.executable)\"; echo \"venv=$env:VIRTUAL_ENV\"");
                }
            });
            let exe_ok = wait_text(&this, cx, "Scripts\\python.exe", 20_000).await;
            let _ = wait_text(&this, cx, "venv=C:", 10_000).await;
            let out = this.update(cx, |t, cx| terminal_text(t, cx)).unwrap_or_default();
            let line = |key: &str| out.lines().find(|l| l.starts_with(key)).unwrap_or("").to_string();
            eprintln!("smoke-termvenv: prompt (.venv) {prompt_ok}, python in venv {exe_ok}");
            eprintln!("smoke-termvenv: {:?} {:?}", line("exe="), line("venv="));
            if std::env::var("BRISK_SMOKE_TERMVENV").as_deref() == Ok("hold") {
                return;
            }
            // A system interpreter: a new tab gets no activation.
            let _ = this.update_in(cx, |this, window, cx| {
                if let Some(base) = base.clone() {
                    this.run_popup_pick(super::Target::Interpreter(base), window, cx);
                }
                eprintln!("smoke-termvenv: switched to {:?}", this.interpreter_label());
                if let Some(t) = &this.terminal {
                    t.update(cx, |t, _| t.new_tab());
                }
                this.bottom = Some(BottomTool::Terminal);
            });
            pause(3000).await;
            let _ = this.update(cx, |t, cx| {
                if let Some(p) = &t.terminal {
                    p.read(cx).type_line("echo \"venv=[$env:VIRTUAL_ENV]\"");
                }
            });
            let plain = wait_text(&this, cx, "venv=[]", 15_000).await;
            let out = this.update(cx, |t, cx| terminal_text(t, cx)).unwrap_or_default();
            eprintln!("smoke-termvenv: new tab after switch: VIRTUAL_ENV empty {plain}, prompt has (.venv) {}", out.contains("(.venv) PS "));
            let _ = this.update(cx, |_, cx| cx.quit());
        })
        .detach();
    }
}
