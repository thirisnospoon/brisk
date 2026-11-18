//! BRISK_SMOKE_PIN=1|hold on an empty folder: Pin Tab keeps a run's tab when the
//! configuration runs again, Rerun from a pinned tab reruns in it, and a run printing more
//! than [`brisk_terminal::RUN_HISTORY_LINES`] lines keeps only the newest ones.

use super::Workspace;
use gpui::{Context, Window};

impl Workspace {
    pub(super) fn pin_smoke(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let root = self.root.clone();
        let _ = std::fs::write(root.join("a.py"), "open('a.log', 'a').write('run\\n')\nprint('FIRST output')\n");
        let n = brisk_run::RunConfig { name: "big".into(), target: "big.py".into(), ..Default::default() };
        let _ = std::fs::write(root.join("big.py"), "import sys\nw = sys.stdout.write\nfor i in range(150000):\n    w(f'line {i}\\n')\n");
        cx.spawn_in(window, async move |this, cx| {
            let bg = cx.background_executor().clone();
            let pause = |ms| bg.timer(std::time::Duration::from_millis(ms));
            for _ in 0..100 {
                pause(100).await;
                if this.update(cx, |t, _| t.runs.interpreter(&t.root).is_some()).unwrap_or(false) {
                    break;
                }
            }
            let a = brisk_run::RunConfig { name: "a".into(), target: "a.py".into(), ..Default::default() };
            let run = |cfg: brisk_run::RunConfig| move |this: &mut Workspace, window: &mut Window, cx: &mut Context<Workspace>| this.run_config(cfg, window, cx);
            let _ = this.update_in(cx, run(a.clone()));
            pause(1500).await;
            // Pin it with a real click on the toolbar's pin button.
            let _ = this.update_in(cx, |_, window, _| match crate::terminal_view::pin_button_center() {
                Some((x, y)) => super::win_input::click(x * window.scale_factor(), y * window.scale_factor()),
                None => eprintln!("smoke-pin: no pin button"),
            });
            pause(500).await;
            let tabs = |this: &Workspace, cx: &gpui::App| this.runs.panel.as_ref().map(|p| p.read(cx).run_tabs_detail()).unwrap_or_default();
            let _ = this.update(cx, |this, cx| eprintln!("smoke-pin: after click {:?}", tabs(this, cx).iter().map(|t| (t.0.clone(), t.1)).collect::<Vec<_>>()));
            let _ = std::fs::write(root.join("a.py"), "open('a.log', 'a').write('run\\n')\nprint('SECOND output')\n");
            let _ = this.update_in(cx, run(a.clone()));
            pause(1500).await;
            let _ = this.update(cx, |this, cx| {
                let p = this.runs.panel.as_ref().unwrap().read(cx);
                eprintln!("smoke-pin: run again -> tabs {:?}, active shows SECOND {}", p.run_tabs_detail().iter().map(|t| (t.0.clone(), t.1)).collect::<Vec<_>>(), p.active_text().unwrap_or_default().contains("SECOND"));
            });
            // Rerun from the pinned tab reruns there (and the unpinned one is left alone).
            let _ = this.update_in(cx, |this, window, cx| {
                if let Some(p) = this.runs.panel.clone() {
                    let id = p.read(cx).runs_info().first().map(|r| r.id);
                    p.update(cx, |p, _| id.map(|id| p.select_run(id)));
                }
                this.dispatch(brisk_keymap::Action::Rerun, window, cx);
            });
            pause(1500).await;
            let _ = this.update(cx, |this, cx| {
                let p = this.runs.panel.as_ref().unwrap().read(cx);
                let runs = std::fs::read_to_string(this.root.join("a.log")).map(|s| s.lines().count()).unwrap_or(0);
                let info = p.runs_info();
                eprintln!(
                    "smoke-pin: rerun pinned -> tabs {:?}, active is the first {}, a.py ran {runs}x",
                    p.run_tabs_detail().iter().map(|t| (t.0.clone(), t.1)).collect::<Vec<_>>(),
                    info.first().is_some_and(|r| r.active)
                );
            });
            if std::env::var("BRISK_SMOKE_PIN").as_deref() == Ok("hold") {
                return;
            }
            let _ = this.update_in(cx, run(n));
            for _ in 0..300 {
                pause(200).await;
                let done = this.update(cx, |t, cx| t.runs.panel.as_ref().is_some_and(|p| p.read(cx).runs_info().iter().any(|r| r.name == "big" && !r.running))).unwrap_or(false);
                if done {
                    break;
                }
            }
            pause(500).await;
            let _ = this.update(cx, |this, cx| {
                let p = this.runs.panel.as_ref().unwrap().read(cx);
                let history = p.run_tabs_detail().into_iter().find(|t| t.0 == "big").map(|t| t.2);
                let text = p.active_text().unwrap_or_default();
                let first = text.lines().next().unwrap_or("").to_string();
                let last = text.lines().filter(|l| l.starts_with("line ")).last().unwrap_or("").to_string();
                eprintln!("smoke-pin: big output -> history {history:?} (limit {}), lines {}, first {first:?}, last {last:?}", brisk_terminal::RUN_HISTORY_LINES, text.lines().count());
            });
            let _ = this.update(cx, |_, cx| cx.quit());
        })
        .detach();
    }
}
