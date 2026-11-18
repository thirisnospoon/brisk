//! Result grid: query output in the Services window and "Edit Data" tabs. Read-only for now.
//! Only visible rows are built (uniform_list), so a 500-row page costs the same as 20.

use crate::theme::{Theme, mono_font};
use brisk_db::QueryOutput;
use gpui::prelude::*;
use gpui::{
    div, px, uniform_list, App, ClipboardItem, Context, EventEmitter, FocusHandle, Focusable, Keystroke, ScrollHandle,
    ScrollStrategy, UniformListScrollHandle, Window,
};
use std::sync::Arc;
use std::time::Instant;

const ROW_H: f32 = 22.;
const NUM_W: f32 = 52.;
const CHAR_W: f32 = 7.0;

pub enum GridEvent {
    Reload,
    Cancel,
}

pub enum GridState {
    Empty,
    Running { sql: String, since: Instant },
    Done { out: Arc<QueryOutput>, message: String },
    Failed(String),
}

pub struct GridView {
    pub title: String,
    /// Data source id and query, for reload (Ctrl+F5) and the tab title.
    pub source: Option<(String, String)>,
    pub state: GridState,
    focus: FocusHandle,
    sel: (usize, usize),
    widths: Vec<f32>,
    scroll: UniformListScrollHandle,
    hscroll: ScrollHandle,
}

impl EventEmitter<GridEvent> for GridView {}

fn one_line(s: &str, max_chars: usize) -> String {
    let mut out = String::new();
    for (n, c) in s.chars().enumerate() {
        if n >= max_chars {
            out.push('…');
            break;
        }
        out.push(match c {
            '\n' => '⏎',
            '\r' | '\t' => ' ',
            c => c,
        });
    }
    out
}

impl GridView {
    pub fn new(title: impl Into<String>, cx: &mut Context<Self>) -> Self {
        Self {
            title: title.into(),
            source: None,
            state: GridState::Empty,
            focus: cx.focus_handle(),
            sel: (0, 0),
            widths: Vec::new(),
            scroll: UniformListScrollHandle::new(),
            hscroll: ScrollHandle::new(),
        }
    }

    pub fn is_running(&self) -> bool {
        matches!(self.state, GridState::Running { .. })
    }

    pub fn set_running(&mut self, sql: &str, cx: &mut Context<Self>) {
        self.state = GridState::Running { sql: sql.trim().to_string(), since: Instant::now() };
        cx.notify();
        // Tick the elapsed time while waiting.
        cx.spawn(async move |this, cx| loop {
            cx.background_executor().timer(std::time::Duration::from_millis(250)).await;
            let running = this.update(cx, |g, cx| {
                cx.notify();
                g.is_running()
            });
            if !matches!(running, Ok(true)) {
                break;
            }
        })
        .detach();
    }

    /// Show a statement's output; for row-less statements show the message only.
    pub fn set_output(&mut self, out: QueryOutput, message: String, cx: &mut Context<Self>) {
        let ncol = out.columns.len();
        self.widths = (0..ncol)
            .map(|c| {
                let mut w = out.columns[c].chars().count();
                for r in out.rows.iter().take(200) {
                    let n = r[c].as_ref().map_or(6, |v| v.chars().count().min(60));
                    w = w.max(n);
                }
                (w as f32 * CHAR_W + 18.).clamp(56., 420.)
            })
            .collect();
        self.sel = (0, 0);
        self.scroll.scroll_to_item(0, ScrollStrategy::Top);
        self.hscroll.set_offset(gpui::point(px(0.), px(0.)));
        self.state = GridState::Done { out: Arc::new(out), message };
        cx.notify();
    }

    pub fn set_error(&mut self, e: String, cx: &mut Context<Self>) {
        self.state = GridState::Failed(e);
        cx.notify();
    }

    fn output(&self) -> Option<&Arc<QueryOutput>> {
        match &self.state {
            GridState::Done { out, .. } if out.has_rows() => Some(out),
            _ => None,
        }
    }

    pub fn copy(&self, cx: &mut App) -> bool {
        let Some(out) = self.output() else { return false };
        let Some(row) = out.rows.get(self.sel.0) else { return false };
        let v = row.get(self.sel.1).cloned().flatten().unwrap_or_else(|| "NULL".into());
        cx.write_to_clipboard(ClipboardItem::new_string(v));
        true
    }

    /// Grid navigation. Returns false for keys the grid doesn't use.
    pub fn key(&mut self, k: &Keystroke, cx: &mut Context<Self>) -> bool {
        let Some(out) = self.output() else { return false };
        let (nr, nc) = (out.rows.len(), out.columns.len());
        if nr == 0 || nc == 0 {
            return false;
        }
        let ctrl = k.modifiers.control;
        let (mut r, mut c) = self.sel;
        let page = 15;
        match k.key.as_str() {
            "up" => r = r.saturating_sub(1),
            "down" => r = (r + 1).min(nr - 1),
            "left" => c = c.saturating_sub(1),
            "right" | "tab" => c = (c + 1).min(nc - 1),
            "pageup" => r = r.saturating_sub(page),
            "pagedown" => r = (r + page).min(nr - 1),
            "home" if ctrl => (r, c) = (0, 0),
            "end" if ctrl => (r, c) = (nr - 1, nc - 1),
            "home" => c = 0,
            "end" => c = nc - 1,
            _ => return false,
        }
        self.select(r, c, cx);
        true
    }

    fn select(&mut self, r: usize, c: usize, cx: &mut Context<Self>) {
        self.sel = (r, c);
        self.scroll.scroll_to_item(r, ScrollStrategy::Top);
        // Horizontal: bring the column into view.
        let x0: f32 = NUM_W + self.widths[..c.min(self.widths.len())].iter().sum::<f32>();
        let x1 = x0 + self.widths.get(c).copied().unwrap_or(0.);
        let view_w = f32::from(self.hscroll.bounds().size.width);
        let off = -f32::from(self.hscroll.offset().x);
        if view_w > 0. {
            let new = if x0 < off + NUM_W { (x0 - NUM_W).max(0.) } else if x1 > off + view_w { x1 - view_w } else { off };
            if new != off {
                self.hscroll.set_offset(gpui::point(px(-new), px(0.)));
            }
        }
        cx.notify();
    }

    fn render_header(&self, cx: &mut Context<Self>) -> impl IntoElement {
        let (text, color) = match &self.state {
            GridState::Empty => ("Run a statement with Ctrl+Enter in a query console".to_string(), Theme::text_muted()),
            GridState::Running { sql, since } => (format!("Running {:.1} s… {}", since.elapsed().as_secs_f32(), one_line(sql, 80)), Theme::text_muted()),
            GridState::Done { message, .. } => (message.clone(), Theme::text_muted()),
            GridState::Failed(e) => (one_line(e, 400), Theme::error()),
        };
        let running = self.is_running();
        let reload = self.source.is_some() && !running;
        let btn = |id: &'static str, glyph: &'static str| {
            div().id(id).px(px(6.)).rounded(px(4.)).cursor_pointer().hover(|s| s.bg(Theme::hover_row())).child(crate::icons::glyph(glyph, 14., Theme::text_muted()))
        };
        div()
            .h(px(28.))
            .flex_shrink_0()
            .flex()
            .items_center()
            .gap(px(6.))
            .px(px(8.))
            .border_b_1()
            .border_color(Theme::divider())
            .text_size(px(12.))
            .when(reload, |d| d.child(btn("grid-reload", "⟳").on_click(cx.listener(|_, _, _, cx| cx.emit(GridEvent::Reload)))))
            .when(running, |d| {
                d.child(btn("grid-cancel", "■").text_color(Theme::error()).on_click(cx.listener(|_, _, _, cx| cx.emit(GridEvent::Cancel))))
            })
            .child(div().flex_1().min_w_0().overflow_hidden().whitespace_nowrap().text_color(color).child(text))
    }

    fn render_table(&self, out: Arc<QueryOutput>, cx: &mut Context<Self>) -> impl IntoElement {
        let total_w: f32 = NUM_W + self.widths.iter().sum::<f32>();
        let widths = self.widths.clone();
        let header = div()
            .h(px(ROW_H + 2.))
            .flex()
            .flex_shrink_0()
            .bg(Theme::panel_alt())
            .border_b_1()
            .border_color(Theme::divider())
            .child(div().w(px(NUM_W)).flex_shrink_0())
            .children(out.columns.iter().zip(&widths).map(|(name, w)| {
                div()
                    .w(px(*w))
                    .flex_shrink_0()
                    .px(px(6.))
                    .flex()
                    .items_center()
                    .overflow_hidden()
                    .whitespace_nowrap()
                    .border_r_1()
                    .border_color(Theme::divider())
                    .font_weight(gpui::FontWeight::SEMIBOLD)
                    .child(one_line(name, ((w - 12.) / CHAR_W) as usize))
            }));
        let n = out.rows.len();
        let focused_sel = self.sel;
        let list = uniform_list(
            "grid-rows",
            n,
            cx.processor(move |this, range: std::ops::Range<usize>, window, cx| {
                let focused = this.focus.is_focused(window);
                let Some(out) = this.output().cloned() else { return Vec::new() };
                let widths = this.widths.clone();
                range
                    .map(|r| {
                        let row = &out.rows[r];
                        div()
                            .id(("grid-row", r))
                            .h(px(ROW_H))
                            .flex()
                            .when(r % 2 == 1, |d| d.bg(Theme::panel_alt()))
                            .child(
                                div().w(px(NUM_W)).flex_shrink_0().px(px(6.)).flex().items_center().justify_end().text_color(Theme::text_dim()).child((r + 1).to_string()),
                            )
                            .children(row.iter().zip(&widths).enumerate().map(|(c, (v, w))| {
                                let selected = focused_sel_eq(this.sel, r, c);
                                let max = ((w - 12.) / CHAR_W).max(1.) as usize;
                                let cell = div()
                                    .id(("cell", r * 4096 + c))
                                    .w(px(*w))
                                    .flex_shrink_0()
                                    .px(px(6.))
                                    .flex()
                                    .items_center()
                                    .overflow_hidden()
                                    .whitespace_nowrap()
                                    .border_r_1()
                                    .border_color(Theme::indent_guide())
                                    .when(selected, |d| d.bg(if focused { Theme::selection() } else { Theme::hover_row() }))
                                    .on_click(cx.listener(move |this, _, window, cx| {
                                        window.focus(&this.focus);
                                        this.select(r, c, cx);
                                    }));
                                match v {
                                    Some(s) => cell.child(one_line(s, max)),
                                    None => cell.text_color(Theme::text_dim()).italic().child("<null>"),
                                }
                            }))
                    })
                    .collect()
            }),
        )
        .track_scroll(self.scroll.clone())
        .flex_1();
        let _ = focused_sel;
        div()
            .id("grid-hscroll")
            .flex_1()
            .min_h_0()
            .overflow_x_scroll()
            .track_scroll(&self.hscroll)
            .child(div().w(px(total_w)).h_full().flex().flex_col().child(header).child(list))
    }
}

fn focused_sel_eq(sel: (usize, usize), r: usize, c: usize) -> bool {
    sel == (r, c)
}

impl Focusable for GridView {
    fn focus_handle(&self, _: &App) -> FocusHandle {
        self.focus.clone()
    }
}

impl Render for GridView {
    fn render(&mut self, _window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let table = self.output().cloned();
        let statement = match &self.state {
            GridState::Done { out, .. } if !out.has_rows() => Some(out.statement.clone()),
            _ => None,
        };
        div()
            .id("grid")
            .key_context("DataGrid")
            .track_focus(&self.focus)
            .size_full()
            .flex()
            .flex_col()
            .bg(Theme::editor_bg())
            .font_family(mono_font())
            .text_size(px(12.5))
            .text_color(Theme::text())
            .child(self.render_header(cx))
            .when_some(table, |d, out| d.child(self.render_table(out, cx)))
            .when_some(statement, |d, s| d.child(div().p(px(10.)).text_color(Theme::text_muted()).child(one_line(&s, 300))))
    }
}
