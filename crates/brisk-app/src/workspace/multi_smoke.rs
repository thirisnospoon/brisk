//! BRISK_SMOKE_MULTI=1|dialog on an empty folder: "When started" (Show in Services, Focus Run
//! window, Don't show) and "Allow multiple instances" — without it a second run of a running
//! configuration asks Stop and Rerun / Cancel; with it both run.

use super::{BottomTool, Workspace};
use brisk_run::{OnStart, RunConfig};
use gpui::{Context, Focusable, Window};

impl Workspace {
    pub(super) fn multi_smoke(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let root = self.root.clone();
        let _ = std::fs::write(root.join("srv.py"), "import time\nopen('srv.log', 'a').write('run\\n')\nprint('serving', flush=True)\ntime.sleep(60)\n");
        let cfg = |name: &str, on_start: OnStart, multiple: bool| RunConfig { name: name.into(), target: "srv.py".into(), on_start, allow_multiple: multiple, ..Default::default() };
        if std::env::var("BRISK_SMOKE_MULTI").as_deref() == Ok("dialog") {
            let file = brisk_run::RunFile { configurations: vec![cfg("srv", OnStart::Services, true)], ..Default::default() };
            let _ = brisk_run::save(&root, &file);
            self.open_run_dialog(cx);
            return;
        }
        cx.spawn_in(window, async move |this, cx| {
            let bg = cx.background_executor().clone();
            let pause = |ms| bg.timer(std::time::Duration::from_millis(ms));
            for _ in 0..100 {
                pause(100).await;
                if this.update(cx, |t, _| t.runs.interpreter(&t.root).is_some()).unwrap_or(false) {
                    break;
                }
            }
            let tabs = |this: &Workspace, cx: &gpui::App| this.runs_list(cx).iter().map(|r| format!("{}{}", r.name, if r.running { "▶" } else { "" })).collect::<Vec<_>>();
            let runs = |root: &std::path::Path| std::fs::read_to_string(root.join("srv.log")).map(|s| s.lines().count()).unwrap_or(0);
            // Show in Services.
            let _ = this.update_in(cx, |this, window, cx| this.run_config(cfg("srv", OnStart::Services, false), window, cx));
            pause(1500).await;
            let _ = this.update(cx, |this, cx| {
                let sel = this.runs_list(cx).into_iter().find(|r| Some(super::services_ui::Item::Run(r.id)) == this.services.selected).map(|r| r.name);
                eprintln!("smoke-multi: services -> bottom {:?}, selected {sel:?}", this.bottom);
            });
            // Again while it runs: the question, then Stop and Rerun.
            let _ = this.update_in(cx, |this, window, cx| {
                this.run_config(cfg("srv", OnStart::Services, false), window, cx);
                let items: Vec<String> = this.popup.as_ref().map(|p| p.items.iter().map(|i| i.label.clone()).collect()).unwrap_or_default();
                eprintln!("smoke-multi: run again -> popup {items:?}, srv.py ran {}x", runs(&this.root));
                this.smoke_press("enter", window, cx);
            });
            pause(2000).await;
            let _ = this.update(cx, |this, cx| eprintln!("smoke-multi: stop and rerun -> tabs {:?}, srv.py ran {}x, popup {}", tabs(this, cx), runs(&this.root), this.popup.is_some()));
            // Cancel leaves it running.
            let _ = this.update_in(cx, |this, window, cx| {
                this.run_config(cfg("srv", OnStart::Services, false), window, cx);
                this.smoke_press("escape", window, cx);
            });
            pause(800).await;
            let _ = this.update(cx, |this, cx| eprintln!("smoke-multi: cancel -> tabs {:?}, srv.py ran {}x", tabs(this, cx), runs(&this.root)));
            // Allow multiple instances: two run side by side.
            for _ in 0..2 {
                let _ = this.update_in(cx, |this, window, cx| this.run_config(cfg("par", OnStart::Show, true), window, cx));
                pause(800).await;
            }
            let _ = this.update(cx, |this, cx| eprintln!("smoke-multi: multiple -> tabs {:?}, popup {}, bottom {:?}", tabs(this, cx), this.popup.is_some(), this.bottom));
            // Focus Run window, then Don't show.
            let _ = this.update_in(cx, |this, window, cx| {
                this.focus_editor(window, cx);
                this.run_config(cfg("foc", OnStart::Focus, false), window, cx);
                let focused = this.runs.panel.as_ref().is_some_and(|p| p.read(cx).focus_handle(cx).is_focused(window));
                eprintln!("smoke-multi: focus -> bottom {:?}, console focused {focused}", this.bottom);
                this.bottom = None;
                this.run_config(cfg("quiet", OnStart::Nothing, false), window, cx);
                eprintln!("smoke-multi: don't show -> bottom {:?}, running {:?}", this.bottom, this.runs_list(cx).iter().any(|r| r.name == "quiet"));
            });
            let _ = this.update(cx, |this, cx| {
                if let Some(p) = this.runs.panel.clone() {
                    p.update(cx, |p, _| p.stop_all());
                }
            });
            pause(800).await;
            let _ = this.update(cx, |this, cx| {
                eprintln!("smoke-multi: done, services open {}", this.bottom == Some(BottomTool::Services));
                cx.quit();
            });
        })
        .detach();
    }
}
