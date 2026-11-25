//! BRISK_SMOKE_LATENCY=1|pytest on an empty folder: how long Shift+F10 takes — the work done
//! on the UI thread before the process starts, the moment the script's first line runs (its
//! own clock), and when that line shows in the Run console. Several runs, the first one
//! (which creates the Run window) reported apart.

use crate::workspace::Workspace;
use brisk_run::{RunConfig, RunFile, RunKind};
use gpui::{Context, Window};
use std::time::{Duration, SystemTime, UNIX_EPOCH};

const RUNS: usize = 8;

fn now() -> f64 {
    SystemTime::now().duration_since(UNIX_EPOCH).map(|d| d.as_secs_f64()).unwrap_or(0.0)
}

impl Workspace {
    pub(in crate::workspace) fn latency_smoke(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let root = self.root.clone();
        let pytest = std::env::var("BRISK_SMOKE_LATENCY").as_deref() == Ok("pytest");
        let _ = std::fs::write(root.join("lat.py"), "import time\nprint('FIRST', repr(time.time()), flush=True)\n");
        let _ = std::fs::write(root.join("test_lat.py"), "import time\n\ndef test_lat():\n    print('FIRST', repr(time.time()))\n");
        let cfg = if pytest {
            RunConfig { name: "lat".into(), kind: RunKind::Pytest, target: "test_lat.py".into(), parameters: "-s".into(), ..Default::default() }
        } else {
            RunConfig { name: "lat".into(), target: "lat.py".into(), ..Default::default() }
        };
        let _ = brisk_run::save(&root, &RunFile { selected: Some("lat".into()), configurations: vec![cfg], ..Default::default() });
        cx.spawn_in(window, async move |this, cx| {
            let bg = cx.background_executor().clone();
            let pause = |ms| bg.timer(Duration::from_millis(ms));
            for _ in 0..100 {
                pause(100).await;
                if this.update(cx, |t, _| t.runs.interpreter(&t.root).is_some()).unwrap_or(false) {
                    break;
                }
            }
            pause(500).await;
            let mut rows: Vec<(f64, f64, f64)> = Vec::new();
            for _ in 0..RUNS {
                let t0 = now();
                let _ = this.update_in(cx, |this, window, cx| this.dispatch(brisk_keymap::Action::Run, window, cx));
                let sync = now() - t0;
                let mut seen = None;
                let mut py = None;
                for _ in 0..3000 {
                    pause(2).await;
                    let text = this.update(cx, |t, cx| t.runs.panel.as_ref().and_then(|p| p.read(cx).active_text()).unwrap_or_default()).unwrap_or_default();
                    if seen.is_none() {
                        if let Some(l) = text.lines().find_map(|l| l.find("FIRST ").map(|i| &l[i..])) {
                            seen = Some(now() - t0);
                            py = l[6..].trim().trim_end_matches('.').parse::<f64>().ok().map(|t| t - t0);
                        }
                    }
                    if text.contains("Process finished") {
                        break;
                    }
                }
                rows.push((sync * 1000.0, py.unwrap_or(f64::NAN) * 1000.0, seen.unwrap_or(f64::NAN) * 1000.0));
                pause(300).await;
            }
            let fmt = |r: &(f64, f64, f64)| format!("ui {:.0} ms, script starts {:.0} ms, first line shown {:.0} ms", r.0, r.1, r.2);
            let med = |k: usize| {
                let mut v: Vec<f64> = rows[1..].iter().map(|r| [r.0, r.1, r.2][k]).collect();
                v.sort_by(|a, b| a.total_cmp(b));
                v[v.len() / 2]
            };
            let _ = this.update(cx, |_, cx| {
                eprintln!("smoke-latency: {} first run: {}", if pytest { "pytest" } else { "script" }, fmt(&rows[0]));
                eprintln!("smoke-latency: {} median of {}: {}", if pytest { "pytest" } else { "script" }, rows.len() - 1, fmt(&(med(0), med(1), med(2))));
                eprintln!("smoke-latency: all {:?}", rows.iter().map(|r| (r.0.round(), r.1.round(), r.2.round())).collect::<Vec<_>>());
                cx.quit();
            });
        })
        .detach();
    }
}
