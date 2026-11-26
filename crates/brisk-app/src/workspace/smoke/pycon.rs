//! BRISK_SMOKE_PYCON=1 on an empty folder: "Run with Python Console" leaves a `>>>` prompt
//! with the script's variables after it ends; "Emulate terminal in output console" gives the
//! process a terminal (`isatty()` is true, `\r` rewrites the line).

use crate::workspace::Workspace;
use brisk_run::RunConfig;
use gpui::{Context, Focusable, Window};

impl Workspace {
    pub(in crate::workspace) fn pycon_smoke(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let root = self.root.clone();
        let _ = std::fs::write(root.join("con.py"), "x = 41\nprint('script done')\n");
        let _ = std::fs::write(
            root.join("tty.py"),
            "import sys\nprint('isatty', sys.stdout.isatty())\nfor i in range(3):\n    print(f'\\rprogress {i}', end='', flush=True)\nprint()\nsys.exit(3)\n",
        );
        cx.spawn_in(window, async move |this, cx| {
            let bg = cx.background_executor().clone();
            let pause = |ms| bg.timer(std::time::Duration::from_millis(ms));
            for _ in 0..100 {
                pause(100).await;
                if this.update(cx, |t, cx| t.interpreter(cx).is_some()).unwrap_or(false) {
                    break;
                }
            }
            let text = |this: &Workspace, cx: &gpui::App| this.runs.panel.as_ref().and_then(|p| p.read(cx).active_text()).unwrap_or_default();
            let wait_for = async |this: &gpui::WeakEntity<Workspace>, cx: &mut gpui::AsyncWindowContext, what: &str| {
                for _ in 0..100 {
                    pause(100).await;
                    if this.update(cx, |t, cx| text(t, cx).contains(what)).unwrap_or(false) {
                        return true;
                    }
                }
                false
            };
            // Run with Python Console: type at the prompt.
            let _ = this.update_in(cx, |this, window, cx| {
                this.run_config(RunConfig { name: "con".into(), target: "con.py".into(), python_console: true, ..Default::default() }, window, cx)
            });
            let prompt = wait_for(&this, cx, ">>>").await;
            let _ = this.update_in(cx, |this, window, cx| {
                let Some(p) = this.runs.panel.clone() else { return };
                window.focus(&p.read(cx).focus_handle(cx));
                for line in ["x + 1", "exit()"] {
                    for ch in line.chars() {
                        let k = gpui::Keystroke {
                            modifiers: gpui::Modifiers::default(),
                            key: if ch == ' ' { "space".into() } else { ch.to_string() },
                            key_char: Some(ch.to_string()),
                        };
                        p.update(cx, |p, cx| p.key(&k, cx));
                    }
                    let k = gpui::Keystroke { modifiers: gpui::Modifiers::default(), key: "enter".into(), key_char: Some("\r".into()) };
                    p.update(cx, |p, cx| p.key(&k, cx));
                }
            });
            let finished = wait_for(&this, cx, "Process finished").await;
            let _ = this.update(cx, |this, cx| {
                let t = text(this, cx);
                eprintln!(
                    "smoke-pycon: python console -> prompt {prompt}, finished {finished}, args -i {}, lines {:?}",
                    t.lines().next().unwrap_or("").contains(" -i "),
                    t.lines().skip(1).collect::<Vec<_>>()
                );
            });
            // Emulate terminal vs pipes.
            for emulate in [false, true] {
                let name = if emulate { "tty" } else { "pipe" };
                let _ = this.update_in(cx, |this, window, cx| {
                    this.run_config(RunConfig { name: name.into(), target: "tty.py".into(), emulate_terminal: emulate, ..Default::default() }, window, cx)
                });
                let done = wait_for(&this, cx, "Process finished").await;
                let _ = this.update(cx, |this, cx| {
                    let t = text(this, cx);
                    let lines: Vec<&str> = t.lines().skip(1).filter(|l| !l.is_empty()).collect();
                    eprintln!("smoke-pycon: emulate terminal {emulate} -> finished {done}, {lines:?}");
                });
            }
            let _ = this.update(cx, |_, cx| cx.quit());
        })
        .detach();
    }
}
