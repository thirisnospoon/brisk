//! `BRISK_SMOKE_OPEN=1` (on an empty scratch folder): how long opening a file takes, from the
//! moment Enter would open it (Project / Recent Files call `open_file`) to the end of the
//! first painted frame that shows it. Python files of 5k and 30k lines, three fresh copies
//! of each (nothing cached for that path), plus the slowest frame in the 2 s after the open
//! (background highlighting or ruff/ty results arriving must not make a slow frame). Exits
//! with code 1 over 50 ms (5k) / 150 ms (30k) on the first frame, or 50 ms on a later one.

use crate::stall;
use crate::workspace::Workspace;
use gpui::{AsyncWindowContext, Context, Window};
use std::path::{Path, PathBuf};
use std::time::{Duration, Instant};

const SIZES: [(usize, f32); 2] = [(5_000, 50.0), (30_000, 150.0)];
const COPIES: usize = 3;
const LATER_MAX_MS: f32 = 50.0;

/// Python that looks like a real module: imports, classes with methods, docstrings, strings,
/// comments, decorators, f-strings — `lines` lines in all.
fn module(lines: usize) -> String {
    let mut s = String::from("\"\"\"Generated module for the open smoke.\"\"\"\n\nimport os\nimport sys\nimport json\nfrom dataclasses import dataclass, field\nfrom typing import Any, Optional\n\n\n");
    let mut i = 0;
    while 9 + i * 22 < lines {
        s.push_str(&format!(
            "@dataclass\nclass Model{i}:\n    \"\"\"A record with a few fields ({i}).\"\"\"\n\n    name: str = \"model-{i}\"\n    size: int = {i}\n    tags: list[str] = field(default_factory=list)\n\n    def describe(self, verbose: bool = False) -> str:\n        # Format the record for the log.\n        text = f\"{{self.name}}: {{self.size}} ({{len(self.tags)}} tags)\"\n        if verbose and self.size > {i} % 7:\n            return text + json.dumps({{'tags': self.tags, 'path': os.sep}})\n        return text\n\n    def scaled(self, factor: float) -> Optional[int]:\n        try:\n            return int(self.size * factor)\n        except (TypeError, ValueError) as e:\n            print(e, file=sys.stderr)\n            return None\n\n\n"
        ));
        i += 1;
    }
    s.lines().take(lines).collect::<Vec<_>>().join("\n") + "\n"
}

fn wait(cx: &AsyncWindowContext, ms: u64) -> gpui::Task<()> {
    cx.background_executor().timer(Duration::from_millis(ms))
}

fn files(root: &Path) -> Vec<(usize, f32, PathBuf)> {
    let dir = root.join("open");
    let _ = std::fs::create_dir_all(&dir);
    let mut out = Vec::new();
    for (lines, limit) in SIZES {
        let text = module(lines);
        for k in 0..COPIES {
            let p = dir.join(format!("mod_{}k_{k}.py", lines / 1000));
            let _ = std::fs::write(&p, &text);
            out.push((lines, limit, p));
        }
    }
    out
}

impl Workspace {
    pub(in crate::workspace) fn open_smoke(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let root = self.root.clone();
        cx.spawn_in(window, async move |this, cx| {
            let r = root.clone();
            let list = cx.background_executor().spawn(async move { files(&r) }).await;
            wait(cx, 2000).await; // startup settles (index, first frames)
            let mut ok = true;
            eprintln!("smoke-open: {:<16} {:>7} {:>10} {:>12} {:>11}", "file", "lines", "open_file", "first frame", "later max");
            for (lines, limit, path) in list {
                stall::take_frames();
                let Ok(sync) = this.update_in(cx, |this, window, cx| {
                    let t = Instant::now();
                    stall::pin_frame_start(t);
                    this.open_file(&path, window, cx);
                    t.elapsed().as_secs_f32() * 1000.0
                }) else {
                    return;
                };
                let mut first = None;
                for _ in 0..1000 {
                    wait(cx, 5).await;
                    if let Some(f) = stall::take_frames().first().copied() {
                        first = Some(f);
                        break;
                    }
                }
                // What follows the first frame: background highlighting, lint results, a toast.
                wait(cx, 2000).await;
                let later = stall::take_frames().into_iter().fold(0.0f32, f32::max);
                let first = first.unwrap_or(f32::INFINITY);
                let pass = first <= limit && later <= LATER_MAX_MS;
                ok &= pass;
                let name = path.file_name().map(|n| n.to_string_lossy().into_owned()).unwrap_or_default();
                eprintln!("smoke-open: {name:<16} {lines:>7} {sync:>8.1}ms {first:>10.1}ms {later:>9.1}ms {}", if pass { "ok" } else { "OVER" });
                let _ = this.update_in(cx, |this, window, cx| {
                    if let Some(i) = this.active {
                        this.close_tab(i, window, cx);
                    }
                });
                wait(cx, 300).await;
            }
            eprintln!("smoke-open: limits first frame 50 ms (5k) / 150 ms (30k), later frames {LATER_MAX_MS} ms -> {}", if ok { "OK" } else { "FAIL" });
            std::process::exit(if ok { 0 } else { 1 });
        })
        .detach();
    }
}
