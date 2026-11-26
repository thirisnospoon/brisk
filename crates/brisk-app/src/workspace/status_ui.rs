//! The Islands status bar, PyCharm new UI style: the background tasks indicator and the last
//! message on the left; widgets on the right (line:col, line separator, encoding, indent,
//! language, interpreter, branch), each a pill on hover, the clickable ones opening their
//! popup. Classic keeps its own status bar (`Workspace::render_status`).

use super::{PopupItem, PopupKind, Target, Workspace};
use crate::theme::ui_px;
use crate::theme::Theme;
use gpui::prelude::*;
use gpui::{div, px, Animation, AnimationExt, Context, Transformation};
use std::time::{Duration, Instant};

/// How long a new status message stays in the status bar.
const SHOWN: Duration = Duration::from_secs(6);

/// The status bar's messages: the one shown now and the recent ones.
#[derive(Default)]
pub struct StatusLog {
    last: String,
    at: Option<Instant>,
    n: u64,
    recent: std::collections::VecDeque<(String, Instant)>,
}

fn ago(t: Instant) -> String {
    let s = t.elapsed().as_secs();
    match s {
        0..=59 => "just now".into(),
        60..=3599 => format!("{} min ago", s / 60),
        _ => format!("{} h ago", s / 3600),
    }
}

impl Workspace {
    /// A new `status` starts its 6 s and goes to the history; a repaint is scheduled for when
    /// it should fade.
    pub(super) fn track_status(&mut self, cx: &mut Context<Self>) {
        if self.status == self.status_log.last {
            return;
        }
        let now = Instant::now();
        self.status_log.last = self.status.clone();
        self.status_log.at = Some(now);
        self.status_log.n += 1;
        if !self.status.trim().is_empty() {
            let log = &mut self.status_log.recent;
            log.retain(|(m, _)| m != &self.status);
            log.push_front((self.status.clone(), now));
            log.truncate(20);
        }
        cx.spawn(async move |this, cx| {
            cx.background_executor().timer(SHOWN).await;
            let _ = this.update(cx, |_, cx| cx.notify());
        })
        .detach();
    }

    /// The message area: a problem at the caret, else the last message while it is new; then
    /// it fades out over 400 ms. A click shows the recent messages.
    pub(super) fn status_message(&self, cx: &mut Context<Self>) -> gpui::AnyElement {
        let problem = self.active_editor().and_then(|e| e.read(cx).caret_problem());
        let fresh = self.status_log.at.is_some_and(|t| t.elapsed() < SHOWN);
        let old = problem.is_none() && !fresh;
        let text = problem.unwrap_or_else(|| self.status.clone());
        let mut d = div().id("sb-message").min_w_0().overflow_hidden().text_ellipsis().whitespace_nowrap().rounded(px(6.)).child(text);
        if !self.status_log.recent.is_empty() {
            d = d.cursor_pointer().hover(|s| s.bg(Theme::hover_row())).on_click(cx.listener(|this, _, _, cx| {
                this.open_popup(PopupKind::StatusHistory);
                cx.notify();
            }));
        }
        if !old {
            return d.into_any_element();
        }
        if !brisk_settings::animations() {
            return d.opacity(0.).into_any_element();
        }
        // Keyed by the message's number, so each message fades once.
        let id = gpui::ElementId::Name(format!("sb-fade-{}", self.status_log.n).into());
        d.with_animation(id, Animation::new(Duration::from_millis(400)).with_easing(gpui::ease_out_quint()), |d, t| d.opacity(1. - t)).into_any_element()
    }

    pub(super) fn status_history_items(&self, q: &str) -> Vec<PopupItem> {
        let ql = q.to_lowercase();
        self.status_log
            .recent
            .iter()
            .filter(|(m, _)| m.to_lowercase().contains(&ql))
            .map(|(m, t)| PopupItem { label: m.clone(), detail: String::new(), hint: ago(*t), target: Target::Task })
            .collect()
    }

    /// What runs in the background now, for the indicator: one line each, most important
    /// first.
    pub(super) fn bg_tasks(&self) -> Vec<String> {
        let mut v: Vec<String> = Vec::new();
        if self.files.is_none() {
            v.push("Indexing files…".into());
        }
        if self.symbols_indexing() {
            v.push("Indexing symbols…".into());
        }
        if let Some(g) = &self.git {
            if let Some(b) = g.busy {
                v.push(format!("{b}…"));
            }
            if g.refreshing {
                v.push("git status…".into());
            }
        }
        if let Some(p) = &self.packages {
            if let Some(b) = &p.busy {
                v.push(b.clone());
            } else if p.loading {
                v.push("Loading packages…".into());
            }
        }
        if self.venv_busy {
            v.push("Creating virtual environment…".into());
        }
        if self.venv_offer.as_ref().is_some_and(|o| o.installing) {
            v.push("Installing requirements…".into());
        }
        if self.runs.all.is_none() {
            v.push("Detecting interpreters…".into());
        }
        v
    }

    /// The background tasks popup: one row per task.
    pub(super) fn bg_task_items(&self, q: &str) -> Vec<PopupItem> {
        let ql = q.to_lowercase();
        let tasks = self.bg_tasks();
        if tasks.is_empty() {
            return vec![PopupItem { label: "No background tasks".into(), detail: String::new(), hint: String::new(), target: Target::Task }];
        }
        tasks
            .into_iter()
            .filter(|t| t.to_lowercase().contains(&ql))
            .map(|t| PopupItem { label: t, detail: String::new(), hint: String::new(), target: Target::Task })
            .collect()
    }

    pub(super) fn render_status_islands(&self, cx: &mut Context<Self>) -> gpui::AnyElement {
        let ed = self.active_editor().map(|e| e.read(cx));
        // The routine `git status` after every save and window switch is not shown in the bar:
        // its spinner would redraw the whole window at the display's rate while it runs. It
        // is still listed in the background tasks popup.
        let tasks: Vec<String> = self.bg_tasks().into_iter().filter(|t| t != "git status…").collect();
        // A widget: muted text, a pill on hover; `click` makes it a button.
        let widget = |id: &'static str| {
            div()
                .id(id)
                .h(ui_px(22.))
                .px(px(7.))
                .flex()
                .items_center()
                .gap(px(5.))
                .rounded(px(6.))
                .text_color(Theme::text_muted())
                .hover(|s| s.bg(Theme::hover_row()).text_color(Theme::text()))
        };
        let mut right = div().flex().items_center().gap(px(2.));
        if let Some(e) = ed {
            let (l, c) = e.caret_line_col();
            let eol = match e.buffer.line_ending {
                brisk_text::LineEnding::Lf => "LF",
                brisk_text::LineEnding::CrLf => "CRLF",
            };
            right = right
                .child(widget("sb-pos").child(format!("{}:{}", l + 1, c + 1)).cursor_pointer().on_click(cx.listener(|this, _, _, cx| {
                    this.open_popup(PopupKind::GotoLine);
                    cx.notify();
                })))
                .child(widget("sb-eol").child(eol))
                .child(widget("sb-enc").child(e.encoding_label()))
                .child(widget("sb-indent").child(e.indent_label()))
                .when(e.language_name() != "Python", |r| r.child(widget("sb-lang").child(e.language_name())));
        }
        right = right.child(widget("sb-interpreter").child(self.interpreter_label()).cursor_pointer().on_click(cx.listener(|this, _, _, cx| {
            this.open_popup(PopupKind::Interpreters);
            cx.notify();
        })));
        // The live branch (after a checkout), not the one read at startup.
        if self.branch.is_some() {
            let b = self.branch_label();
            right = right.child(
                widget("sb-branch")
                    .cursor_pointer()
                    .child(crate::icons::ui_icon("git-branch", 13., Theme::text_muted()))
                    .child(super::fit_text(cx, &b, 12., 220.))
                    .on_click(cx.listener(|this, _, window, cx| {
                        this.dispatch(brisk_keymap::Action::Branches, window, cx);
                        cx.notify();
                    })),
            );
        }
        // Left: the tasks indicator (a spinner and the first task), then the last message.
        let indicator = (!tasks.is_empty()).then(|| {
            let more = tasks.len() - 1;
            let spinner = crate::icons::ui_icon("loader-circle", 12., Theme::accent()).with_animation(
                "sb-spin",
                Animation::new(Duration::from_millis(900)).repeat(),
                |s, d| s.with_transformation(Transformation::rotate(gpui::percentage(d))),
            );
            widget("sb-tasks")
                .cursor_pointer()
                .child(spinner)
                .child(tasks[0].clone())
                .when(more > 0, |d| d.child(div().text_color(Theme::text_dim()).child(format!("+{more}"))))
                .on_click(cx.listener(|this, _, _, cx| {
                    this.open_popup(PopupKind::BackgroundTasks);
                    cx.notify();
                }))
        });
        div()
            .h(ui_px(28.))
            .flex_shrink_0()
            .flex()
            .items_center()
            .px(px(8.))
            .gap(px(6.))
            .bg(Theme::frame())
            .text_size(ui_px(12.))
            .children(indicator)
            .child(div().flex_1().min_w_0().flex().px(px(4.)).text_color(Theme::text_dim()).child(self.status_message(cx)))
            .child(right)
            .into_any_element()
    }
}
