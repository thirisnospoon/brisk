//! `BRISK_SMOKE_DATAVIEW=<file.py>` (the project needs an interpreter with pandas and numpy,
//! e.g. a `.venv`): stops at the line marked `# BREAK`, opens View as DataFrame for `df`,
//! sorts and filters it, formats floats, opens a 1M-row frame and pages to its middle, opens
//! View as Array for `arr`, checks Copy Value (whole long string), Copy as Expression, Add to
//! Watches and View Value, then steps and checks that the open tables evaluate again.
//! `BRISK_SMOKE_DATAVIEW_HOLD=1` keeps the window open on the `df` table (screenshots).

use super::debug_ui::State;
use super::{Tab, Workspace};
use brisk_keymap::Action;
use gpui::{Context, Window};
use std::path::PathBuf;
use std::time::Instant;

impl Workspace {
    pub(super) fn dataview_smoke(&mut self, file: PathBuf, window: &mut Window, cx: &mut Context<Self>) {
        self.open_file(&file, window, cx);
        let Some(ed) = self.active_editor().cloned() else { return };
        let line = ed.read(cx).buffer.text().lines().position(|l| l.contains("# BREAK")).unwrap_or(0);
        ed.update(cx, |e, cx| {
            e.set_breakpoints(&[]);
            e.go_to_line(line, cx);
        });
        self.dispatch(Action::ToggleBreakpoint, window, cx);
        cx.spawn_in(window, async move |this, cx| {
            let bg = cx.background_executor().clone();
            let wait = |ms| bg.timer(std::time::Duration::from_millis(ms));
            for _ in 0..100 {
                if this.update(cx, |t, _| t.runs.all.is_some()).unwrap_or(true) {
                    break;
                }
                wait(100).await;
            }
            let _ = this.update_in(cx, |t, w, cx| t.dispatch(Action::DebugContext, w, cx));
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

            let t0 = Instant::now();
            let _ = this.update_in(cx, |t, w, cx| {
                if let Some(i) = row_of(t, "df") {
                    t.view_as_data(i, w, cx);
                }
            });
            let df = table!();
            eprintln!("smoke-dataview: df in {:?} -> {df:?}", t0.elapsed());
            let Some(df_view) = data(&this, cx) else {
                eprintln!("smoke-dataview: FAIL no table");
                let _ = this.update(cx, |_, cx| cx.quit());
                return;
            };
            let _ = df_view.update(cx, |v, cx| v.set_sort(1, cx));
            let s = table!();
            eprintln!("smoke-dataview: sort by column 1 asc -> {:?} {:?}", s.as_ref().and_then(|s| s.3.clone()), s.as_ref().and_then(|s| s.4.clone()));
            let _ = df_view.update(cx, |v, cx| v.set_sort(1, cx));
            let s = table!();
            eprintln!("smoke-dataview: sort desc -> {:?}", s.as_ref().and_then(|s| s.3.clone()));
            let _ = df_view.update(cx, |v, cx| v.set_query("price > 2 and city == 'Kyiv'", "%.2f", cx));
            let s = table!();
            eprintln!("smoke-dataview: filter+format -> total {:?}, first {:?}", s.as_ref().and_then(|s| s.1.as_ref().map(|i| i.total)), s.as_ref().and_then(|s| s.3.clone()));
            let _ = df_view.update(cx, |v, cx| v.set_query("nope > 1", "", cx));
            let s = table!();
            eprintln!("smoke-dataview: bad filter -> error {:?}", s.as_ref().and_then(|s| s.2.clone()));
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
            }

            let _ = this.update_in(cx, |t, w, cx| {
                if let Some(i) = row_of(t, "arr") {
                    t.view_as_data(i, w, cx);
                }
            });
            let a = table!();
            eprintln!("smoke-dataview: arr -> {:?} rows {:?} {:?}", a.as_ref().and_then(|a| a.1.clone()), a.as_ref().and_then(|a| a.3.clone()), a.as_ref().and_then(|a| a.4.clone()));

            // Copy Value of a long string: the whole value, not the tree's short repr.
            let _ = this.update(cx, |t, cx| {
                if let Some(i) = row_of(t, "long_s") {
                    t.copy_value(i, cx);
                    t.copy_as_expression(i, cx);
                }
            });
            wait(1500).await;
            let _ = this.update(cx, |t, cx| {
                if let Some(i) = row_of(t, "long_s") {
                    eprintln!("smoke-dataview: tree shows {} chars of long_s", t.debug.vars[i].var.value.chars().count());
                    t.copy_value(i, cx);
                }
            });
            wait(1500).await;
            let clip = this.update(cx, |_, cx| cx.read_from_clipboard().and_then(|c| c.text()).unwrap_or_default()).unwrap_or_default();
            eprintln!("smoke-dataview: copy value -> {} chars, ends {:?}", clip.chars().count(), clip.chars().rev().take(6).collect::<String>().chars().rev().collect::<String>());
            let _ = this.update(cx, |t, cx| {
                if let Some(i) = row_of(t, "long_s") {
                    t.add_row_to_watches(i, cx);
                    t.view_value(i, cx);
                }
            });
            wait(1500).await;
            let _ = this.update(cx, |t, cx| {
                let title = t.active.and_then(|i| t.tabs.get(i)).map(|tab| match tab {
                    Tab::Editor(e) => format!("{} ({} chars)", e.read(cx).title(), e.read(cx).buffer.len_chars()),
                    _ => "?".into(),
                });
                eprintln!("smoke-dataview: watches {:?}, view value tab {title:?}", t.debug.watches);
                t.remove_watch(t.debug.watches.len().saturating_sub(1), cx);
            });

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
            let _ = this.update(cx, |_, cx| {
                let shape = df_view.read(cx).info.as_ref().map(|i| i.shape.clone());
                eprintln!("smoke-dataview: after step df shape {shape:?}, stale {:?}", df_view.read(cx).stale);
            });
            let _ = this.update_in(cx, |t, w, cx| t.dispatch(Action::Stop, w, cx));
            wait(1500).await;
            let _ = this.update(cx, |_, cx| {
                eprintln!("smoke-dataview: after stop stale {:?}", df_view.read(cx).stale);
                eprintln!("smoke-dataview: done");
                cx.quit();
            });
        })
        .detach();
    }
}
