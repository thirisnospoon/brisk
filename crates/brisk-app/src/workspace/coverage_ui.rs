//! Run with Coverage: the run goes through coverage.py, then the Coverage tool window lists
//! the project files with their percentage and the editors show green / red gutter bars.

use super::run_marks_ui::norm;
use super::{BottomTool, Workspace};
use crate::coverage::{self, Report};
use crate::editor::Editor;
use crate::theme::Theme;
use brisk_keymap::Action;
use brisk_run::{RunConfig, RunKind};
use gpui::prelude::*;
use gpui::EntityInputHandler;
use gpui::{div, px, Context, Entity, FocusHandle, Window};
use std::path::PathBuf;

/// The Coverage tool window.
pub struct CoverageView {
    pub report: Report,
    /// The configuration that was measured.
    pub name: String,
    pub selected: usize,
    pub focus: FocusHandle,
}

/// A coverage run in progress: its tab name, interpreter and working directory.
pub struct Pending {
    name: String,
    python: PathBuf,
    cwd: PathBuf,
}

impl Workspace {
    /// Run › Run with Coverage: the selected configuration, else the current file.
    pub(super) fn run_with_coverage(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        self.reload_runs();
        let Some(cfg) = self.runs.file.selected_config().cloned().or_else(|| self.context_config(window, cx)) else {
            self.status = "Nothing to run with coverage: open a Python file or select a configuration".into();
            return;
        };
        if cfg.kind == RunKind::Compound {
            self.show_toast(format!("'{}' is a Compound: run its configurations with coverage one at a time", cfg.name), true, cx);
            return;
        }
        let Some(python) = self.config_python(&cfg) else {
            self.show_toast("No Python interpreter configured: create a .venv or install Python".into(), true, cx);
            return;
        };
        // coverage.py must be importable by the run's interpreter.
        let check = python.clone();
        let task = cx.background_executor().spawn(async move { has_coverage(&check) });
        cx.spawn_in(window, async move |this, cx| {
            let ok = task.await;
            let _ = this.update_in(cx, |this, window, cx| {
                if ok {
                    this.start_coverage_run(cfg, python, window, cx);
                } else {
                    this.cov_offer = Some(cfg);
                    cx.notify();
                }
            });
        })
        .detach();
    }

    pub(super) fn config_python(&self, cfg: &RunConfig) -> Option<PathBuf> {
        match &cfg.interpreter {
            Some(p) => Some(brisk_run::resolve(&self.root, p)),
            None => self.runs.interpreter(&self.root).map(|i| i.path),
        }
    }

    fn start_coverage_run(&mut self, cfg: RunConfig, python: PathBuf, window: &mut Window, cx: &mut Context<Self>) {
        self.save_all(cx);
        if !self.env_files_ok(&cfg, cx) {
            return;
        }
        let data = coverage::data_file(&self.root);
        if let Some(dir) = data.parent() {
            let _ = std::fs::create_dir_all(dir);
        }
        let _ = std::fs::remove_file(&data);
        let mut cmd = brisk_run::build(&self.root, &cfg, &python);
        coverage::wrap(&mut cmd, &cfg, &data);
        self.cov_pending = Some(Pending { name: cfg.name.clone(), python, cwd: cmd.cwd.clone() });
        let name = cfg.name.clone();
        self.run_command(cfg, cmd, window, cx);
        self.status = format!("Running '{name}' with coverage");
    }

    /// A Run tab finished: if it was the coverage run, read the report.
    pub(super) fn coverage_finished(&mut self, name: &str, cx: &mut Context<Self>) {
        if self.cov_pending.as_ref().is_none_or(|p| p.name != name) {
            return;
        }
        let Some(p) = self.cov_pending.take() else { return };
        let (root, data) = (self.root.clone(), coverage::data_file(&self.root));
        let out = data.with_extension("json");
        let name = p.name.clone();
        let task = cx.background_executor().spawn(async move {
            if !data.is_file() {
                return Err("the run produced no coverage data".to_string());
            }
            let res = coverage::json_command(&p.python, &data, &out, &p.cwd).output().map_err(|e| format!("cannot run coverage: {e}"))?;
            if !res.status.success() {
                let err = String::from_utf8_lossy(&res.stderr);
                return Err(err.lines().rev().find(|l| !l.trim().is_empty()).unwrap_or("coverage json failed").trim().to_string());
            }
            let text = std::fs::read_to_string(&out).map_err(|e| e.to_string())?;
            coverage::parse(&text, &root, &p.cwd)
        });
        cx.spawn(async move |this, cx| {
            let res = task.await;
            let _ = this.update(cx, |this, cx| {
                match res {
                    Ok(report) => {
                        this.status = format!("Coverage of '{name}': {:.0}% of lines in {} files", report.percent, report.files.len());
                        let focus = this.coverage.as_ref().map(|c| c.focus.clone()).unwrap_or_else(|| cx.focus_handle());
                        this.coverage = Some(CoverageView { report, name, selected: 0, focus });
                        this.bottom = Some(BottomTool::Coverage);
                        let eds: Vec<_> = this.editors().cloned().collect();
                        for ed in eds {
                            this.push_coverage(&ed, cx);
                        }
                    }
                    Err(e) => this.show_toast(format!("Coverage of '{name}': {e}"), true, cx),
                }
                cx.notify();
            });
        })
        .detach();
    }

    /// Show the current report on an editor (or clear it).
    pub(super) fn push_coverage(&self, ed: &Entity<Editor>, cx: &mut Context<Self>) {
        let Some(path) = ed.read(cx).path.clone() else { return };
        let key = norm(&path);
        let file = self.coverage.as_ref().and_then(|c| c.report.files.iter().find(|f| norm(&f.path) == key)).cloned();
        ed.update(cx, |e, cx| {
            e.set_coverage(file.as_ref().map(|f| (f.covered.as_slice(), f.missed.as_slice())));
            cx.notify();
        });
    }

    /// Hide the marks and the report (the window's ✕).
    fn clear_coverage(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        self.coverage = None;
        let eds: Vec<_> = self.editors().cloned().collect();
        for ed in eds {
            self.push_coverage(&ed, cx);
        }
        if self.bottom == Some(BottomTool::Coverage) {
            self.bottom = None;
        }
        self.focus_editor(window, cx);
        cx.notify();
    }

    pub(super) fn toggle_coverage_window(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        self.toggle_bottom(BottomTool::Coverage, window, cx);
        if let (Some(BottomTool::Coverage), Some(c)) = (self.bottom, &self.coverage) {
            window.focus(&c.focus.clone());
        }
        cx.notify();
    }

    fn coverage_open(&mut self, i: usize, window: &mut Window, cx: &mut Context<Self>) {
        let Some(path) = self.coverage.as_ref().and_then(|c| c.report.files.get(i)).map(|f| f.path.clone()) else { return };
        // The first missed line, like a jump to what still needs a test.
        let line = self.coverage.as_ref().and_then(|c| c.report.files.get(i)).and_then(|f| f.missed.iter().min().copied());
        self.open_file(&path, window, cx);
        if let (Some(l), Some(ed)) = (line, self.active_editor().cloned()) {
            ed.update(cx, |e, cx| e.go_to_line(l, cx));
        }
    }

    fn coverage_key(&mut self, key: &str, window: &mut Window, cx: &mut Context<Self>) {
        let Some(c) = self.coverage.as_mut() else { return };
        let n = c.report.files.len();
        match key {
            "down" if c.selected + 1 < n => c.selected += 1,
            "up" => c.selected = c.selected.saturating_sub(1),
            "enter" => {
                let i = c.selected;
                self.coverage_open(i, window, cx);
            }
            "escape" => self.focus_editor(window, cx),
            _ => return,
        }
        cx.notify();
    }

    pub(super) fn render_coverage(&mut self, cx: &mut Context<Self>) -> gpui::AnyElement {
        let Some(c) = &self.coverage else {
            return div().p(px(10.)).text_size(px(13.)).text_color(Theme::text_dim()).child("No coverage data: Run › Run with Coverage").into_any_element();
        };
        let rel = |p: &std::path::Path| p.strip_prefix(&self.root).unwrap_or(p).to_string_lossy().replace('\\', "/");
        let toolbar = div()
            .flex()
            .flex_shrink_0()
            .items_center()
            .gap(px(6.))
            .px(px(8.))
            .h(px(30.))
            .border_b_1()
            .border_color(Theme::divider())
            .child(
                div()
                    .id("cov-rerun")
                    .w(px(26.))
                    .h(px(24.))
                    .flex()
                    .items_center()
                    .justify_center()
                    .rounded(px(5.))
                    .cursor_pointer()
                    .text_color(Theme::accent())
                    .hover(|s| s.bg(Theme::hover_row()))
                    .active(|s| s.bg(Theme::press()))
                    .child("↻")
                    .tooltip(|_, cx| cx.new(|_| super::Tip("Run with Coverage again".into())).into())
                    .on_click(cx.listener(|this, _, w, cx| this.dispatch(Action::RunWithCoverage, w, cx))),
            )
            .child(
                div()
                    .id("cov-clear")
                    .w(px(26.))
                    .h(px(24.))
                    .flex()
                    .items_center()
                    .justify_center()
                    .rounded(px(5.))
                    .cursor_pointer()
                    .text_color(Theme::text_muted())
                    .hover(|s| s.bg(Theme::hover_row()))
                    .active(|s| s.bg(Theme::press()))
                    .child("✕")
                    .tooltip(|_, cx| cx.new(|_| super::Tip("Hide coverage (gutter marks and this report)".into())).into())
                    .on_click(cx.listener(|this, _, w, cx| this.clear_coverage(w, cx))),
            )
            .child(div().text_size(px(12.5)).text_color(Theme::text_muted()).child(format!("'{}': {:.0}% of lines covered", c.name, c.report.percent)));
        let header = div()
            .flex()
            .flex_shrink_0()
            .h(px(22.))
            .items_center()
            .px(px(8.))
            .text_size(px(12.))
            .text_color(Theme::text_dim())
            .border_b_1()
            .border_color(Theme::divider())
            .child(div().flex_1().child("File"))
            .child(div().w(px(120.)).child("% Lines"))
            .child(div().w(px(110.)).child("Covered / Total"));
        let bar = |pct: f64| {
            let w = 60.0f64;
            div()
                .w(px(w as f32))
                .h(px(6.))
                .rounded(px(3.))
                .bg(Theme::coverage_miss())
                .child(div().h_full().rounded(px(3.)).w(px((w * pct / 100.0) as f32)).bg(Theme::coverage_hit()))
        };
        let list = div()
            .id("cov-list")
            .track_focus(&c.focus)
            .flex_1()
            .min_h_0()
            .overflow_y_scroll()
            .text_size(px(13.))
            .on_key_down(cx.listener(|this, e: &gpui::KeyDownEvent, w, cx| {
                let k = e.keystroke.key.clone();
                if !e.keystroke.modifiers.modified() {
                    this.coverage_key(&k, w, cx);
                }
            }))
            .when(c.report.files.is_empty(), |d| d.child(div().p(px(10.)).text_color(Theme::text_dim()).child("No project files were executed")))
            .children(c.report.files.iter().enumerate().map(|(i, f)| {
                let sel = i == c.selected;
                div()
                    .id(("cov-row", i))
                    .w_full()
                    .h(px(22.))
                    .px(px(8.))
                    .flex()
                    .items_center()
                    .whitespace_nowrap()
                    .overflow_hidden()
                    .cursor_pointer()
                    .when(sel, |d| d.bg(Theme::selection_row()))
                    .when(!sel, |d| d.hover(|s| s.bg(Theme::hover_row())))
                    .on_mouse_down(gpui::MouseButton::Left, cx.listener(move |this, e: &gpui::MouseDownEvent, w, cx| {
                        if let Some(c) = this.coverage.as_mut() {
                            c.selected = i;
                            w.focus(&c.focus.clone());
                        }
                        if e.click_count >= 2 {
                            this.coverage_open(i, w, cx);
                        }
                        cx.notify();
                    }))
                    .child(div().flex_1().overflow_hidden().text_ellipsis().text_color(Theme::text()).child(rel(&f.path)))
                    .child(div().w(px(120.)).flex().items_center().gap(px(6.)).child(bar(f.percent)).child(div().text_size(px(12.)).child(format!("{:.0}%", f.percent))))
                    .child(div().w(px(110.)).text_size(px(12.)).text_color(Theme::text_muted()).child(format!("{} / {}", f.covered.len(), f.statements)))
            }));
        div().id("coverage-window").size_full().flex().flex_col().child(toolbar).child(header).child(list).into_any_element()
    }

    /// The balloon offering to install coverage.py into the run's interpreter.
    pub(super) fn render_cov_offer(&self, cx: &mut Context<Self>) -> Option<gpui::AnyElement> {
        let cfg = self.cov_offer.as_ref()?;
        let label = self.config_python(cfg).map(|p| self.runs.lookup(&p).label(&self.root)).unwrap_or_default();
        let button = |id: &'static str, text: &'static str, primary: bool| {
            div()
                .id(id)
                .h(px(24.))
                .px(px(10.))
                .flex()
                .items_center()
                .rounded(px(5.))
                .cursor_pointer()
                .when(primary, |d| d.bg(Theme::accent()).text_color(gpui::white()))
                .when(!primary, |d| d.border_1().border_color(Theme::border()))
                .hover(|s| s.opacity(0.9))
                .active(|s| s.bg(Theme::press()))
                .child(text)
        };
        Some(
            div()
                .id("cov-offer")
                .occlude()
                .absolute()
                .bottom(px(96.))
                .right(px(16.))
                .w(px(420.))
                .p(px(12.))
                .flex()
                .flex_col()
                .gap(px(8.))
                .rounded(px(8.))
                .bg(Theme::panel())
                .border_1()
                .border_color(Theme::divider())
                .shadow_lg()
                .text_size(px(13.))
                .text_color(Theme::text())
                .child(div().child(format!("coverage.py is not installed in {label}. Install it and run '{}' with coverage?", cfg.name)))
                .child(
                    div()
                        .flex()
                        .gap(px(8.))
                        .child(button("cov-install", "Install coverage", true).on_click(cx.listener(|this, _, w, cx| this.install_coverage(w, cx))))
                        .child(button("cov-cancel", "Cancel", false).on_click(cx.listener(|this, _, _, cx| {
                            this.cov_offer = None;
                            cx.notify();
                        }))),
                )
                .into_any_element(),
        )
    }

    fn install_coverage(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let Some(cfg) = self.cov_offer.take() else { return };
        let Some(python) = self.config_python(&cfg) else { return };
        self.status = "Installing coverage…".into();
        let (py, root) = (python.clone(), self.root.clone());
        let task = cx.background_executor().spawn(async move { crate::py_venv::install(&py, &root, &crate::py_venv::Requirements::Pyproject(vec!["coverage".into()])) });
        cx.spawn_in(window, async move |this, cx| {
            let res = task.await;
            let _ = this.update_in(cx, |this, window, cx| match res {
                Ok(()) => {
                    this.status = "coverage installed".into();
                    this.start_coverage_run(cfg, python, window, cx);
                }
                Err(e) => this.show_toast(format!("Installing coverage failed: {e}"), true, cx),
            });
        })
        .detach();
        cx.notify();
    }

    // --- Smoke --------------------------------------------------------------------------

    /// BRISK_SMOKE_COVERAGE=1|hold on an empty folder: writes `calc.py` and `test_calc.py`,
    /// runs the script and then pytest with coverage, prints the report and the gutter marks;
    /// checks that an edit hides the marks and ✕ clears them.
    pub(super) fn coverage_smoke(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let root = self.root.clone();
        let _ = std::fs::write(root.join("calc.py"), "def add(a, b):\n    return a + b\n\n\ndef sub(a, b):\n    return a - b\n\n\nif __name__ == \"__main__\":\n    print(add(2, 3))\n");
        let _ = std::fs::write(root.join("test_calc.py"), "from calc import add, sub\n\n\ndef test_add():\n    assert add(1, 2) == 3\n\n\ndef test_sub():\n    assert sub(3, 1) == 2\n");
        self.open_file(&root.join("calc.py"), window, cx);
        cx.spawn_in(window, async move |this, cx| {
            let bg = cx.background_executor().clone();
            let pause = |ms| bg.timer(std::time::Duration::from_millis(ms));
            for _ in 0..100 {
                pause(100).await;
                if this.update(cx, |t, _| t.runs.interpreter(&t.root).is_some()).unwrap_or(false) {
                    break;
                }
            }
            // =offer: in a project whose venv has no coverage.py, the install offer appears
            // (and stays for a screenshot).
            if std::env::var("BRISK_SMOKE_COVERAGE").as_deref() == Ok("offer") {
                let _ = this.update_in(cx, |this, window, cx| this.dispatch(Action::RunWithCoverage, window, cx));
                for _ in 0..100 {
                    pause(100).await;
                    if this.update(cx, |t, _| t.cov_offer.is_some()).unwrap_or(false) {
                        break;
                    }
                }
                let _ = this.update(cx, |this, _| eprintln!("smoke-coverage: offer for {:?}, interpreter {:?}", this.cov_offer.as_ref().map(|c| c.name.clone()), this.interpreter_label()));
                return;
            }
            let marks = |this: &Workspace, cx: &gpui::App| {
                this.active_editor().map(|e| {
                    let e = e.read(cx);
                    (0..e.buffer.len_lines()).filter_map(|l| e.coverage_at(l).map(|h| format!("{}{}", l + 1, if h { "+" } else { "-" }))).collect::<Vec<_>>()
                })
            };
            let report = |this: &Workspace| {
                this.coverage.as_ref().map(|c| (c.report.percent.round(), c.report.files.iter().map(|f| format!("{} {:.0}% {}/{}", f.path.file_name().unwrap().to_string_lossy(), f.percent, f.covered.len(), f.statements)).collect::<Vec<_>>()))
            };
            for (step, file) in [("script", "calc.py"), ("pytest", "test_calc.py")] {
                let _ = this.update_in(cx, |this, window, cx| {
                    this.open_file(&this.root.join(file), window, cx);
                    this.runs.file.selected = None;
                    this.coverage = None;
                    this.dispatch(Action::RunWithCoverage, window, cx);
                });
                for _ in 0..300 {
                    pause(100).await;
                    if this.update(cx, |t, _| t.coverage.is_some()).unwrap_or(false) {
                        break;
                    }
                }
                let _ = this.update_in(cx, |this, window, cx| {
                    eprintln!("smoke-coverage: {step}: window {:?}, report {:?}", this.bottom == Some(BottomTool::Coverage), report(this));
                    this.open_file(&this.root.join("calc.py"), window, cx);
                    this.push_coverage(&this.active_editor().cloned().unwrap(), cx);
                    eprintln!("smoke-coverage: {step}: calc.py marks {:?}", marks(this, cx));
                });
            }
            let hold = std::env::var("BRISK_SMOKE_COVERAGE").as_deref() == Ok("hold");
            let _ = this.update_in(cx, |this, window, cx| {
                if hold {
                    return;
                }
                // Enter in the window opens the first file at its first missed line.
                if let Some(c) = this.coverage.as_mut() {
                    c.selected = 0;
                }
                this.coverage_key("enter", window, cx);
                let at = this.active_editor().map(|e| (e.read(cx).path.clone().and_then(|p| p.file_name().map(|n| n.to_string_lossy().into_owned())), e.read(cx).caret_line_col().0 + 1));
                eprintln!("smoke-coverage: enter -> {at:?}");
                let ed = this.active_editor().cloned().unwrap();
                ed.update(cx, |e, cx| e.replace_text_in_range(None, "x = 1  # edited", window, cx));
                eprintln!("smoke-coverage: after an edit marks {:?}", marks(this, cx));
                this.clear_coverage(window, cx);
                eprintln!("smoke-coverage: cleared -> report {:?}, window {:?}", report(this), this.bottom);
                cx.quit();
            });
        })
        .detach();
    }
}

/// `python -c "import coverage"` succeeds.
fn has_coverage(python: &std::path::Path) -> bool {
    if brisk_run::pytest::has_package(python, "coverage") == Some(true) {
        return true;
    }
    let mut c = std::process::Command::new(python);
    c.args(["-c", "import coverage"]);
    #[cfg(windows)]
    {
        use std::os::windows::process::CommandExt;
        c.creation_flags(0x0800_0000);
    }
    c.output().is_ok_and(|o| o.status.success())
}
