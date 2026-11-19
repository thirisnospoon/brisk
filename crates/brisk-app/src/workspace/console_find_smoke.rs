//! BRISK_SMOKE_CONSOLEFIND=1|hold on an empty folder: a run prints 300 lines with ten
//! "error" lines and a long line, then Ctrl+F, typing, Enter / Shift+Enter / F3, Match Case
//! (Alt+C), Filter, a click on a filtered line, a match beyond the edge with Soft-wrap off,
//! and Escape go through the Run console's search.

use super::Workspace;
use gpui::{Context, Focusable, Window};

impl Workspace {
    pub(super) fn console_find_smoke(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let root = self.root.clone();
        let _ = std::fs::write(
            root.join("out.py"),
            "for i in range(300):\n    print(f'ERROR item {i}' if i % 37 == 0 else f'line {i}')\nprint('an error in lowercase')\nprint('x' * 250 + ' needle')\nprint('the end')\n",
        );
        cx.spawn_in(window, async move |this, cx| {
            let bg = cx.background_executor().clone();
            let pause = |ms| bg.timer(std::time::Duration::from_millis(ms));
            for _ in 0..100 {
                pause(100).await;
                if this.update(cx, |t, _| t.runs.interpreter(&t.root).is_some()).unwrap_or(false) {
                    break;
                }
            }
            let _ = this.update_in(cx, |this, window, cx| {
                this.run_config(brisk_run::RunConfig { name: "out".into(), target: "out.py".into(), ..Default::default() }, window, cx);
            });
            for _ in 0..100 {
                pause(100).await;
                if this.update(cx, |t, cx| t.runs.panel.as_ref().is_some_and(|p| p.read(cx).run_tabs().iter().all(|(_, r)| !r))).unwrap_or(false) {
                    break;
                }
            }
            pause(300).await;
            let show = |this: &Workspace, cx: &gpui::App, what: &str| {
                if let Some(p) = &this.runs.panel {
                    let p = p.read(cx);
                    eprintln!("smoke-cfind: {what} -> open {} summary {:?} {}", p.search_open(), p.search_summary(), p.search_probe());
                }
            };
            let _ = this.update_in(cx, |this, window, cx| {
                if let Some(p) = this.runs.panel.clone() {
                    let f = p.read(cx).focus_handle(cx);
                    window.focus(&f);
                }
                this.smoke_press("ctrl-f", window, cx);
                show(this, cx, "ctrl+f");
                if let Some(p) = this.runs.panel.clone() {
                    p.update(cx, |p, _| p.search_type("error"));
                }
                show(this, cx, "typed error");
                this.smoke_press("enter", window, cx);
                show(this, cx, "enter");
                this.smoke_press("shift-enter", window, cx);
                show(this, cx, "shift+enter");
                this.smoke_press("f3 f3 f3", window, cx);
                show(this, cx, "f3 x3");
                this.smoke_press("alt-c", window, cx);
                show(this, cx, "alt+c (match case)");
                this.smoke_press("alt-c", window, cx);
                if let Some(p) = this.runs.panel.clone() {
                    p.update(cx, |p, _| p.toggle_search_filter());
                    let lines = p.read(cx).filtered_lines();
                    eprintln!("smoke-cfind: filter -> {} lines, first {:?}, last {:?}", lines.len(), lines.first().map(|l| &l.1), lines.last().map(|l| &l.1));
                    if let Some(&(line, _)) = lines.get(4) {
                        p.update(cx, |p, _| p.open_filtered(line));
                    }
                }
                show(this, cx, "click 5th filtered line");
            });
            if std::env::var("BRISK_SMOKE_CONSOLEFIND").as_deref() == Ok("hold") {
                return;
            }
            // A match past the right edge with Soft-wrap off.
            let _ = this.update_in(cx, |this, window, cx| {
                this.smoke_press("ctrl-backspace", window, cx);
                if let Some(p) = this.runs.panel.clone() {
                    p.update(cx, |p, _| {
                        p.toggle_soft_wrap();
                        p.search_type("needle");
                    });
                }
                cx.notify();
            });
            pause(400).await;
            let _ = this.update_in(cx, |this, window, cx| {
                this.smoke_press("f3", window, cx);
                show(this, cx, "needle, soft-wrap off, f3");
                // Soft-wrap back on: a word broken at the right edge is still found.
                if let Some(p) = this.runs.panel.clone() {
                    p.update(cx, |p, _| p.toggle_soft_wrap());
                }
                cx.notify();
            });
            pause(400).await;
            let cols = this.update(cx, |t, cx| t.runs.panel.as_ref().map(|p| p.read(cx).columns()).unwrap_or(80)).unwrap_or(80);
            let _ = std::fs::write(root.join("split.py"), format!("print('.' * {} + 'SPLITME end')\n", cols.saturating_sub(3)));
            let _ = this.update_in(cx, |this, window, cx| {
                this.run_config(brisk_run::RunConfig { name: "split".into(), target: "split.py".into(), ..Default::default() }, window, cx);
            });
            for _ in 0..100 {
                pause(100).await;
                if this.update(cx, |t, cx| t.runs.panel.as_ref().is_some_and(|p| p.read(cx).run_tabs().iter().all(|(_, r)| !r))).unwrap_or(false) {
                    break;
                }
            }
            pause(300).await;
            let _ = this.update_in(cx, |this, window, cx| {
                if let Some(p) = this.runs.panel.clone() {
                    let f = p.read(cx).focus_handle(cx);
                    window.focus(&f);
                }
                this.smoke_press("ctrl-f ctrl-backspace", window, cx);
                if let Some(p) = this.runs.panel.clone() {
                    p.update(cx, |p, _| p.search_type("splitme"));
                }
                show(this, cx, &format!("splitme across the wrap at {cols} columns"));
                this.smoke_press("escape", window, cx);
                let focused = this.run_panel_focused(window, cx);
                show(this, cx, &format!("escape (run panel still focused {focused})"));
                // Soft-Wrap and Scroll to End per configuration: off for "split", "out" keeps both on.
                if let Some(p) = this.runs.panel.clone() {
                    p.update(cx, |p, _| {
                        p.toggle_soft_wrap();
                        // Clicked off (it may already show off: the search scrolled up).
                        p.toggle_scroll_to_end();
                        if p.scroll_to_end {
                            p.toggle_scroll_to_end();
                        }
                    });
                }
            });
            let toggles = |this: &Workspace, cx: &gpui::App| this.runs.panel.as_ref().map(|p| (p.read(cx).soft_wrap, p.read(cx).scroll_to_end));
            let tabs_key = |this: &mut Workspace, cx: &mut Context<Workspace>| {
                if let Some(p) = this.runs.panel.clone() {
                    p.update(cx, |p, cx| p.key(&gpui::Keystroke::parse("alt-left").unwrap(), cx));
                }
            };
            let mut seen = Vec::new();
            for _ in 0..3 {
                let _ = this.update(cx, |this, cx| {
                    tabs_key(this, cx);
                    cx.notify();
                });
                pause(200).await;
                let _ = this.update(cx, |this, cx| {
                    let name = this.runs.panel.as_ref().and_then(|p| p.read(cx).run_tabs_detail().into_iter().nth(0).map(|t| t.0));
                    seen.push((name, toggles(this, cx)));
                });
            }
            let _ = this.update_in(cx, |this, window, cx| {
                this.run_config(brisk_run::RunConfig { name: "split".into(), target: "split.py".into(), ..Default::default() }, window, cx);
            });
            pause(800).await;
            let _ = this.update(cx, |this, cx| {
                let file = std::fs::read_to_string(this.root.join(".brisk/run-console.json")).unwrap_or_default().split_whitespace().collect::<String>();
                eprintln!("smoke-cfind: per-configuration toggles while switching tabs {:?}; split run again {:?}; file {file}", seen.iter().map(|s| s.1).collect::<Vec<_>>(), toggles(this, cx));
                cx.quit();
            });
        })
        .detach();
    }
}
