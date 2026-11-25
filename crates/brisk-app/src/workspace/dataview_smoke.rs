//! `BRISK_SMOKE_DATAVIEW=1`: self-contained. Makes a venv with pandas and numpy in
//! `%TEMP%\brisk-smoke-dataview` (or `BRISK_SMOKE_DATAVIEW_DIR`; reused while it imports them),
//! writes `frames.py` there and debugs it with that interpreter. No interpreter, no network
//! or no wheels: prints `SKIP` and exits with 0. `BRISK_SMOKE_DATAVIEW=<file.py>` debugs that
//! file with the project's interpreter instead (it needs pandas and numpy).
//!
//! The run stops at the line marked `# BREAK`, opens View as DataFrame for `df`, sorts and
//! filters it, formats floats, opens a 1M-row frame and pages to its middle, opens View as
//! Array for `arr`, checks Copy Value (whole long string), Copy as Expression, Add to Watches
//! and View Value, then steps and checks that the open tables evaluate again. Exits with 1 if
//! any check fails. `BRISK_SMOKE_DATAVIEW_HOLD=1` keeps the window open on the `df` table.

use super::debug_ui::State;
use super::{Tab, Workspace};
use brisk_keymap::Action;
use gpui::{Context, Window};
use std::path::{Path, PathBuf};
use std::time::{Duration, Instant};

const FRAMES: &str = "import numpy as np
import pandas as pd

df = pd.DataFrame(
    {
        \"city\": [\"Lviv\", \"Kyiv\", \"Odesa\", \"Kyiv\", \"Dnipro\"],
        \"price\": [1.5, 3.14159, 2.0, 7.25, 0.5],
        \"qty\": [3, 1, 4, 1, 5],
    }
)
big = pd.DataFrame({\"v\": np.random.default_rng(1).random(1_000_000), \"k\": np.arange(1_000_000)})
arr = np.arange(20).reshape(5, 4)
long_s = \"x\" * 5000 + \"END\"
print(len(df))  # BREAK
df.loc[len(df)] = [\"Kharkiv\", 9.0, 2]
print(len(df))
";

fn quiet(cmd: &mut std::process::Command) -> &mut std::process::Command {
    #[cfg(windows)]
    {
        use std::os::windows::process::CommandExt;
        cmd.creation_flags(0x0800_0000);
    }
    crate::py_packages::offline_in_smoke(cmd);
    cmd.stdin(std::process::Stdio::null())
}

/// The packages the venv needs: debugpy too when Brisk has no bundled copy (a build without
/// `vendor/debugpy-lib`).
fn packages() -> Vec<&'static str> {
    let mut v = vec!["pandas", "numpy"];
    if brisk_dap::bundled_debugpy().is_none() {
        v.push("debugpy");
    }
    v
}

fn imports_pandas(python: &Path) -> bool {
    let code = format!("import {}", packages().join(", "));
    python.is_file() && quiet(std::process::Command::new(python).args(["-c", &code])).output().is_ok_and(|o| o.status.success())
}

/// The venv's interpreter with pandas and numpy, made with `base` if needed.
fn prepare_venv(dir: &Path, base: &Path) -> Result<PathBuf, String> {
    let venv = dir.join(".venv");
    let python = if cfg!(windows) { venv.join("Scripts").join("python.exe") } else { venv.join("bin").join("python") };
    if imports_pandas(&python) {
        return Ok(python);
    }
    std::fs::create_dir_all(dir).map_err(|e| format!("cannot create {}: {e}", dir.display()))?;
    if !python.is_file() {
        let o = quiet(std::process::Command::new(base).arg("-m").arg("venv").arg(&venv)).output().map_err(|e| format!("venv: {e}"))?;
        if !o.status.success() {
            return Err(format!("venv failed: {}", String::from_utf8_lossy(&o.stderr).trim()));
        }
    }
    let o = quiet(std::process::Command::new(&python).args(["-m", "pip", "install", "--disable-pip-version-check", "-q"]).args(packages()))
        .output()
        .map_err(|e| format!("pip: {e}"))?;
    if !o.status.success() || !imports_pandas(&python) {
        let err = String::from_utf8_lossy(&o.stderr);
        return Err(format!("pip install {} failed: {}", packages().join(" "), err.lines().last().unwrap_or("").trim()));
    }
    Ok(python)
}

/// The end of the smoke: the verdict is the exit code (quitting first would end with 0).
fn finish(ok: bool) -> ! {
    eprintln!("smoke-dataview: {}", if ok { "OK" } else { "FAIL" });
    std::process::exit(if ok { 0 } else { 1 });
}

impl Workspace {
    pub(super) fn dataview_smoke(&mut self, arg: PathBuf, window: &mut Window, cx: &mut Context<Self>) {
        let own = arg.extension().is_none_or(|e| e != "py");
        cx.spawn_in(window, async move |this, cx| {
            let bg = cx.background_executor().clone();
            let wait = |ms| bg.timer(Duration::from_millis(ms));
            for _ in 0..150 {
                if this.update(cx, |t, _| t.runs.all.is_some()).unwrap_or(true) {
                    break;
                }
                wait(100).await;
            }
            let file = if own {
                let dir = std::env::var_os("BRISK_SMOKE_DATAVIEW_DIR").map(PathBuf::from).unwrap_or_else(|| std::env::temp_dir().join("brisk-smoke-dataview"));
                // A machine interpreter to make the venv with (not a venv itself).
                let base = this
                    .update(cx, |t, _| {
                        let all = t.runs.all.clone().unwrap_or_default();
                        all.iter().find(|i| i.kind == brisk_run::InterpKind::System).or(all.first()).map(|i| i.path.clone())
                    })
                    .ok()
                    .flatten();
                let Some(base) = base else {
                    eprintln!("smoke-dataview: SKIP no Python interpreter on this machine");
                    std::process::exit(0);
                };
                let t0 = Instant::now();
                let d = dir.clone();
                let python = match bg.spawn(async move { prepare_venv(&d, &base) }).await {
                    Ok(p) => p,
                    Err(e) => {
                        eprintln!("smoke-dataview: SKIP {e}");
                        std::process::exit(0);
                    }
                };
                eprintln!("smoke-dataview: venv {} ready in {:.1?}", python.display(), t0.elapsed());
                let file = dir.join("frames.py");
                let _ = std::fs::write(&file, FRAMES);
                // In memory only: the project's run.json does not change.
                let explicit = this
                    .update(cx, |t, _| {
                        let i = t.runs.lookup(&python);
                        t.runs.venvs.insert(0, i);
                        t.runs.file.interpreter.is_some()
                    })
                    .unwrap_or(false);
                if explicit {
                    eprintln!("smoke-dataview: SKIP the project has an interpreter chosen in its run.json; run on a project without one");
                    std::process::exit(0);
                }
                file
            } else {
                arg
            };
            let _ = this.update_in(cx, |t, w, cx| {
                t.open_file(&file, w, cx);
                let Some(ed) = t.active_editor().cloned() else { return };
                let line = ed.read(cx).buffer.text().lines().position(|l| l.contains("# BREAK")).unwrap_or(0);
                ed.update(cx, |e, cx| {
                    e.set_breakpoints(&[]);
                    e.go_to_line(line, cx);
                });
                t.dispatch(Action::ToggleBreakpoint, w, cx);
                t.dispatch(Action::DebugContext, w, cx);
            });
            let mut ok = true;
            let mut check = |what: &str, pass: bool| {
                if !pass {
                    eprintln!("smoke-dataview: FAIL {what}");
                    ok = false;
                }
            };
            let mut paused = false;
            for _ in 0..300 {
                wait(100).await;
                if this.update(cx, |t, _| matches!(t.debug.state(), Some(State::Paused(_)))).unwrap_or(false) {
                    paused = true;
                    break;
                }
            }
            // Variables arrive after the stop.
            for _ in 0..50 {
                if this.update(cx, |t, _| t.debug.vars.iter().any(|r| r.var.name == "df")).unwrap_or(false) {
                    break;
                }
                wait(100).await;
            }
            let rows = this
                .update(cx, |t, _| t.debug.vars.iter().filter(|r| r.depth == 1).map(|r| format!("{} {{{}}} [{}]", r.var.name, r.var.type_name, r.var.evaluate_name)).collect::<Vec<_>>())
                .unwrap_or_default();
            eprintln!("smoke-dataview: paused {paused}, variables {rows:?}");
            check("did not stop at # BREAK", paused);
            if !paused {
                let tail = this.update(cx, |t, _| t.debug.console.iter().rev().take(6).map(|(l, _)| l.trim_end().to_string()).collect::<Vec<_>>()).unwrap_or_default();
                eprintln!("smoke-dataview: debug console (last lines first) {tail:?}");
                finish(false);
            }
            let row_of = |t: &Workspace, name: &str| t.debug.vars.iter().position(|r| r.depth == 1 && r.var.name == name);
            // Wait until the active table has its first page (and nothing in flight).
            macro_rules! table {
                () => {{
                    let mut out = None;
                    for _ in 0..150 {
                        wait(100).await;
                        let s = this.update(cx, |t, cx| match t.active.and_then(|i| t.tabs.get(i)) {
                            Some(Tab::Data(v)) => {
                                let v = v.read(cx);
                                (!v.is_loading() && (v.info.is_some() || v.error.is_some())).then(|| (v.title.clone(), v.info.clone(), v.error.clone(), v.row_text(0), v.row_text(1)))
                            }
                            _ => None,
                        });
                        if let Ok(Some(s)) = s {
                            out = Some(s);
                            break;
                        }
                    }
                    out
                }};
            }
            let data = |this: &gpui::WeakEntity<Workspace>, cx: &mut gpui::AsyncWindowContext| {
                this.update(cx, |t, _| match t.active.and_then(|i| t.tabs.get(i)) {
                    Some(Tab::Data(v)) => Some(v.clone()),
                    _ => None,
                })
                .ok()
                .flatten()
            };
            let first = |s: &Option<(String, Option<crate::data_view::Info>, Option<String>, Option<(String, Vec<String>)>, Option<(String, Vec<String>)>)>| {
                s.as_ref().and_then(|s| s.3.clone()).map(|r| r.0)
            };

            let t0 = Instant::now();
            let _ = this.update_in(cx, |t, w, cx| {
                if let Some(i) = row_of(t, "df") {
                    t.view_as_data(i, w, cx);
                }
            });
            let df = table!();
            eprintln!("smoke-dataview: df in {:?} -> {df:?}", t0.elapsed());
            let Some(df_view) = data(&this, cx) else {
                check("no table for df", false);
                finish(false);
            };
            check("df shape", df.as_ref().and_then(|d| d.1.as_ref()).is_some_and(|i| i.shape == [5, 3]));
            let _ = df_view.update(cx, |v, cx| v.set_sort(1, cx));
            let s = table!();
            eprintln!("smoke-dataview: sort by column 1 asc -> {:?} {:?}", s.as_ref().and_then(|s| s.3.clone()), s.as_ref().and_then(|s| s.4.clone()));
            check("sort asc", first(&s).as_deref() == Some("4"));
            let _ = df_view.update(cx, |v, cx| v.set_sort(1, cx));
            let s = table!();
            eprintln!("smoke-dataview: sort desc -> {:?}", s.as_ref().and_then(|s| s.3.clone()));
            check("sort desc", first(&s).as_deref() == Some("3"));
            let _ = df_view.update(cx, |v, cx| v.set_query("price > 2 and city == 'Kyiv'", "%.2f", cx));
            let s = table!();
            eprintln!("smoke-dataview: filter+format -> total {:?}, first {:?}", s.as_ref().and_then(|s| s.1.as_ref().map(|i| i.total)), s.as_ref().and_then(|s| s.3.clone()));
            check("filter", s.as_ref().and_then(|s| s.1.as_ref()).is_some_and(|i| i.total == 2));
            check("format %.2f", s.as_ref().and_then(|s| s.3.clone()).is_some_and(|r| r.1.get(1).map(String::as_str) == Some("7.25")));
            let _ = df_view.update(cx, |v, cx| v.set_query("nope > 1", "", cx));
            let s = table!();
            eprintln!("smoke-dataview: bad filter -> error {:?}", s.as_ref().and_then(|s| s.2.clone()));
            check("bad filter error", s.as_ref().and_then(|s| s.2.clone()).is_some_and(|e| e.contains("nope")));
            let _ = df_view.update(cx, |v, cx| v.set_query("", "", cx));
            let _ = table!();

            if std::env::var_os("BRISK_SMOKE_DATAVIEW_HOLD").is_some() {
                eprintln!("smoke-dataview: holding");
                return;
            }

            // A 1M-row frame: the first page, then a page in the middle, sorted.
            let t0 = Instant::now();
            let _ = this.update_in(cx, |t, w, cx| {
                if let Some(i) = row_of(t, "big") {
                    t.view_as_data(i, w, cx);
                }
            });
            let b = table!();
            eprintln!("smoke-dataview: big first page in {:?} -> {:?}", t0.elapsed(), b.as_ref().and_then(|b| b.1.clone()));
            check("big total", b.as_ref().and_then(|b| b.1.as_ref()).is_some_and(|i| i.total == 1_000_000));
            if let Some(big) = data(&this, cx) {
                let _ = big.update(cx, |v, cx| v.set_sort(0, cx));
                let t0 = Instant::now();
                let _ = table!();
                let first_sorted = t0.elapsed();
                let t0 = Instant::now();
                let _ = big.update(cx, |v, cx| v.ensure_row(500_000, cx));
                let mut mid = None;
                for _ in 0..100 {
                    wait(50).await;
                    mid = big.read_with(cx, |v, _| v.row_text(500_000)).ok().flatten();
                    if mid.is_some() {
                        break;
                    }
                }
                eprintln!("smoke-dataview: big sorted first page {first_sorted:?}, row 500000 in {:?} -> {mid:?}", t0.elapsed());
                check("big row 500000", mid.is_some());
            }

            let _ = this.update_in(cx, |t, w, cx| {
                if let Some(i) = row_of(t, "arr") {
                    t.view_as_data(i, w, cx);
                }
            });
            let a = table!();
            eprintln!("smoke-dataview: arr -> {:?} rows {:?} {:?}", a.as_ref().and_then(|a| a.1.clone()), a.as_ref().and_then(|a| a.3.clone()), a.as_ref().and_then(|a| a.4.clone()));
            check("arr shape", a.as_ref().and_then(|a| a.1.as_ref()).is_some_and(|i| i.shape == [5, 4]));

            // Copy Value of a long string: the whole value, not the tree's short repr.
            let _ = this.update(cx, |t, cx| {
                if let Some(i) = row_of(t, "long_s") {
                    t.copy_value(i, cx);
                }
            });
            wait(1500).await;
            let clip = this.update(cx, |_, cx| cx.read_from_clipboard().and_then(|c| c.text()).unwrap_or_default()).unwrap_or_default();
            eprintln!("smoke-dataview: copy value -> {} chars, ends {:?}", clip.chars().count(), clip.chars().rev().take(6).collect::<String>().chars().rev().collect::<String>());
            check("copy value is the whole string", clip.chars().count() == 5003 && clip.ends_with("END"));
            let _ = this.update(cx, |t, cx| {
                if let Some(i) = row_of(t, "long_s") {
                    t.copy_as_expression(i, cx);
                }
            });
            wait(300).await;
            let clip = this.update(cx, |_, cx| cx.read_from_clipboard().and_then(|c| c.text()).unwrap_or_default()).unwrap_or_default();
            check("copy as expression", clip == "long_s");
            let _ = this.update(cx, |t, cx| {
                if let Some(i) = row_of(t, "long_s") {
                    t.add_row_to_watches(i, cx);
                    t.view_value(i, cx);
                }
            });
            wait(1500).await;
            let (watches, title) = this
                .update(cx, |t, cx| {
                    let title = t.active.and_then(|i| t.tabs.get(i)).map(|tab| match tab {
                        Tab::Editor(e) => format!("{} ({} chars)", e.read(cx).title(), e.read(cx).buffer.len_chars()),
                        _ => "?".into(),
                    });
                    let w = t.debug.watches.clone();
                    t.remove_watch(t.debug.watches.len().saturating_sub(1), cx);
                    (w, title)
                })
                .unwrap_or_default();
            eprintln!("smoke-dataview: watches {watches:?}, view value tab {title:?}");
            check("add to watches", watches.last().map(String::as_str) == Some("long_s"));
            check("view value", title.as_deref() == Some("long_s (value) (5003 chars)"));

            // Two steps (past `df.loc[...] = ...`): the open tables evaluate again at each stop.
            for _ in 0..2 {
                let _ = this.update_in(cx, |t, w, cx| t.dispatch(Action::StepOver, w, cx));
                wait(300).await;
                for _ in 0..100 {
                    wait(100).await;
                    if this.update(cx, |t, _| matches!(t.debug.state(), Some(State::Paused(_))) && !t.debug.frames.is_empty()).unwrap_or(false) {
                        break;
                    }
                }
            }
            wait(2500).await;
            let shape = df_view.read_with(cx, |v, _| v.info.as_ref().map(|i| i.shape.clone())).ok().flatten();
            eprintln!("smoke-dataview: after step df shape {shape:?}");
            check("table evaluated again after the step", shape.as_deref() == Some(&[6, 3][..]));
            let _ = this.update_in(cx, |t, w, cx| t.dispatch(Action::Stop, w, cx));
            wait(1500).await;
            let stale = df_view.read_with(cx, |v, _| v.stale.clone()).ok().flatten();
            eprintln!("smoke-dataview: after stop stale {stale:?}");
            check("stale after stop", stale.is_some());
            finish(ok);
        })
        .detach();
    }
}
