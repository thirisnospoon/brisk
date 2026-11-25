//! BRISK_SMOKE_FLOOD=1 (or `=pty` for "Emulate terminal") on an empty folder: a script prints
//! as fast as it can for 4 s while the smoke types into an editor. Reports how many lines the
//! script wrote, how far behind the console was when it ended, and the UI thread's stalls:
//! the gaps of a 5 ms ticker on the UI thread (max, p99, count over 50 ms) and the time each
//! typed character took. `=find`: the same with the console's Ctrl+F bar open on a query that
//! matches every line, then the time one search over the full 20 000-line buffer takes.

use crate::workspace::Workspace;
use brisk_run::RunConfig;
use gpui::{Context, Window};
use std::time::{Duration, Instant};

const FLOOD: &str = "import sys, time\nt0 = time.time()\nn = 0\nwhile time.time() - t0 < 4:\n    for _ in range(1000):\n        print(n, 'flood line with some text to make it longer', n * 7)\n        n += 1\nsys.stdout.flush()\nprint('FLOOD DONE', n, round(n / (time.time() - t0)), flush=True)\nopen('flood.out', 'w').write(str(n))\n";

fn stats(mut v: Vec<f64>) -> String {
    if v.is_empty() {
        return "none".into();
    }
    v.sort_by(|a, b| a.total_cmp(b));
    let p = |q: f64| v[((v.len() - 1) as f64 * q).round() as usize];
    format!(
        "n {}, median {:.1} ms, p99 {:.1} ms, max {:.1} ms, over 50 ms: {}",
        v.len(),
        p(0.5),
        p(0.99),
        v[v.len() - 1],
        v.iter().filter(|&&x| x > 50.0).count()
    )
}

impl Workspace {
    pub(in crate::workspace) fn flood_smoke(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let root = self.root.clone();
        let pty = std::env::var("BRISK_SMOKE_FLOOD").as_deref() == Ok("pty");
        let find = std::env::var("BRISK_SMOKE_FLOOD").as_deref() == Ok("find");
        let _ = std::fs::write(root.join("flood.py"), FLOOD);
        let _ = std::fs::write(root.join("notes.py"), "# typing here during the flood\n");
        let root2 = root.clone();
        cx.spawn_in(window, async move |this, cx| {
            let root = root2;
            let bg = cx.background_executor().clone();
            let pause = |ms| bg.timer(Duration::from_millis(ms));
            for _ in 0..100 {
                pause(100).await;
                if this.update(cx, |t, _| t.runs.interpreter(&t.root).is_some()).unwrap_or(false) {
                    break;
                }
            }
            let _ = this.update_in(cx, |this, window, cx| {
                this.open_file(&this.root.join("notes.py"), window, cx);
                this.smoke_press("ctrl-end", window, cx);
            });
            pause(500).await;
            use crate::terminal_view::{PANEL_RENDERS, PUMP_NOTIFIES};
            use std::sync::atomic::Ordering::Relaxed;
            let counts0 = (PUMP_NOTIFIES.load(Relaxed), PANEL_RENDERS.load(Relaxed));
            let started = Instant::now();
            let _ = this.update_in(cx, |this, window, cx| {
                let cfg = RunConfig { name: "flood".into(), target: "flood.py".into(), emulate_terminal: pty, ..Default::default() };
                this.run_config(cfg, window, cx);
                if find {
                    if let Some(p) = this.runs.panel.clone() {
                        window.focus(&gpui::Focusable::focus_handle(p.read(cx), cx));
                        this.smoke_press("ctrl-f", window, cx);
                        p.update(cx, |p, _| p.search_type("longer"));
                    }
                }
                // The editor keeps the focus, as with "Show Run window".
                if let Some(ed) = this.active_editor().cloned() {
                    window.focus(&gpui::Focusable::focus_handle(ed.read(cx), cx));
                }
            });
            let mut gaps = Vec::new();
            let mut keys = Vec::new();
            let mut last = Instant::now();
            let mut next_key = Instant::now();
            let mut done_at = None;
            let mut report = String::new();
            while started.elapsed() < Duration::from_secs(30) {
                pause(5).await;
                let now = Instant::now();
                gaps.push((now - last).as_secs_f64() * 1000.0 - 5.0);
                last = now;
                if now >= next_key && done_at.is_none() {
                    next_key = now + Duration::from_millis(40);
                    let t = Instant::now();
                    let _ = this.update_in(cx, |this, window, cx| this.smoke_type("x", window, cx));
                    keys.push(t.elapsed().as_secs_f64() * 1000.0);
                    last = Instant::now();
                }
                if gaps.len() % 20 == 0 {
                    let text = this.update(cx, |t, cx| t.runs.panel.as_ref().and_then(|p| p.read(cx).active_text()).unwrap_or_default()).unwrap_or_default();
                    if done_at.is_none() {
                        if let Some(l) = text.lines().find(|l| l.contains("FLOOD DONE")) {
                            done_at = Some(started.elapsed());
                            report = l.trim().to_string();
                        }
                    }
                    if text.contains("Process finished") {
                        if done_at.is_none() {
                            report = format!(
                                "lines written {:?}, console tail {:?}",
                                std::fs::read_to_string(root.join("flood.out")).ok(),
                                text.lines().rev().take(4).collect::<Vec<_>>()
                            );
                        }
                        break;
                    }
                    last = Instant::now();
                }
            }
            let secs = started.elapsed().as_secs_f64();
            let (notifies, renders) = (PUMP_NOTIFIES.load(Relaxed) - counts0.0, PANEL_RENDERS.load(Relaxed) - counts0.1);
            let _ = this.update(cx, |this, cx| {
                eprintln!(
                    "smoke-flood: output redraws {notifies} ({:.0}/s), Run panel renders {renders} ({:.0}/s)",
                    notifies as f64 / secs,
                    renders as f64 / secs
                );
                let typed = this.active_editor().map(|e| e.read(cx).text_string().matches('x').count()).unwrap_or(0);
                eprintln!("smoke-flood: mode {}, script {report:?}, seen after {:?}, total {:?}", if pty { "pty" } else { "pipe" }, done_at, started.elapsed());
                eprintln!("smoke-flood: ui ticker gaps (minus 5 ms): {}", stats(gaps));
                if let Some(p) = this.runs.panel.clone().filter(|_| find) {
                    let t = Instant::now();
                    let n = p.update(cx, |p, _| {
                        p.search_type("");
                        p.search_summary()
                    });
                    let ms = t.elapsed().as_secs_f64() * 1000.0;
                    eprintln!("smoke-flood: search over {} lines: {n:?} in {ms:.1} ms", p.read(cx).active_text().unwrap_or_default().lines().count());
                }
                eprintln!("smoke-flood: typed chars {typed}, per key: {}", stats(keys));
                cx.quit();
            });
        })
        .detach();
    }
}
