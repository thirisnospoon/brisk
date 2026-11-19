//! Services (Alt+8), like PyCharm's Run Dashboard: every Run tab on the left with its
//! status, ↻ Rerun and ■ Stop per row; the selected run's console (or the database
//! console's results) on the right.

use crate::theme::ui_px;
use super::{BottomTool, Workspace};
use crate::terminal_view::RunInfo;
use crate::theme::Theme;
use gpui::prelude::*;
use gpui::{div, px, Context, FocusHandle, Window};
use std::collections::HashMap;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Item {
    Run(u64),
    Database,
}

pub struct Services {
    pub selected: Option<Item>,
    pub focus: FocusHandle,
}

thread_local! {
    /// Where each row's buttons were painted: smoke clicks aim here.
    static BUTTONS: std::cell::RefCell<HashMap<(u64, &'static str), gpui::Bounds<gpui::Pixels>>> = std::cell::RefCell::new(HashMap::new());
}

fn track(id: u64, what: &'static str) -> impl IntoElement {
    gpui::canvas(move |b, _, _| BUTTONS.with(|c| c.borrow_mut().insert((id, what), b)), |_, _, _, _| {}).absolute().top_0().left_0().size_full()
}

fn button_center(id: u64, what: &'static str) -> Option<(f32, f32)> {
    BUTTONS.with(|c| c.borrow().get(&(id, what)).map(|b| (f32::from(b.center().x), f32::from(b.center().y))))
}

/// "1.2 s", "3 min 4 s".
fn took(d: std::time::Duration) -> String {
    let s = d.as_secs_f64();
    if s < 60.0 {
        format!("{s:.1} s")
    } else {
        format!("{} min {} s", (s / 60.0) as u64, s as u64 % 60)
    }
}

/// Status glyph, its color and the detail after the name.
fn status(r: &RunInfo) -> (&'static str, gpui::Hsla, String) {
    match (r.running, r.code) {
        (true, _) => ("▶", Theme::file_added(), "running".into()),
        (false, Some(0)) => ("✓", Theme::file_added(), r.tests.clone().unwrap_or_else(|| format!("exit code 0 · {}", took(r.elapsed)))),
        (false, Some(c)) => ("✗", Theme::error(), r.tests.clone().unwrap_or_else(|| format!("exit code {c} · {}", took(r.elapsed)))),
        (false, None) => ("■", Theme::text_muted(), format!("terminated · {}", took(r.elapsed))),
    }
}

impl Workspace {
    pub(super) fn runs_list(&self, cx: &gpui::App) -> Vec<RunInfo> {
        self.runs.panel.as_ref().map(|p| p.read(cx).runs_info()).unwrap_or_default()
    }

    /// Alt+8: open Services on the active run (else the database console).
    pub(super) fn toggle_services_window(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        self.toggle_bottom(BottomTool::Services, window, cx);
        if self.bottom != Some(BottomTool::Services) {
            return;
        }
        let valid = |s: &Option<Item>, runs: &[RunInfo]| match s {
            Some(Item::Run(id)) => runs.iter().any(|r| r.id == *id),
            Some(Item::Database) => self.db.results.is_some(),
            None => false,
        };
        let runs = self.runs_list(cx);
        if !valid(&self.services.selected, &runs) {
            self.services.selected = runs.iter().find(|r| r.active).map(|r| Item::Run(r.id)).or(self.db.results.as_ref().map(|_| Item::Database));
        }
        window.focus(&self.services.focus.clone());
        cx.notify();
    }

    pub(super) fn services_select(&mut self, item: Item, cx: &mut Context<Self>) {
        self.services.selected = Some(item);
        if let (Item::Run(id), Some(p)) = (item, self.runs.panel.clone()) {
            p.update(cx, |p, cx| {
                p.select_run(id);
                cx.notify();
            });
        }
        cx.notify();
    }

    pub(super) fn services_rerun(&mut self, id: u64, window: &mut Window, cx: &mut Context<Self>) {
        self.services_select(Item::Run(id), cx);
        self.rerun_active(window, cx);
        // The Run window opens on a rerun; Services stays where the user is.
        self.bottom = Some(BottomTool::Services);
        let active = self.runs_list(cx).into_iter().find(|r| r.active).map(|r| r.id);
        self.services.selected = active.map(Item::Run).or(self.services.selected);
        cx.notify();
    }

    pub(super) fn services_stop(&mut self, id: u64, cx: &mut Context<Self>) {
        if let Some(p) = self.runs.panel.clone() {
            p.update(cx, |p, cx| {
                p.stop_run(id);
                cx.notify();
            });
        }
    }

    /// The tree's keys: ↑/↓ select, Delete stops, Escape returns to the editor.
    pub(super) fn services_key(&mut self, e: &gpui::KeyDownEvent, window: &mut Window, cx: &mut Context<Self>) -> bool {
        if !self.services.focus.is_focused(window) || e.keystroke.modifiers.modified() {
            return false;
        }
        let mut items: Vec<Item> = self.runs_list(cx).iter().map(|r| Item::Run(r.id)).collect();
        if self.db.results.is_some() {
            items.push(Item::Database);
        }
        let at = self.services.selected.and_then(|s| items.iter().position(|i| *i == s));
        match e.keystroke.key.as_str() {
            "down" | "up" if !items.is_empty() => {
                let i = match (at, e.keystroke.key.as_str()) {
                    (Some(i), "down") => (i + 1).min(items.len() - 1),
                    (Some(i), _) => i.saturating_sub(1),
                    (None, _) => 0,
                };
                self.services_select(items[i], cx);
            }
            "delete" => {
                if let Some(Item::Run(id)) = self.services.selected {
                    self.services_stop(id, cx);
                }
            }
            "escape" => self.focus_editor(window, cx),
            _ => return false,
        }
        cx.notify();
        true
    }

    pub(super) fn render_services(&mut self, cx: &mut Context<Self>) -> gpui::AnyElement {
        let runs = self.runs_list(cx);
        let sel = self.services.selected;
        let small = |id: (&'static str, u64), glyph: &'static str, color: gpui::Hsla, tip: &'static str| {
            div()
                .id(id)
                .relative()
                .w(px(22.))
                .h(ui_px(20.))
                .flex()
                .items_center()
                .justify_center()
                .rounded(px(4.))
                .cursor_pointer()
                .text_color(color)
                .hover(|s| s.bg(Theme::hover_row()))
                .active(|s| s.bg(Theme::press()))
                .tooltip(move |_, cx| cx.new(|_| super::Tip(tip.into())).into())
                .child(crate::icons::glyph(glyph, 14., color))
        };
        let group = |title: &str| div().px(px(8.)).pt(px(6.)).pb(px(2.)).text_size(ui_px(11.5)).text_color(Theme::text_dim()).child(title.to_string());
        let mut tree = div()
            .id("services-tree")
            .track_focus(&self.services.focus)
            .w(px(300.))
            .flex_shrink_0()
            .h_full()
            .overflow_y_scroll()
            .border_r_1()
            .border_color(Theme::divider())
            .text_size(ui_px(12.5))
            .child(group("Run"));
        if runs.is_empty() {
            tree = tree.child(div().px(px(14.)).py(px(4.)).text_color(Theme::text_muted()).child("Nothing has run yet"));
        }
        for r in &runs {
            let id = r.id;
            let on = sel == Some(Item::Run(id));
            let (glyph, color, detail) = status(r);
            tree = tree.child(
                div()
                    .id(("svc-run", id))
                    .w_full()
                    .h(ui_px(24.))
                    .pl(px(14.))
                    .pr(px(4.))
                    .flex()
                    .items_center()
                    .gap(px(6.))
                    .cursor_pointer()
                    .when(on, |d| d.bg(Theme::selection_row()))
                    .when(!on, |d| d.hover(|s| s.bg(Theme::hover_row())))
                    .on_mouse_down(gpui::MouseButton::Left, cx.listener(move |this, _, w, cx| {
                        this.services_select(Item::Run(id), cx);
                        w.focus(&this.services.focus.clone());
                    }))
                    .child(div().w(px(14.)).flex().items_center().child(crate::icons::glyph(glyph, 12., color)))
                    .child(div().flex_shrink_0().whitespace_nowrap().child(r.name.clone()))
                    .child(div().flex_1().min_w_0().overflow_hidden().whitespace_nowrap().text_ellipsis().text_size(ui_px(11.5)).text_color(Theme::text_dim()).child(detail))
                    .child(small(("svc-rerun", id), "↻", Theme::file_added(), "Rerun").child(track(id, "rerun")).on_click(cx.listener(move |this, _, w, cx| {
                        cx.stop_propagation();
                        this.services_rerun(id, w, cx);
                    })))
                    .child(
                        small(("svc-stop", id), "■", if r.running { Theme::error() } else { Theme::text_dim() }, "Stop").child(track(id, "stop")).on_click(cx.listener(move |this, _, _, cx| {
                            cx.stop_propagation();
                            this.services_stop(id, cx);
                        })),
                    ),
            );
        }
        if let Some(title) = self.db.results.as_ref().map(|g| g.read(cx).title.clone()) {
            let on = sel == Some(Item::Database);
            tree = tree.child(group("Database")).child(
                div()
                    .id("svc-db")
                    .w_full()
                    .h(ui_px(24.))
                    .pl(px(14.))
                    .flex()
                    .items_center()
                    .gap(px(6.))
                    .cursor_pointer()
                    .when(on, |d| d.bg(Theme::selection_row()))
                    .when(!on, |d| d.hover(|s| s.bg(Theme::hover_row())))
                    .on_mouse_down(gpui::MouseButton::Left, cx.listener(|this, _, w, cx| {
                        this.services_select(Item::Database, cx);
                        w.focus(&this.services.focus.clone());
                    }))
                    .child(crate::icons::ui_icon("database", 12., Theme::accent()))
                    .child(format!("{title} · console")),
            );
        }
        let right: gpui::AnyElement = match (sel, &self.runs.panel) {
            (Some(Item::Run(_)), Some(p)) => div().flex_1().min_w_0().h_full().child(p.clone()).into_any_element(),
            (Some(Item::Database), _) => self.render_db_results(cx),
            _ => crate::ui_kit::empty_state("play", "Select a run to see its console. Runs appear here as you start them (Shift+F10, Ctrl+Shift+F10).").into_any_element(),
        };
        // The shared tool window header: the title, then how many runs are live.
        let running = runs.iter().filter(|r| r.running).count();
        let header = div()
            .h(px(crate::ui_kit::header_h()))
            .flex_shrink_0()
            .flex()
            .items_center()
            .gap(px(8.))
            .px(px(12.))
            .text_size(ui_px(13.))
            .when(!crate::theme::islands(), |d| d.border_b_1().border_color(Theme::divider()))
            .child(div().font_weight(gpui::FontWeight::SEMIBOLD).text_color(Theme::text()).child("Services"))
            .when(running > 0, |d| d.child(div().text_size(ui_px(12.)).text_color(Theme::text_muted()).child(format!("{running} running"))));
        div()
            .id("services-window")
            .size_full()
            .flex()
            .flex_col()
            .child(header)
            .child(div().flex_1().min_h_0().flex().child(tree).child(div().flex_1().min_w_0().h_full().flex().child(right)))
            .into_any_element()
    }

    // --- Smoke --------------------------------------------------------------------------

    /// BRISK_SMOKE_SERVICES=1|hold on an empty folder: starts `long.py` (runs 60 s), `quick.py`
    /// (exit 0) and `fail.py` (exit 2), opens Services and checks the statuses, then really
    /// clicks ■ on `long` and ↻ on `fail`.
    pub(super) fn services_smoke(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let root = self.root.clone();
        let w = |n: &str, s: &str| {
            let _ = std::fs::write(root.join(n), s);
        };
        w("long.py", "import time\nprint('long started', flush=True)\ntime.sleep(60)\n");
        w("quick.py", "print('quick done')\n");
        w("fail.py", "import sys\nopen('fail.log', 'a').write('run\\n')\nprint('failing')\nsys.exit(2)\n");
        BUTTONS.with(|c| c.borrow_mut().clear());
        cx.spawn_in(window, async move |this, cx| {
            let bg = cx.background_executor().clone();
            let pause = |ms| bg.timer(std::time::Duration::from_millis(ms));
            for _ in 0..100 {
                pause(100).await;
                if this.update(cx, |t, _| t.runs.interpreter(&t.root).is_some()).unwrap_or(false) {
                    break;
                }
            }
            for n in ["long", "quick", "fail"] {
                let _ = this.update_in(cx, |this, window, cx| {
                    this.run_config(brisk_run::RunConfig { name: n.into(), target: format!("{n}.py"), ..Default::default() }, window, cx);
                });
            }
            pause(2500).await;
            let rows = |this: &Workspace, cx: &gpui::App| this.runs_list(cx).iter().map(|r| format!("{} {}", r.name, status(r).2)).collect::<Vec<_>>();
            let _ = this.update_in(cx, |this, window, cx| {
                this.dispatch(brisk_keymap::Action::ToggleServicesWindow, window, cx);
                eprintln!("smoke-services: open {} selected {:?} rows {:?}", this.bottom == Some(BottomTool::Services), this.services.selected, rows(this, cx));
                // ↑ from the active (last) run selects the one above.
                this.smoke_press("up", window, cx);
                let name = this.runs_list(cx).into_iter().find(|r| Some(Item::Run(r.id)) == this.services.selected).map(|r| r.name);
                eprintln!("smoke-services: up -> {name:?}, run window shows it {}", this.runs_list(cx).iter().any(|r| r.active && Some(r.name.clone()) == name));
            });
            if std::env::var("BRISK_SMOKE_SERVICES").as_deref() == Ok("hold") {
                return;
            }
            let id_of = |this: &Workspace, cx: &gpui::App, n: &str| this.runs_list(cx).into_iter().find(|r| r.name == n).map(|r| r.id);
            for (name, what) in [("long", "stop"), ("fail", "rerun")] {
                pause(300).await;
                let _ = this.update_in(cx, |this, window, cx| {
                    let scale = window.scale_factor();
                    match id_of(this, cx, name).and_then(|id| button_center(id, what)) {
                        Some((x, y)) => super::win_input::click(x * scale, y * scale),
                        None => eprintln!("smoke-services: no {what} button for {name}"),
                    }
                });
                pause(1500).await;
                let _ = this.update(cx, |this, cx| {
                    if what == "stop" {
                        if let Some(id) = id_of(this, cx, name) {
                            this.services_select(Item::Run(id), cx);
                        }
                        let text = this.runs.panel.as_ref().and_then(|p| p.read(cx).active_text()).unwrap_or_default();
                        eprintln!("smoke-services: {name} console ends {:?}", text.lines().last().unwrap_or(""));
                    }
                    let runs =std::fs::read_to_string(this.root.join("fail.log")).map(|s| s.lines().count()).unwrap_or(0);
                    eprintln!("smoke-services: click {what} on {name} -> rows {:?}, fail ran {runs}x, services open {}", rows(this, cx), this.bottom == Some(BottomTool::Services));
                });
            }
            let _ = this.update(cx, |_, cx| cx.quit());
        })
        .detach();
    }
}
