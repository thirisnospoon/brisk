//! Find in the Run console, like PyCharm's (Ctrl+F): a search bar over the output, every
//! match highlighted, Enter / Shift+Enter (F3 / Shift+F3) go through them, and Filter shows
//! only the matching lines (a click on one jumps to it).

use super::TerminalPanel;
use crate::theme::{mono_font, Theme};
use brisk_terminal::Found;
use gpui::prelude::*;
use gpui::{div, px, Context, Keystroke, MouseButton};

#[derive(Default)]
pub struct ConsoleSearch {
    pub open: bool,
    /// Typing goes to the query (a click in the output gives it back to the process).
    pub focused: bool,
    pub query: String,
    pub case: bool,
    /// Show only the matching lines.
    pub filter: bool,
    pub found: Vec<Found>,
    pub current: usize,
    /// The query, the case or the tab changed since `found` was computed.
    pub dirty: bool,
    /// New output since `found` was computed: searched again at most every few hundred ms
    /// (a search over 20 000 lines on every frame of a flood would freeze the window).
    pub output_changed: bool,
    /// A redraw is scheduled for when the throttle allows the next search.
    pub tick_pending: bool,
    last_search: Option<std::time::Instant>,
    last_cost: std::time::Duration,
    /// The tab and grid size `found` belongs to (Soft-wrap and resizing re-wrap the rows).
    tab: u64,
    size: (usize, usize),
}

impl TerminalPanel {
    pub fn search_open(&self) -> bool {
        self.search.open
    }

    /// Ctrl+F: open the bar (a one-line selection becomes the query).
    pub fn open_search(&mut self) {
        let sel = self.session().and_then(|s| s.selection_text()).filter(|t| !t.contains('\n') && !t.trim().is_empty());
        let s = &mut self.search;
        s.open = true;
        s.focused = true;
        if let Some(t) = sel {
            s.query = t;
        }
        self.search_changed();
    }

    pub fn close_search(&mut self) {
        self.search.open = false;
        self.search.filter = false;
        self.search.found.clear();
    }

    /// Recompute matches when the output or the tab changed (keeps the current match).
    pub(super) fn refresh_search(&mut self) {
        let tab = self.tabs.get(self.active).map(|t| t.id).unwrap_or(0);
        let size = self.session().map(|s| s.size()).unwrap_or_default();
        let due = self.search.output_changed && self.search.last_search.is_none_or(|t| t.elapsed() >= Self::search_interval(self.search.last_cost));
        if !self.search.open || (!self.search.dirty && !due && self.search.tab == tab && self.search.size == size) {
            return;
        }
        let started = std::time::Instant::now();
        self.search.dirty = false;
        self.search.output_changed = false;
        self.search.tab = tab;
        self.search.size = size;
        self.search.found = match self.session() {
            Some(s) if !self.search.query.is_empty() => s.find(&self.search.query, self.search.case),
            _ => Vec::new(),
        };
        self.search.current = self.search.current.min(self.search.found.len().saturating_sub(1));
        self.search.last_search = Some(std::time::Instant::now());
        self.search.last_cost = started.elapsed();
    }

    /// How often new output is searched: 300 ms, or ten times as long as a search takes.
    pub(super) fn search_interval(cost: std::time::Duration) -> std::time::Duration {
        std::time::Duration::from_millis(300).max(cost * 10)
    }

    /// New output with the bar open: search it once the throttle allows (a redraw then).
    pub(super) fn search_output_changed(&mut self, cx: &mut Context<Self>) {
        if !self.search.open {
            return;
        }
        self.search.output_changed = true;
        if self.search.tick_pending {
            return;
        }
        self.search.tick_pending = true;
        let wait = Self::search_interval(self.search.last_cost);
        cx.spawn(async move |this, cx| {
            cx.background_executor().timer(wait).await;
            let _ = this.update(cx, |this, cx| {
                this.search.tick_pending = false;
                cx.notify();
            });
        })
        .detach();
    }

    /// The query or Match Case changed: the first match from the top of the view.
    fn search_changed(&mut self) {
        self.search.dirty = true;
        self.refresh_search();
        let top = -(self.session().map(|s| s.snapshot().display_offset).unwrap_or(0) as i32);
        let i = self.search.found.iter().position(|f| f.line >= top).unwrap_or(0);
        self.jump(i);
    }

    fn jump(&mut self, i: usize) {
        let Some(f) = self.search.found.get(i).cloned() else { return };
        self.search.current = i;
        if let Some(s) = self.session() {
            s.scroll_to(f.line);
        }
        // Looking at a match stops following the output.
        self.scroll_to_end = false;
        // Soft-wrap off: bring the match's columns into view.
        if let Some(g) = self.grid {
            let fit = (f32::from(g.bounds.size.width) / f32::from(g.cell_w)).floor().max(2.0) as usize;
            if f.cols.start < self.h_scroll || f.cols.end > self.h_scroll + fit {
                self.h_scroll = f.cols.start.saturating_sub(8);
            }
        }
    }

    /// Next (`back` = previous) match, wrapping around.
    pub fn search_step(&mut self, back: bool) {
        self.refresh_search();
        let n = self.search.found.len();
        if n == 0 {
            return;
        }
        let i = if back { (self.search.current + n - 1) % n } else { (self.search.current + 1) % n };
        self.jump(i);
    }

    /// "3/12", "No matches" or "".
    pub fn search_summary(&self) -> String {
        match (self.search.query.is_empty(), self.search.found.len()) {
            (true, _) => String::new(),
            (false, 0) => "No matches".into(),
            (false, n) => format!("{}/{n}", self.search.current + 1),
        }
    }

    /// Where the current match is and whether it is on screen (smoke tests).
    #[cfg(feature = "smoke")]
    pub fn search_probe(&self) -> String {
        let Some(f) = self.search.found.get(self.search.current) else { return "no match".into() };
        let snap = self.session().map(|s| s.snapshot());
        let off = snap.as_ref().map_or(0, |s| s.display_offset as i32);
        let rows = snap.as_ref().map_or(0, |s| s.rows as i32);
        let visible = f.line + off >= 0 && f.line + off < rows;
        format!(
            "line {} cols {:?} more {:?} visible {visible} h_scroll {} follow {} text {:?}",
            f.line,
            f.cols,
            f.more,
            self.h_scroll,
            self.scroll_to_end,
            f.text.chars().take(30).collect::<String>()
        )
    }

    /// Type into the query (smoke tests use it as a user would).
    pub fn search_type(&mut self, text: &str) {
        self.search.query.push_str(text);
        self.search_changed();
    }

    pub fn toggle_search_filter(&mut self) {
        self.search.filter = !self.search.filter;
        self.refresh_search();
    }

    /// The filtered view: one entry per matching (logical) line, by its first row.
    pub fn filtered_lines(&self) -> Vec<(i32, String)> {
        let mut out: Vec<(i32, String)> = Vec::new();
        for f in &self.search.found {
            if out.last().is_none_or(|(l, _)| *l != f.row) {
                out.push((f.row, f.text.clone()));
            }
        }
        out
    }

    /// A row of the filtered view was clicked: back to the whole output, at that row.
    pub fn open_filtered(&mut self, line: i32) {
        self.search.filter = false;
        if let Some(i) = self.search.found.iter().position(|f| f.row == line) {
            self.jump(i);
        }
    }

    /// Keys while the bar is open. `true` when handled.
    pub(super) fn search_key(&mut self, k: &Keystroke, cx: &mut Context<Self>) -> bool {
        let m = k.modifiers;
        let key = k.key.as_str();
        let plain = !m.control && !m.alt && !m.platform;
        match key {
            "escape" if plain && !m.shift => self.close_search(),
            "f3" => self.search_step(m.shift),
            "enter" if self.search.focused => self.search_step(m.shift),
            _ if !self.search.focused => return false,
            "backspace" => {
                if m.control {
                    self.search.query.clear();
                } else {
                    self.search.query.pop();
                }
                self.search_changed();
            }
            "v" if m.control => {
                if let Some(t) = cx.read_from_clipboard().and_then(|c| c.text()) {
                    self.search_type(t.lines().next().unwrap_or(""));
                }
            }
            "c" if m.alt => {
                self.search.case = !self.search.case;
                self.search_changed();
            }
            _ if plain => match k.key_char.as_deref().filter(|c| !c.chars().any(char::is_control)) {
                Some(c) => self.search_type(c),
                None => return false,
            },
            // Other shortcuts (Ctrl+F2, Ctrl+F5…) keep working.
            _ => return false,
        }
        cx.notify();
        true
    }

    pub(super) fn render_search_bar(&self, cx: &mut Context<Self>) -> impl IntoElement {
        let s = &self.search;
        let none = !s.query.is_empty() && s.found.is_empty();
        let button = |id: &'static str, glyph: &'static str, on: bool| {
            div()
                .id(id)
                .h(px(22.))
                .px(px(6.))
                .flex()
                .items_center()
                .rounded(px(4.))
                .cursor_pointer()
                .text_color(if on { Theme::text() } else { Theme::text_muted() })
                .when(on, |d| d.bg(Theme::selection_row()))
                .when(!on, |d| d.hover(|s| s.bg(Theme::hover_row())))
                .active(|s| s.bg(Theme::press()))
                .child(crate::icons::glyph(glyph, 14., if on { Theme::text() } else { Theme::text_muted() }))
        };
        let caret = div().w(px(1.5)).h(px(15.)).bg(Theme::accent());
        div()
            .id("console-find")
            .flex()
            .flex_shrink_0()
            .items_center()
            .gap(px(4.))
            .h(px(30.))
            .px(px(8.))
            .border_b_1()
            .border_color(Theme::divider())
            .text_size(px(12.5))
            .on_mouse_down(
                MouseButton::Left,
                cx.listener(|this, _, _, cx| {
                    this.search.focused = true;
                    cx.notify();
                }),
            )
            .child(crate::icons::icon("search", 14., Theme::text_dim()))
            .child(
                div()
                    .w(px(260.))
                    .h(px(22.))
                    .px(px(6.))
                    .flex()
                    .items_center()
                    .rounded(px(4.))
                    .border_1()
                    .border_color(if s.focused { Theme::accent() } else { Theme::border() })
                    .when(none, |d| d.bg(Theme::error().opacity(0.15)))
                    .font_family(mono_font())
                    .overflow_hidden()
                    .whitespace_nowrap()
                    .child(s.query.clone())
                    .when(s.focused, |d| d.child(caret))
                    .when(s.query.is_empty(), |d| d.child(div().text_color(Theme::text_dim()).child(" Find in output")))
                    .when(s.focused, |d| d.relative().child(crate::ui_kit::focus_ring(4., 0.))),
            )
            .child(div().min_w(px(70.)).text_color(if none { Theme::error() } else { Theme::text_muted() }).child(self.search_summary()))
            .child(button("cf-prev", "↑", false).on_click(cx.listener(|this, _, _, cx| {
                this.search_step(true);
                cx.notify();
            })))
            .child(button("cf-next", "↓", false).on_click(cx.listener(|this, _, _, cx| {
                this.search_step(false);
                cx.notify();
            })))
            .child(button("cf-case", "Aa", s.case).on_click(cx.listener(|this, _, _, cx| {
                this.search.case = !this.search.case;
                this.search_changed();
                cx.notify();
            })))
            .child(button("cf-filter", "Filter", s.filter).on_click(cx.listener(|this, _, _, cx| {
                this.toggle_search_filter();
                cx.notify();
            })))
            .child(div().flex_1())
            .child(button("cf-close", "✕", false).on_click(cx.listener(|this, _, _, cx| {
                this.close_search();
                cx.notify();
            })))
    }

    /// Filter on: the matching rows only.
    pub(super) fn render_filtered(&self, cx: &mut Context<Self>) -> impl IntoElement {
        let lines = self.filtered_lines();
        let empty = lines.is_empty();
        div()
            .id("console-filtered")
            .flex_1()
            .min_h_0()
            .mx(px(8.))
            .overflow_y_scroll()
            .font_family(mono_font())
            .text_size(px(13.))
            .when(empty, |d| d.child(div().p(px(8.)).text_color(Theme::text_muted()).child("No lines match")))
            .children(lines.into_iter().enumerate().map(|(i, (line, text))| {
                div().id(("cf-line", i)).w_full().px(px(4.)).whitespace_nowrap().cursor_pointer().hover(|s| s.bg(Theme::hover_row())).child(text).on_mouse_down(
                    MouseButton::Left,
                    cx.listener(move |this, _, _, cx| {
                        this.open_filtered(line);
                        cx.notify();
                    }),
                )
            }))
    }
}
