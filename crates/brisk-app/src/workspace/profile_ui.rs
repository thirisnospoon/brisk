//! Profile: the run goes through cProfile, then the Profiler tool window shows a table of
//! functions (Name, Call Count, Own Time, Time) sortable by a click on a column header;
//! a double click or Enter opens the function.

use crate::theme::ui_px;
use super::{BottomTool, Workspace};
use crate::profile::{self, Row, SortBy};
use crate::theme::Theme;
use brisk_keymap::Action;
use brisk_run::RunKind;
use gpui::prelude::*;
use gpui::{div, px, Context, FocusHandle, Window};
use std::path::PathBuf;

/// Rows shown at most (the slowest by the current sort come first).
const MAX_ROWS: usize = 1000;

pub struct ProfileView {
    pub rows: Vec<Row>,
    pub total: f64,
    pub name: String,
    pub sort: SortBy,
    pub desc: bool,
    pub selected: usize,
    pub focus: FocusHandle,
}

pub struct Pending {
    name: String,
    python: PathBuf,
}

thread_local! {
    /// Where the column headers were painted (Name, Calls, Own, Total): smoke clicks aim here.
    static HEADERS: std::cell::RefCell<[Option<gpui::Bounds<gpui::Pixels>>; 4]> = const { std::cell::RefCell::new([None, None, None, None]) };
}

fn track(i: usize) -> impl IntoElement {
    gpui::canvas(move |b, _, _| HEADERS.with(|c| c.borrow_mut()[i] = Some(b)), |_, _, _, _| {}).absolute().top_0().left_0().size_full()
}

fn header_center(i: usize) -> Option<(f32, f32)> {
    HEADERS.with(|c| c.borrow()[i]).map(|b| (f32::from(b.center().x), f32::from(b.center().y)))
}

fn ms(s: f64) -> String {
    let ms = s * 1000.0;
    if ms >= 100.0 {
        format!("{ms:.0}")
    } else {
        format!("{ms:.1}")
    }
}

impl Workspace {
    /// Run › Profile: the selected configuration, else the current file, under cProfile.
    pub(super) fn profile_run(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        self.reload_runs();
        let Some(cfg) = self.runs.file.selected_config().cloned().or_else(|| self.context_config(window, cx)) else {
            self.status = "Nothing to profile: open a Python file or select a configuration".into();
            return;
        };
        self.profile_config(cfg, window, cx);
    }

    pub(super) fn profile_config(&mut self, cfg: brisk_run::RunConfig, window: &mut Window, cx: &mut Context<Self>) {
        if cfg.kind == RunKind::Compound {
            self.show_toast(format!("'{}' is a Compound: profile its configurations one at a time", cfg.name), true, cx);
            return;
        }
        let Some(python) = self.config_python(&cfg) else {
            self.show_toast("No Python interpreter configured: create a .venv or install Python".into(), true, cx);
            return;
        };
        if self.before_launch(&cfg, super::before_launch_ui::Then::Profile, window, cx) {
            return;
        }
        self.save_all(cx);
        if !self.env_files_ok(&cfg, cx) {
            return;
        }
        let stats = profile::stats_file(&self.root);
        if let Some(dir) = stats.parent() {
            let _ = std::fs::create_dir_all(dir);
        }
        let _ = std::fs::remove_file(&stats);
        let mut cmd = brisk_run::build(&self.root, &cfg, &python);
        profile::wrap(&mut cmd, &cfg, &stats);
        self.prof_pending = Some(Pending { name: cfg.name.clone(), python });
        let name = cfg.name.clone();
        self.run_command(cfg, cmd, window, cx);
        self.status = format!("Profiling '{name}'");
    }

    /// A Run tab finished: if it was the profiled run, read the stats.
    pub(super) fn profile_finished(&mut self, name: &str, cx: &mut Context<Self>) {
        if self.prof_pending.as_ref().is_none_or(|p| p.name != name) {
            return;
        }
        let Some(p) = self.prof_pending.take() else { return };
        let stats = profile::stats_file(&self.root);
        let out = stats.with_extension("json");
        let name = p.name.clone();
        let task = cx.background_executor().spawn(async move {
            if !stats.is_file() {
                return Err("the run wrote no profile (did it exit through os._exit?)".to_string());
            }
            let mut c = std::process::Command::new(&p.python);
            c.arg("-c").arg(profile::DUMP).arg(&stats).arg(&out);
            #[cfg(windows)]
            {
                use std::os::windows::process::CommandExt;
                c.creation_flags(0x0800_0000);
            }
            let res = c.output().map_err(|e| format!("cannot read the profile: {e}"))?;
            if !res.status.success() {
                return Err(String::from_utf8_lossy(&res.stderr).lines().last().unwrap_or("pstats failed").to_string());
            }
            profile::parse(&std::fs::read_to_string(&out).map_err(|e| e.to_string())?)
        });
        cx.spawn(async move |this, cx| {
            let res = task.await;
            let _ = this.update(cx, |this, cx| {
                match res {
                    Ok((mut rows, total)) => {
                        profile::sort(&mut rows, SortBy::Own, true, &this.root);
                        this.status = format!("Profile of '{name}': {} functions, {} ms", rows.len(), ms(total));
                        let focus = this.profile.as_ref().map(|p| p.focus.clone()).unwrap_or_else(|| cx.focus_handle());
                        this.profile = Some(ProfileView { rows, total, name, sort: SortBy::Own, desc: true, selected: 0, focus });
                        this.bottom = Some(BottomTool::Profiler);
                    }
                    Err(e) => this.show_toast(format!("Profile of '{name}': {e}"), true, cx),
                }
                cx.notify();
            });
        })
        .detach();
    }

    /// A click on a column header: sort by it, or flip the order.
    pub(super) fn profile_sort(&mut self, by: SortBy, cx: &mut Context<Self>) {
        let root = self.root.clone();
        let Some(p) = self.profile.as_mut() else { return };
        p.desc = if p.sort == by { !p.desc } else { by != SortBy::Name };
        p.sort = by;
        profile::sort(&mut p.rows, by, p.desc, &root);
        p.selected = 0;
        cx.notify();
    }

    fn profile_open(&mut self, i: usize, window: &mut Window, cx: &mut Context<Self>) {
        let Some(row) = self.profile.as_ref().and_then(|p| p.rows.get(i)).cloned() else { return };
        match row.source() {
            Some((path, line)) => {
                self.open_file(&path, window, cx);
                if let Some(ed) = self.active_editor().cloned() {
                    ed.update(cx, |e, cx| e.go_to_line(line.saturating_sub(1), cx));
                }
            }
            None => self.status = format!("{} has no Python source", row.func),
        }
    }

    pub(super) fn toggle_profiler_window(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        self.toggle_bottom(BottomTool::Profiler, window, cx);
        if let (Some(BottomTool::Profiler), Some(p)) = (self.bottom, &self.profile) {
            window.focus(&p.focus.clone());
        }
        cx.notify();
    }

    fn profile_key(&mut self, key: &str, window: &mut Window, cx: &mut Context<Self>) {
        let Some(p) = self.profile.as_mut() else { return };
        let n = p.rows.len().min(MAX_ROWS);
        match key {
            "down" if p.selected + 1 < n => p.selected += 1,
            "up" => p.selected = p.selected.saturating_sub(1),
            "enter" => {
                let i = p.selected;
                self.profile_open(i, window, cx);
            }
            "escape" => self.focus_editor(window, cx),
            _ => return,
        }
        cx.notify();
    }

    pub(super) fn render_profiler(&mut self, cx: &mut Context<Self>) -> gpui::AnyElement {
        let Some(p) = &self.profile else {
            return div().p(px(10.)).text_size(ui_px(13.)).text_color(Theme::text_dim()).child("No profile yet: Run › Profile").into_any_element();
        };
        let root = self.root.clone();
        let col = |i: usize, by: SortBy, title: &'static str, w: Option<f32>| {
            let on = p.sort == by;
            let arrow = if on { if p.desc { " ▾" } else { " ▴" } } else { "" };
            div()
                .id(("prof-h", i))
                .relative()
                .h_full()
                .flex()
                .items_center()
                .px(px(6.))
                .cursor_pointer()
                .when_some(w, |d, w| d.w(px(w)).flex_shrink_0().justify_end())
                .when(w.is_none(), |d| d.flex_1())
                .when(on, |d| d.text_color(Theme::text()))
                .hover(|s| s.bg(Theme::hover_row()))
                .active(|s| s.bg(Theme::press()))
                .child(format!("{title}{arrow}"))
                .child(track(i))
                .on_click(cx.listener(move |this, _, _, cx| this.profile_sort(by, cx)))
        };
        let toolbar = div()
            .flex()
            .flex_shrink_0()
            .items_center()
            .gap(px(6.))
            .pl(px(12.)).pr(px(6.))
            .h(px(crate::ui_kit::header_h()))
            .border_b_1()
            .border_color(Theme::divider())
            .child(crate::ui_kit::tool_title("Profiler"))
            .child(
                div()
                    .id("prof-rerun")
                    .size(ui_px(24.))
                    .flex()
                    .items_center()
                    .justify_center()
                    .rounded(px(5.))
                    .cursor_pointer()
                    .text_color(Theme::accent())
                    .hover(|s| s.bg(Theme::hover_row()))
                    .active(|s| s.bg(Theme::press()))
                    .child("↻")
                    .tooltip(|_, cx| cx.new(|_| super::Tip("Profile again".into())).into())
                    .on_click(cx.listener(|this, _, w, cx| this.dispatch(Action::Profile, w, cx))),
            )
            .child(div().text_size(ui_px(12.5)).text_color(Theme::text_muted()).child(format!("'{}': {} functions, {} ms total (times in ms)", p.name, p.rows.len(), ms(p.total))));
        let header = div()
            .flex()
            .flex_shrink_0()
            .h(ui_px(24.))
            .px(px(2.))
            .text_size(ui_px(12.))
            .text_color(Theme::text_dim())
            .border_b_1()
            .border_color(Theme::divider())
            .child(col(0, SortBy::Name, "Name", None))
            .child(col(1, SortBy::Calls, "Call Count", Some(100.)))
            .child(col(2, SortBy::Own, "Own Time", Some(100.)))
            .child(col(3, SortBy::Total, "Time", Some(100.)));
        let num = |s: String| div().w(px(100.)).flex_shrink_0().px(px(6.)).flex().justify_end().child(s);
        let list = div()
            .id("prof-list")
            .track_focus(&p.focus)
            .flex_1()
            .min_h_0()
            .overflow_y_scroll()
            .text_size(ui_px(12.5))
            .font_family(crate::theme::mono_font())
            .on_key_down(cx.listener(|this, e: &gpui::KeyDownEvent, w, cx| {
                if !e.keystroke.modifiers.modified() {
                    let k = e.keystroke.key.clone();
                    this.profile_key(&k, w, cx);
                }
            }))
            .children(p.rows.iter().take(MAX_ROWS).enumerate().map(|(i, r)| {
                let sel = i == p.selected;
                let calls = if r.calls != r.primitive { format!("{}/{}", r.calls, r.primitive) } else { r.calls.to_string() };
                div()
                    .id(("prof-row", i))
                    .w_full()
                    .h(ui_px(20.))
                    .px(px(2.))
                    .flex()
                    .items_center()
                    .whitespace_nowrap()
                    .cursor_pointer()
                    .when(sel, |d| d.bg(Theme::selection_row()))
                    .when(!sel, |d| d.hover(|s| s.bg(Theme::hover_row())))
                    .on_mouse_down(gpui::MouseButton::Left, cx.listener(move |this, e: &gpui::MouseDownEvent, w, cx| {
                        if let Some(p) = this.profile.as_mut() {
                            p.selected = i;
                            w.focus(&p.focus.clone());
                        }
                        if e.click_count >= 2 {
                            this.profile_open(i, w, cx);
                        }
                        cx.notify();
                    }))
                    .child(div().flex_1().min_w_0().px(px(6.)).overflow_hidden().text_ellipsis().text_color(if r.file == "~" { Theme::text_muted() } else { Theme::text() }).child(r.name(&root)))
                    .child(num(calls))
                    .child(num(ms(r.own)))
                    .child(num(ms(r.total)))
            }));
        div().id("profiler-window").size_full().flex().flex_col().child(toolbar).child(header).child(list).into_any_element()
    }

    // --- Smoke --------------------------------------------------------------------------

    /// BRISK_SMOKE_PROFILE=1|hold on an empty folder: writes `prof.py` (a slow loop, a
    /// recursive fib, many cheap calls), profiles it, checks the top rows, sorts by real
    /// clicks on the Call Count and Name headers, and opens a function by Enter.
    pub(super) fn profile_smoke(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let root = self.root.clone();
        let _ = std::fs::write(
            root.join("prof.py"),
            "import time\n\n\ndef slow():\n    t = time.perf_counter()\n    while time.perf_counter() - t < 0.2:\n        pass\n\n\ndef fib(n):\n    return n if n < 2 else fib(n - 1) + fib(n - 2)\n\n\ndef cheap(x):\n    return x * 2\n\n\nif __name__ == \"__main__\":\n    slow()\n    fib(15)\n    print(sum(cheap(i) for i in range(5000)))\n",
        );
        self.open_file(&root.join("prof.py"), window, cx);
        HEADERS.with(|c| *c.borrow_mut() = [None, None, None, None]);
        cx.spawn_in(window, async move |this, cx| {
            let bg = cx.background_executor().clone();
            let pause = |ms| bg.timer(std::time::Duration::from_millis(ms));
            for _ in 0..100 {
                pause(100).await;
                if this.update(cx, |t, _| t.runs.interpreter(&t.root).is_some()).unwrap_or(false) {
                    break;
                }
            }
            let _ = this.update_in(cx, |this, window, cx| this.dispatch(Action::Profile, window, cx));
            for _ in 0..300 {
                pause(100).await;
                if this.update(cx, |t, _| t.profile.is_some()).unwrap_or(false) {
                    break;
                }
            }
            let top = |this: &Workspace, n: usize| {
                this.profile.as_ref().map(|p| p.rows.iter().take(n).map(|r| format!("{} calls={} own={} total={}", r.name(&this.root), r.calls, ms(r.own), ms(r.total))).collect::<Vec<_>>()).unwrap_or_default()
            };
            let _ = this.update(cx, |this, _| {
                eprintln!("smoke-profile: window {}, status {:?}", this.bottom == Some(BottomTool::Profiler), this.status);
                eprintln!("smoke-profile: by own time {:?}", top(this, 3));
            });
            if std::env::var("BRISK_SMOKE_PROFILE").as_deref() == Ok("hold") {
                return;
            }
            // Real clicks on the headers.
            for (i, what) in [(1usize, "call count"), (0, "name")] {
                for _ in 0..30 {
                    pause(100).await;
                    if header_center(i).is_some() {
                        break;
                    }
                }
                let _ = this.update_in(cx, |_, window, _| {
                    let scale = window.scale_factor();
                    if let Some((x, y)) = header_center(i) {
                        super::win_input::click(x * scale, y * scale);
                    }
                });
                pause(400).await;
                let _ = this.update(cx, |this, _| {
                    let p = this.profile.as_ref();
                    eprintln!("smoke-profile: click {what} -> sort {:?} desc {:?}, top {:?}", p.map(|p| p.sort), p.map(|p| p.desc), top(this, 2));
                });
            }
            let _ = this.update_in(cx, |this, window, cx| {
                // Back to own time; Enter on the fib row opens its definition.
                this.profile_sort(SortBy::Own, cx);
                if let Some(p) = this.profile.as_mut() {
                    p.selected = p.rows.iter().position(|r| r.func == "fib").unwrap_or(0);
                }
                this.profile_key("enter", window, cx);
                let at = this.active_editor().map(|e| (e.read(cx).path.clone().and_then(|p| p.file_name().map(|n| n.to_string_lossy().into_owned())), e.read(cx).caret_line_col().0 + 1));
                eprintln!("smoke-profile: enter on fib -> {at:?}");
                cx.quit();
            });
        })
        .detach();
    }
}
