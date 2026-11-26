//! Run with Coverage: the run goes through coverage.py, then the Coverage tool window lists
//! the project files with their percentage and the editors show green / red gutter bars.

use super::run_marks_ui::norm;
use super::{BottomTool, Workspace};
use crate::editor::Editor;
use crate::theme::ui_px;
use crate::theme::Theme;
use brisk_keymap::Action;
use brisk_run::coverage::{self, Report};
use brisk_run::{RunConfig, RunKind};
use gpui::prelude::*;
#[cfg(feature = "smoke")]
use gpui::EntityInputHandler;
use gpui::{div, px, App, Context, Entity, EventEmitter, FocusHandle, Window};
use std::path::PathBuf;

/// The Coverage tool window as `Workspace` holds it: its focus and its model, which is
/// created when the first report comes in or the window first opens.
pub struct Coverage {
    pub focus: FocusHandle,
    model: Option<Entity<CoverageModel>>,
}

impl Coverage {
    pub fn new(cx: &mut Context<Workspace>) -> Self {
        Self { focus: cx.focus_handle(), model: None }
    }

    /// The report shown, if there is one.
    pub fn report<'a>(&'a self, cx: &'a App) -> Option<&'a Report> {
        self.model.as_ref().and_then(|m| m.read(cx).report.as_ref())
    }

    /// Drops the report.
    fn clear(&self, cx: &mut App) {
        if let Some(m) = &self.model {
            m.update(cx, |m, cx| m.clear(cx));
        }
    }
}

/// What the Coverage model asks of the workspace.
pub enum CoverageEvent {
    /// Open a file, at its first missed line if it has one.
    Open(PathBuf, Option<usize>),
    /// Esc: back to the editor.
    FocusEditor,
    /// A header button that runs an action (Run with Coverage again).
    Dispatch(Action),
    /// ✕: hide the report and the gutter marks.
    Clear,
}

/// The last coverage report, and the window that lists its files.
pub struct CoverageModel {
    root: PathBuf,
    focus: FocusHandle,
    report: Option<Report>,
    /// The configuration that was measured.
    name: String,
    selected: usize,
    /// The list's scroll: Up / Down keep the selected row in view.
    scroll: gpui::ScrollHandle,
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
        self.reload_runs(cx);
        let Some(cfg) = self.runs.file.selected_config().cloned().or_else(|| self.context_config(window, cx)) else {
            self.status = "Nothing to run with coverage: open a Python file or select a configuration".into();
            return;
        };
        if cfg.kind == RunKind::Compound {
            self.show_toast(format!("'{}' is a Compound: run its configurations with coverage one at a time", cfg.name), true, cx);
            return;
        }
        if self.remote_blocks(&cfg, "Coverage", cx) {
            return;
        }
        let Some(python) = self.config_python(&cfg, cx) else {
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
                    this.install_offer =
                        Some(super::install_offer_ui::InstallOffer { package: "coverage", python, then: super::install_offer_ui::Then::Coverage(cfg) });
                    cx.notify();
                }
            });
        })
        .detach();
    }

    pub(super) fn config_python(&self, cfg: &RunConfig, cx: &gpui::App) -> Option<PathBuf> {
        match &cfg.interpreter {
            Some(p) => Some(brisk_run::resolve(&self.root, p)),
            None => self.interpreter(cx).map(|i| i.path),
        }
    }

    pub(super) fn start_coverage_run(&mut self, cfg: RunConfig, python: PathBuf, window: &mut Window, cx: &mut Context<Self>) {
        if self.before_launch(&cfg, super::before_launch_ui::Then::Coverage(python.clone()), window, cx) {
            return;
        }
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
    pub(super) fn coverage_finished(&mut self, name: &str, window: &mut Window, cx: &mut Context<Self>) {
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
        cx.spawn_in(window, async move |this, cx| {
            let res = task.await;
            let _ = this.update_in(cx, |this, window, cx| {
                match res {
                    Ok(report) => {
                        this.status = format!("Coverage of '{name}': {:.0}% of lines in {} files", report.percent, report.files.len());
                        let m = this.coverage_model(window, cx);
                        m.update(cx, |m, cx| m.set_report(report, name, cx));
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
        let file = self.coverage.report(cx).and_then(|r| r.files.iter().find(|f| norm(&f.path) == key)).cloned();
        ed.update(cx, |e, cx| {
            e.set_coverage(file.as_ref().map(|f| (f.covered.as_slice(), f.missed.as_slice())));
            cx.notify();
        });
    }

    /// Hide the marks and the report (the window's ✕).
    fn clear_coverage(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        self.coverage.clear(cx);
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

    /// The Coverage model, created on first use.
    fn coverage_model(&mut self, window: &mut Window, cx: &mut Context<Self>) -> Entity<CoverageModel> {
        if let Some(m) = &self.coverage.model {
            return m.clone();
        }
        let m = cx.new(|_| CoverageModel::new(self.root.clone(), self.coverage.focus.clone()));
        cx.subscribe_in(&m, window, |this, _, ev, window, cx| this.on_coverage_event(ev, window, cx)).detach();
        self.coverage.model = Some(m.clone());
        m
    }

    fn on_coverage_event(&mut self, ev: &CoverageEvent, window: &mut Window, cx: &mut Context<Self>) {
        match ev {
            CoverageEvent::Open(path, line) => {
                self.open_file(path, window, cx);
                if let (Some(l), Some(ed)) = (line, self.active_editor().cloned()) {
                    ed.update(cx, |e, cx| e.go_to_line(*l, cx));
                }
            }
            CoverageEvent::FocusEditor => self.focus_editor(window, cx),
            CoverageEvent::Dispatch(action) => self.dispatch(*action, window, cx),
            CoverageEvent::Clear => self.clear_coverage(window, cx),
        }
        cx.notify();
    }

    pub(super) fn toggle_coverage_window(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        self.coverage_model(window, cx);
        self.toggle_bottom(BottomTool::Coverage, window, cx);
        if self.bottom == Some(BottomTool::Coverage) && self.coverage.report(cx).is_some() {
            window.focus(&self.coverage.focus);
        }
        cx.notify();
    }

    pub(super) fn render_coverage(&self) -> gpui::AnyElement {
        match &self.coverage.model {
            Some(m) => m.clone().into_any_element(),
            None => no_report().into_any_element(),
        }
    }

    // --- Smoke --------------------------------------------------------------------------

    /// BRISK_SMOKE_COVERAGE=1|hold on an empty folder: writes `calc.py` and `test_calc.py`,
    /// runs the script and then pytest with coverage, prints the report and the gutter marks;
    /// checks that an edit hides the marks and ✕ clears them.
    #[cfg(feature = "smoke")]
    pub(super) fn coverage_smoke(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let root = self.root.clone();
        let _ = std::fs::write(
            root.join("calc.py"),
            "def add(a, b):\n    return a + b\n\n\ndef sub(a, b):\n    return a - b\n\n\nif __name__ == \"__main__\":\n    print(add(2, 3))\n",
        );
        let _ = std::fs::write(
            root.join("test_calc.py"),
            "from calc import add, sub\n\n\ndef test_add():\n    assert add(1, 2) == 3\n\n\ndef test_sub():\n    assert sub(3, 1) == 2\n",
        );
        self.open_file(&root.join("calc.py"), window, cx);
        cx.spawn_in(window, async move |this, cx| {
            let bg = cx.background_executor().clone();
            let pause = |ms| bg.timer(std::time::Duration::from_millis(ms));
            for _ in 0..100 {
                pause(100).await;
                if this.update(cx, |t, cx| t.interpreter(cx).is_some()).unwrap_or(false) {
                    break;
                }
            }
            // =offer: in a project whose venv has no coverage.py, the install offer appears
            // (and stays for a screenshot).
            if std::env::var("BRISK_SMOKE_COVERAGE").as_deref() == Ok("offer") {
                let _ = this.update_in(cx, |this, window, cx| this.dispatch(Action::RunWithCoverage, window, cx));
                for _ in 0..100 {
                    pause(100).await;
                    if this.update(cx, |t, _| t.install_offer.is_some()).unwrap_or(false) {
                        break;
                    }
                }
                let _ = this.update(cx, |this, cx| {
                    eprintln!(
                        "smoke-coverage: offer for {:?}, interpreter {:?}",
                        this.install_offer.as_ref().and_then(|o| match &o.then {
                            super::install_offer_ui::Then::Coverage(c) => Some(c.name.clone()),
                            _ => None,
                        }),
                        this.interpreter_label(cx)
                    )
                });
                return;
            }
            let marks = |this: &Workspace, cx: &gpui::App| {
                this.active_editor().map(|e| {
                    let e = e.read(cx);
                    (0..e.buffer.len_lines()).filter_map(|l| e.coverage_at(l).map(|h| format!("{}{}", l + 1, if h { "+" } else { "-" }))).collect::<Vec<_>>()
                })
            };
            let report = |this: &Workspace, cx: &gpui::App| {
                this.coverage.report(cx).map(|r| {
                    (
                        r.percent.round(),
                        r.files
                            .iter()
                            .map(|f| {
                                format!("{} {:.0}% {}/{}", f.path.file_name().unwrap_or_default().to_string_lossy(), f.percent, f.covered.len(), f.statements)
                            })
                            .collect::<Vec<_>>(),
                    )
                })
            };
            for (step, file) in [("script", "calc.py"), ("pytest", "test_calc.py")] {
                let _ = this.update_in(cx, |this, window, cx| {
                    this.open_file(&this.root.join(file), window, cx);
                    this.runs.file.selected = None;
                    this.coverage.clear(cx);
                    this.dispatch(Action::RunWithCoverage, window, cx);
                });
                for _ in 0..300 {
                    pause(100).await;
                    if this.update(cx, |t, cx| t.coverage.report(cx).is_some()).unwrap_or(false) {
                        break;
                    }
                }
                let _ = this.update_in(cx, |this, window, cx| {
                    eprintln!("smoke-coverage: {step}: window {:?}, report {:?}", this.bottom == Some(BottomTool::Coverage), report(this, cx));
                    this.open_file(&this.root.join("calc.py"), window, cx);
                    if let Some(ed) = this.active_editor().cloned() {
                        this.push_coverage(&ed, cx);
                    }
                    eprintln!("smoke-coverage: {step}: calc.py marks {:?}", marks(this, cx));
                });
            }
            let hold = std::env::var("BRISK_SMOKE_COVERAGE").as_deref() == Ok("hold");
            let _ = this.update_in(cx, |this, window, cx| {
                if hold {
                    return;
                }
                // Enter in the window opens the first file at its first missed line.
                if let Some(m) = this.coverage.model.clone() {
                    if let Some(ev) = m.update(cx, |m, cx| {
                        m.selected = 0;
                        m.key("enter", cx)
                    }) {
                        this.on_coverage_event(&ev, window, cx);
                    }
                }
                let at = this.active_editor().map(|e| {
                    (e.read(cx).path.clone().and_then(|p| p.file_name().map(|n| n.to_string_lossy().into_owned())), e.read(cx).caret_line_col().0 + 1)
                });
                eprintln!("smoke-coverage: enter -> {at:?}");
                if let Some(ed) = this.active_editor().cloned() {
                    ed.update(cx, |e, cx| e.replace_text_in_range(None, "x = 1  # edited", window, cx));
                }
                eprintln!("smoke-coverage: after an edit marks {:?}", marks(this, cx));
                this.clear_coverage(window, cx);
                eprintln!("smoke-coverage: cleared -> report {:?}, window {:?}", report(this, cx), this.bottom);
                cx.quit();
            });
        })
        .detach();
    }
}

impl EventEmitter<CoverageEvent> for CoverageModel {}

impl CoverageModel {
    /// No report yet, for the project at `root`; `focus` is the tool window's.
    pub fn new(root: PathBuf, focus: FocusHandle) -> Self {
        Self { root, focus, report: None, name: String::new(), selected: 0, scroll: gpui::ScrollHandle::new() }
    }

    /// The report of the configuration `name`.
    pub fn set_report(&mut self, report: Report, name: String, cx: &mut Context<Self>) {
        self.report = Some(report);
        self.name = name;
        self.selected = 0;
        cx.notify();
    }

    pub fn clear(&mut self, cx: &mut Context<Self>) {
        self.report = None;
        cx.notify();
    }

    /// Selects row `i`; the file and its first missed line, like a jump to what still needs
    /// a test.
    fn open(&self, i: usize) -> Option<CoverageEvent> {
        let f = self.report.as_ref()?.files.get(i)?;
        Some(CoverageEvent::Open(f.path.clone(), f.missed.iter().min().copied()))
    }

    /// A key pressed in the list with no modifier held; what the workspace has to do.
    fn key(&mut self, key: &str, cx: &mut Context<Self>) -> Option<CoverageEvent> {
        let n = self.report.as_ref().map_or(0, |r| r.files.len());
        let ev = match key {
            "down" if self.selected + 1 < n => {
                self.selected += 1;
                None
            }
            "up" => {
                self.selected = self.selected.saturating_sub(1);
                None
            }
            "enter" => self.open(self.selected),
            "escape" => Some(CoverageEvent::FocusEditor),
            _ => return None,
        };
        self.scroll.scroll_to_item(self.selected);
        cx.notify();
        ev
    }
}

/// The window before the first report.
fn no_report() -> gpui::Div {
    div().p(px(10.)).text_size(ui_px(13.)).text_color(Theme::text_dim()).child("No coverage data: Run › Run with Coverage")
}

impl Render for CoverageModel {
    fn render(&mut self, _window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let Some(report) = &self.report else { return no_report().into_any_element() };
        let rel = |p: &std::path::Path| p.strip_prefix(&self.root).unwrap_or(p).to_string_lossy().replace('\\', "/");
        let toolbar = div()
            .flex()
            .flex_shrink_0()
            .items_center()
            .gap(px(6.))
            .pl(px(12.))
            .pr(px(6.))
            .h(px(crate::ui_kit::header_h()))
            .border_b_1()
            .border_color(Theme::divider())
            .child(crate::ui_kit::tool_title("Coverage"))
            .child(
                div()
                    .id("cov-rerun")
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
                    .tooltip(|_, cx| cx.new(|_| super::Tip("Run with Coverage again".into())).into())
                    .on_click(cx.listener(|_, _, _, cx| cx.emit(CoverageEvent::Dispatch(Action::RunWithCoverage)))),
            )
            .child(
                div()
                    .id("cov-clear")
                    .size(ui_px(24.))
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
                    .on_click(cx.listener(|_, _, _, cx| cx.emit(CoverageEvent::Clear))),
            )
            .child(div().text_size(ui_px(12.5)).text_color(Theme::text_muted()).child(format!("'{}': {:.0}% of lines covered", self.name, report.percent)));
        let header = div()
            .flex()
            .flex_shrink_0()
            .h(ui_px(22.))
            .items_center()
            .px(px(8.))
            .text_size(ui_px(12.))
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
                .h(ui_px(6.))
                .rounded(px(3.))
                .bg(Theme::coverage_miss())
                .child(div().h_full().rounded(px(3.)).w(px((w * pct / 100.0) as f32)).bg(Theme::coverage_hit()))
        };
        let list = div()
            .id("cov-list")
            .track_focus(&self.focus)
            .flex_1()
            .min_h_0()
            .overflow_y_scroll()
            .track_scroll(&self.scroll)
            .text_size(ui_px(13.))
            .on_key_down(cx.listener(|this, e: &gpui::KeyDownEvent, _, cx| {
                if !e.keystroke.modifiers.modified() {
                    if let Some(ev) = this.key(&e.keystroke.key, cx) {
                        cx.emit(ev);
                    }
                }
            }))
            .when(report.files.is_empty(), |d| d.child(div().p(px(10.)).text_color(Theme::text_dim()).child("No project files were executed")))
            .children(report.files.iter().enumerate().map(|(i, f)| {
                let sel = i == self.selected;
                div()
                    .id(("cov-row", i))
                    .w_full()
                    .h(ui_px(22.))
                    .px(px(8.))
                    .flex()
                    .items_center()
                    .whitespace_nowrap()
                    .overflow_hidden()
                    .cursor_pointer()
                    .when(sel, |d| d.bg(Theme::selection_row()))
                    .when(!sel, |d| d.hover(|s| s.bg(Theme::hover_row())))
                    .on_mouse_down(
                        gpui::MouseButton::Left,
                        cx.listener(move |this, e: &gpui::MouseDownEvent, w, cx| {
                            this.selected = i;
                            w.focus(&this.focus);
                            if e.click_count >= 2 {
                                if let Some(ev) = this.open(i) {
                                    cx.emit(ev);
                                }
                            }
                            cx.notify();
                        }),
                    )
                    .child(div().flex_1().overflow_hidden().text_ellipsis().text_color(Theme::text()).child(rel(&f.path)))
                    .child(
                        div()
                            .w(px(120.))
                            .flex()
                            .items_center()
                            .gap(px(6.))
                            .child(bar(f.percent))
                            .child(div().text_size(ui_px(12.)).child(format!("{:.0}%", f.percent))),
                    )
                    .child(div().w(px(110.)).text_size(ui_px(12.)).text_color(Theme::text_muted()).child(format!("{} / {}", f.covered.len(), f.statements)))
            }));
        div().id("coverage-window").size_full().flex().flex_col().child(toolbar).child(header).child(list).into_any_element()
    }
}

/// `python -c "import coverage"` succeeds.
fn has_coverage(python: &std::path::Path) -> bool {
    if brisk_run::pytest::has_package(python, "coverage") == Some(true) {
        return true;
    }
    let mut c = std::process::Command::new(python);
    c.args(["-c", "import coverage"]);
    brisk_platform::process::no_window(&mut c);
    c.output().is_ok_and(|o| o.status.success())
}
