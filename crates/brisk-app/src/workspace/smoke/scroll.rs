//! `BRISK_SMOKE_SCROLL=1` (on an empty scratch folder; it writes a 20k-file folder, a 2000-commit
//! git history, 1000 pytest tests and a 5000-definition file there): fills the long lists with
//! large data, then scrolls each one for ~2 s with real WM_MOUSEWHEEL messages and prints the
//! frame times (p50 / p95 / max, from `stall::frame_*`) and the worst main-thread stall. Exits
//! with code 1 when a list goes over p95 16 ms or max 50 ms in 3 runs (debug build). `=hold` leaves the
//! window open; `=<name>` (e.g. `=todo`) runs only the lists whose name contains it.

use crate::stall;
use crate::workspace::{BottomTool, LeftTool, Workspace};
use gpui::{AsyncWindowContext, Context, UniformListScrollHandle, WeakEntity, Window};
use std::path::Path;
use std::time::Duration;

const P95_MS: f32 = 16.0;
const MAX_MS: f32 = 50.0;
const TREE_FILES: usize = 20_000;
const COMMITS: usize = 2000;
const TESTS: usize = 1000;

/// Where to point the wheel, in logical pixels from the window's size.
#[derive(Clone)]
enum At {
    /// The left tool window (Project, Structure).
    Left,
    /// The bottom tool window, `x` as a fraction of the window width.
    Bottom(f32),
    /// The middle of a list's laid-out bounds (from its scroll handle).
    List(UniformListScrollHandle),
}

struct Stats {
    frames: usize,
    p50: f32,
    p95: f32,
    max: f32,
    stall: u64,
    /// How many runs [`best`] took (1: the first was within the limits).
    runs: usize,
}

fn stats(mut f: Vec<f32>) -> Stats {
    f.sort_by(|a, b| a.total_cmp(b));
    let at = |q: f32| f.get(((f.len() as f32 * q) as usize).min(f.len().saturating_sub(1))).copied().unwrap_or(0.0);
    Stats { frames: f.len(), p50: at(0.5), p95: at(0.95), max: f.last().copied().unwrap_or(0.0), stall: stall::worst().0, runs: 1 }
}

fn git(root: &Path, args: &[&str], stdin: Option<&[u8]>) -> bool {
    use std::io::Write;
    let mut c = std::process::Command::new("git");
    c.arg("-C").arg(root).args(args).env("GIT_TERMINAL_PROMPT", "0");
    c.stdin(if stdin.is_some() { std::process::Stdio::piped() } else { std::process::Stdio::null() });
    c.stdout(std::process::Stdio::null()).stderr(std::process::Stdio::null());
    brisk_platform::process::no_window(&mut c);
    let Ok(mut ch) = c.spawn() else { return false };
    if let (Some(data), Some(mut si)) = (stdin, ch.stdin.take()) {
        let _ = si.write_all(data);
    }
    ch.wait().is_ok_and(|s| s.success())
}

/// The scratch data: a big folder, a long history, a test file, a long Python file.
fn generate(root: &Path) {
    let big = root.join("big");
    if !big.join(format!("f{:05}.py", TREE_FILES - 1)).exists() {
        let _ = std::fs::create_dir_all(&big);
        for i in 0..TREE_FILES {
            let _ = std::fs::write(big.join(format!("f{i:05}.py")), format!("# TODO: item {i}\nneedle = {i}\n"));
        }
    }
    let mut outline = String::new();
    for i in 0..1250 {
        outline
            .push_str(&format!("class C{i}:\n    def a(self):\n        return {i}\n\n    def b(self):\n        pass\n\n    def c(self):\n        pass\n\n\n"));
    }
    let _ = std::fs::write(root.join("outline.py"), outline);
    let _ = std::fs::write(
        root.join("test_scroll.py"),
        format!("import pytest\n\n\n@pytest.mark.parametrize(\"i\", range({TESTS}))\ndef test_n(i):\n    assert i >= 0\n"),
    );
    if !git(root, &["rev-parse", "-q", "--verify", "HEAD"], None) {
        git(root, &["init", "-q"], None);
        // 2000 commits in one `git fast-import`: one file changed per commit. Each commit
        // continues the branch the stream builds (a `from` line would name a ref that does
        // not exist yet, and fast-import rejects the whole stream).
        let mut s = String::new();
        for i in 0..COMMITS {
            let body = format!("v = {i}\n");
            let msg = format!("Change {i}: tweak the value in the log scroll smoke\n");
            s.push_str(&format!("commit refs/heads/main\ncommitter Smoke <smoke@example.invalid> {} +0000\ndata {}\n{msg}", 1_700_000_000 + i * 60, msg.len()));
            s.push_str(&format!("M 644 inline log/v{}.py\ndata {}\n{body}\n", i % 50, body.len()));
        }
        if !git(root, &["fast-import", "--quiet"], Some(s.as_bytes())) {
            eprintln!("smoke-scroll: git fast-import failed: no history for git-log");
        }
        git(root, &["symbolic-ref", "HEAD", "refs/heads/main"], None);
        git(root, &["reset", "-q", "--hard"], None);
    }
}

fn wait(cx: &AsyncWindowContext, ms: u64) -> gpui::Task<()> {
    cx.background_executor().timer(Duration::from_millis(ms))
}

/// Scroll the list under `at` for ~2 s (down, then back up) and return its frame stats.
async fn scroll(this: &WeakEntity<Workspace>, cx: &mut AsyncWindowContext, at: At) -> Stats {
    wait(cx, 600).await; // let the open settle (first layout, font cache)
    let Ok((x, y)) = this.update_in(cx, |this, window, _| {
        let v = window.viewport_size();
        let (w, h) = (f32::from(v.width), f32::from(v.height));
        let bottom_h = this.bottom.map_or(240., |b| this.bottom_height(b));
        let (x, y) = match &at {
            At::Left => (190., h / 2.),
            At::Bottom(fx) => (w * fx, h - 30. - bottom_h / 2.),
            At::List(l) => {
                let b = l.0.borrow().base_handle.bounds();
                (f32::from(b.center().x), f32::from(b.center().y))
            }
        };
        let s = window.scale_factor();
        (x * s, y * s)
    }) else {
        return stats(Vec::new());
    };
    crate::workspace::smoke::win_input::post(crate::workspace::smoke::win_input::Mouse::Move, x, y, false);
    wait(cx, 100).await;
    stall::take_frames();
    stall::reset();
    for i in 0..120 {
        crate::workspace::smoke::win_input::wheel(x, y, if i < 60 { -120 } else { 120 });
        wait(cx, 16).await;
    }
    wait(cx, 100).await;
    stats(stall::take_frames())
}

/// [`scroll`], measured again (up to 3 runs in all) while a run is over the limits: on a loaded
/// machine a single preempted frame can push the max up. Every run is printed; a real slowdown
/// fails all three.
async fn best(this: &WeakEntity<Workspace>, cx: &mut AsyncWindowContext, at: At) -> Stats {
    let mut s = scroll(this, cx, at.clone()).await;
    let mut runs = 1;
    for run in 2..=3 {
        if s.frames > 0 && s.p95 <= P95_MS && s.max <= MAX_MS {
            break;
        }
        eprintln!("smoke-scroll:   over ({} frames, p95 {:.1} ms, max {:.1} ms): run {run}", s.frames, s.p95, s.max);
        let again = scroll(this, cx, at.clone()).await;
        runs = run;
        if again.frames > 0 && ((again.p95 <= P95_MS && again.max <= MAX_MS) || again.max <= s.max || s.frames == 0) {
            s = again;
        }
    }
    Stats { runs, ..s }
}

impl Workspace {
    pub(in crate::workspace) fn list_scroll_smoke(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let arg = std::env::var("BRISK_SMOKE_SCROLL").unwrap_or_default();
        let hold = arg == "hold";
        let only = (!hold && arg != "1" && !arg.is_empty()).then(|| arg.clone());
        let root = self.root.clone();
        cx.spawn_in(window, async move |this, cx| {
            let r = root.clone();
            cx.background_executor().spawn(async move { generate(&r) }).await;
            eprintln!("smoke-scroll: data ready in {}", root.display());
            let mut results: Vec<(&'static str, Stats)> = Vec::new();
            let want = |name: &str| only.as_ref().is_none_or(|o| name.contains(o.as_str()));

            if want("project-tree") {
                let big = root.join("big");
                let _ = this.update_in(cx, |this, window, cx| {
                    this.reindex_files(cx);
                    this.left = Some(LeftTool::Project);
                    if let Some(id) = this.tree.reveal(&big.join("f00000.py")) {
                        this.tree.expand(id);
                    }
                    let _ = window;
                    cx.notify();
                });
                results.push(("project-tree (20k files)", best(&this, cx, At::Left).await));
            }

            if want("structure") {
                let f = root.join("outline.py");
                let _ = this.update_in(cx, |this, window, cx| {
                    this.open_file(&f, window, cx);
                    this.left = Some(LeftTool::Structure);
                    cx.notify();
                });
                results.push(("structure (5000 defs)", best(&this, cx, At::Left).await));
                let _ = this.update(cx, |this, cx| {
                    this.left = None;
                    cx.notify();
                });
            }

            if want("problems") {
                let _ = this.update(cx, |this, cx| {
                    let diags = (0..10_000)
                        .map(|i| brisk_lint::Diagnostic {
                            file: root.join("big").join(format!("f{:05}.py", i / 5)),
                            code: "F401".into(),
                            message: format!("`os` imported but unused ({i})"),
                            severity: if i % 3 == 0 { brisk_lint::Severity::Error } else { brisk_lint::Severity::Warning },
                            start: brisk_lint::Pos { line: i % 40, col: 0 },
                            end: brisk_lint::Pos { line: i % 40, col: 9 },
                            fix: None,
                            noqa_line: Some(i % 40),
                        })
                        .collect();
                    this.problems.set_project(Some(Ok(diags)));
                    this.problems.tab = crate::workspace::problems_ui::ProblemsTab::Project;
                    this.bottom = Some(BottomTool::Problems);
                    cx.notify();
                });
                results.push(("problems (10k)", best(&this, cx, At::Bottom(0.5)).await));
            }

            if want("todo") {
                let _ = this.update_in(cx, |this, window, cx| {
                    this.toggle_todo_window(window, cx);
                });
                for _ in 0..300 {
                    wait(cx, 100).await;
                    if this.update(cx, |this, cx| this.todo.scanned(cx)).unwrap_or(true) {
                        break;
                    }
                }
                results.push(("todo (20k items)", best(&this, cx, At::Bottom(0.5)).await));
            }

            if want("packages") {
                let _ = this.update_in(cx, |this, window, cx| {
                    this.bottom = None;
                    this.toggle_python_packages(window, cx);
                });
                wait(cx, 300).await;
                let _ = this.update(cx, |this, cx| {
                    if let Some(p) = this.packages.as_mut() {
                        p.list = (0..500).map(|i| brisk_run::env::packages::Package { name: format!("package-{i:03}"), version: format!("1.{i}.0") }).collect();
                        p.loading = false;
                    }
                    cx.notify();
                });
                results.push(("packages (500)", best(&this, cx, At::Bottom(0.3)).await));
            }

            if want("python-console") {
                let _ = this.update_in(cx, |this, window, cx| {
                    this.bottom = None;
                    this.toggle_python_console(window, cx);
                });
                wait(cx, 300).await;
                let _ = this.update(cx, |this, cx| {
                    if let Some(p) = this.pyc.as_mut() {
                        p.lines = (0..5000)
                            .map(|i| {
                                if i % 4 == 0 {
                                    (format!(">>> print(value_{i})\n"), crate::workspace::console_ui::Kind::Code(Vec::new()))
                                } else {
                                    (format!("{i}: the quick brown fox jumps over the lazy dog 0123456789\n"), crate::workspace::console_ui::Kind::Out)
                                }
                            })
                            .collect();
                        p.cap_and_follow();
                    }
                    cx.notify();
                });
                results.push(("python-console (5000 lines)", best(&this, cx, At::Bottom(0.3)).await));
            }

            if want("debug-console") || want("debug-variables") {
                let tb = root.join("big").join("f00001.py");
                let _ = this.update(cx, |this, cx| {
                    let mut d = this.debug.as_mut(cx);
                    d.console = (0..5000)
                        .map(|i| {
                            if i % 3 == 0 {
                                (format!("  File \"{}\", line {}, in f\n", tb.display(), i % 40 + 1), crate::workspace::debug::Line::Err)
                            } else {
                                (format!("{i}: output line of the debugged program\n"), crate::workspace::debug::Line::Out)
                            }
                        })
                        .collect();
                    d.console_changed(true);
                    d.vars = (0..10_000)
                        .map(|i| crate::workspace::debug::VarRow {
                            depth: if i == 0 { 0 } else { 1 },
                            var: brisk_dap::Variable {
                                name: format!("[{i}]"),
                                value: format!("{{'key': {i}, 'name': 'row {i}'}}"),
                                type_name: "dict".into(),
                                reference: 0,
                                evaluate_name: String::new(),
                            },
                            expanded: i == 0,
                            kind: if i == 0 { crate::workspace::debug::RowKind::Scope } else { crate::workspace::debug::RowKind::Var },
                            parent: 0,
                        })
                        .collect();
                    drop(d);
                    this.bottom = Some(BottomTool::Debug);
                    cx.notify();
                });
                if want("debug-variables") {
                    let _ = this.update(cx, |this, cx| {
                        this.debug.as_mut(cx).tab = crate::workspace::debug::DebugTab::Debugger;
                        cx.notify();
                    });
                    results.push(("debug-variables (10k)", best(&this, cx, At::Bottom(0.7)).await));
                }
                if want("debug-console") {
                    let _ = this.update(cx, |this, cx| {
                        this.debug.as_mut(cx).tab = crate::workspace::debug::DebugTab::Console;
                        cx.notify();
                    });
                    results.push(("debug-console (5000 lines)", best(&this, cx, At::Bottom(0.5)).await));
                }
            }

            if want("git-log") {
                let _ = this.update_in(cx, |this, window, cx| {
                    // On a fresh folder the repository was created after the project opened.
                    if this.git(cx).is_none() {
                        this.set_git(brisk_git::Repo::discover(&this.root).map(|repo| crate::workspace::git_ui::Git::new(repo, cx)), cx);
                    }
                    this.bottom = None;
                    this.dispatch(brisk_keymap::Action::ToggleGitWindow, window, cx);
                    this.refresh_git(cx);
                });
                for _ in 0..200 {
                    wait(cx, 100).await;
                    if this.update(cx, |this, cx| this.git(cx).is_some_and(|g| g.log.len() >= 1000)).unwrap_or(true) {
                        break;
                    }
                }
                let n = this.update(cx, |this, cx| this.git(cx).map_or(0, |g| g.log.len())).unwrap_or(0);
                if n < 1000 {
                    // An empty log scrolls nothing: report it instead of measuring it.
                    eprintln!("smoke-scroll: git-log shows {n} commits, expected {COMMITS}");
                    results.push(("git-log (2000 commits)", stats(Vec::new())));
                } else {
                    let at = this.update(cx, |this, cx| this.git(cx).map(|g| At::List(g.log_handle()))).ok().flatten().unwrap_or(At::Bottom(0.45));
                    results.push(("git-log (2000 commits)", best(&this, cx, at).await));
                }
            }

            if want("find") {
                let _ = this.update_in(cx, |this, window, cx| {
                    this.bottom = None;
                    this.find.set_query_for_smoke("needle");
                    this.open_find(false, window, cx);
                });
                for _ in 0..240 {
                    wait(cx, 250).await;
                    if this.update(cx, |this, _| this.files.is_some() && this.find.idle()).unwrap_or(true) {
                        break;
                    }
                }
                let at = this.update(cx, |this, _| At::List(this.find.list_handle())).unwrap_or(At::Bottom(0.5));
                results.push(("find-in-files (needle, 20k files)", best(&this, cx, at).await));
                let _ = this.update(cx, |this, cx| {
                    this.find.open = false;
                    cx.notify();
                });
            }

            if want("test-tree") {
                let cfg = brisk_run::RunConfig {
                    name: "scroll tests".into(),
                    kind: brisk_run::RunKind::Pytest,
                    target: "test_scroll.py".into(),
                    ..Default::default()
                };
                for _ in 0..100 {
                    if this.update(cx, |t, cx| t.interpreter(cx).is_some()).unwrap_or(true) {
                        break;
                    }
                    wait(cx, 100).await;
                }
                let _ = this.update_in(cx, |this, window, cx| this.run_config(cfg, window, cx));
                for _ in 0..600 {
                    wait(cx, 100).await;
                    let done = this
                        .update(cx, |t, cx| t.runs.panel.as_ref().and_then(|p| p.read(cx).active_text()).unwrap_or_default().contains("Process finished"))
                        .unwrap_or(true);
                    if done {
                        break;
                    }
                }
                results.push(("test-tree (1000 tests)", best(&this, cx, At::Bottom(0.15)).await));
            }

            let mut ok = true;
            eprintln!("smoke-scroll: {:<36} {:>6} {:>7} {:>7} {:>7} {:>7}", "list", "frames", "p50", "p95", "max", "stall");
            for (name, s) in &results {
                let pass = s.frames > 0 && s.p95 <= P95_MS && s.max <= MAX_MS;
                ok &= pass;
                eprintln!(
                    "smoke-scroll: {name:<36} {:>6} {:>6.1}ms {:>6.1}ms {:>6.1}ms {:>5}ms {}{}",
                    s.frames,
                    s.p50,
                    s.p95,
                    s.max,
                    s.stall,
                    if pass { "ok" } else { "OVER" },
                    if s.runs > 1 { format!(" ({} runs)", s.runs) } else { String::new() }
                );
            }
            eprintln!("smoke-scroll: limits p95 {P95_MS} ms, max {MAX_MS} ms -> {}", if ok { "OK" } else { "FAIL" });
            if !hold {
                std::process::exit(if ok { 0 } else { 1 });
            }
        })
        .detach();
    }
}
