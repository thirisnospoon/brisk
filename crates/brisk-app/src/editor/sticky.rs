//! Sticky lines (PyCharm: Settings › Editor › General › Sticky Lines): the headers of the
//! classes and functions enclosing the top of the view stay pinned above the text, at most
//! three, the innermost lowest. When a block ends its header is pushed up by the next line.
//! A click on one goes to that header (it lands where it was shown); the row under the mouse
//! is highlighted.

use super::{line_height, scope, Editor};
use crate::theme::Theme;
use gpui::{Context, Pixels, Point, TextRun};

pub(super) const MAX: usize = 3;

#[derive(Default)]
pub(super) struct Sticky {
    /// The headers shown: (buffer line, top offset from the editor's top in px).
    pub(super) shown: Vec<(usize, f32)>,
    /// The index in `shown` under the mouse.
    pub(super) hover: Option<usize>,
}

impl Editor {
    /// Before laying out a frame: which headers stick at this scroll position.
    pub(super) fn update_sticky(&mut self) {
        self.sticky.shown.clear();
        if !self.show_gutter || self.is_field || !crate::settings::sticky_lines() || self.scroll_y <= 0.0 {
            return;
        }
        let outline = self.outline_cached();
        if outline.is_empty() {
            return;
        }
        let lh = line_height();
        let rows = self.display_rows();
        let mut picked: Vec<usize> = Vec::new();
        for depth in 0..MAX {
            // The line just under the headers picked so far.
            let row = (((self.scroll_y + picked.len() as f32 * lh) / lh).floor() as usize).min(rows.saturating_sub(1));
            let line = self.line_of_row(row);
            let chain = scope::enclosing(&outline, line);
            if chain.len() <= depth || chain[..depth] != picked[..] {
                break;
            }
            let item = &outline[chain[depth]];
            if self.row_of(item.line) >= row {
                break;
            }
            picked.push(chain[depth]);
        }
        for (k, &i) in picked.iter().enumerate() {
            let item = &outline[i];
            // Pushed up when the block's last line scrolls past this slot.
            let bottom = (self.row_of(item.end_line) + 1) as f32 * lh - self.scroll_y;
            let y = (k as f32 * lh).min(bottom - lh);
            if y > -lh {
                self.sticky.shown.push((item.line, y));
            }
        }
        if self.sticky.hover.is_some_and(|h| h >= self.sticky.shown.len()) {
            self.sticky.hover = None;
        }
    }

    /// Height of the sticky area from the editor's top (0 when none is shown).
    pub(super) fn sticky_height(&self) -> f32 {
        self.sticky.shown.iter().map(|&(_, y)| y + line_height()).fold(0.0, f32::max)
    }

    /// The sticky line under `p`: its index in `shown`.
    fn sticky_at(&self, p: Point<Pixels>) -> Option<usize> {
        let l = self.layout.as_ref()?;
        let b = l.bounds;
        if p.x < b.left() || p.x >= b.right() - gpui::px(super::scroll::BAR) {
            return None;
        }
        let y = f32::from(p.y - b.top());
        // The lowest one wins where a pushed-up header overlaps the one above.
        self.sticky.shown.iter().rposition(|&(_, top)| y >= top.max(0.0) && y < top + line_height())
    }

    /// A press on a sticky line: go to that header, scrolled so it stays where it was shown.
    pub(super) fn sticky_click(&mut self, p: Point<Pixels>, cx: &mut Context<Self>) -> bool {
        let Some(k) = self.sticky_at(p) else { return false };
        let (line, top) = self.sticky.shown[k];
        let text = self.buffer.line(line);
        let ws = text.chars().take_while(|c| c.is_whitespace()).count();
        let pos = self.buffer.line_col_to_pos(line, ws);
        self.buffer.set_caret(pos, false);
        let shown = self.begin_scroll();
        self.scroll_y = (self.row_of(line) as f32 * line_height() - top.max(0.0)).clamp(0.0, self.max_scroll_y());
        self.end_scroll(shown, Self::jump_duration());
        self.sticky.hover = None;
        cx.notify();
        true
    }

    pub(super) fn sticky_hover(&mut self, p: Option<Point<Pixels>>, cx: &mut Context<Self>) {
        let h = p.and_then(|p| self.sticky_at(p));
        if h != self.sticky.hover {
            self.sticky.hover = h;
            cx.notify();
        }
    }

    /// The text of `line` with syntax colors, for drawing it as a sticky line.
    pub(super) fn sticky_runs(&self, line: usize, run: &dyn Fn(usize, gpui::Hsla) -> TextRun) -> (String, Vec<TextRun>) {
        let b = &self.buffer;
        let text = b.line(line).trim_end_matches(['\n', '\r']).to_string();
        let rope = b.rope();
        let lb = rope.char_to_byte(b.line_start(line));
        let le = lb + text.len();
        let spans = match &self.syntax {
            Some(s) => s.highlights(&self.text_cache.1, lb..le),
            None => Vec::new(),
        };
        let mut runs = Vec::new();
        let mut cur = lb;
        for sp in spans.iter().filter(|s| s.range.end > lb && s.range.start < le) {
            let (s, e) = (sp.range.start.max(lb).max(cur), sp.range.end.min(le));
            if s > cur {
                runs.push(run(s - cur, Theme::text()));
            }
            if e > s {
                runs.push(run(e - s, Theme::syntax(sp.kind)));
                cur = e;
            }
        }
        if le > cur {
            runs.push(run(le - cur, Theme::text()));
        }
        (text, runs)
    }

    /// For smokes: the sticky lines shown (buffer lines, top offsets).
    pub fn sticky_shown(&self) -> Vec<(usize, f32)> {
        self.sticky.shown.clone()
    }
}

impl Editor {
    /// For smokes: a press in the middle of sticky line `k`, as a click would do it.
    pub fn click_sticky_for_smoke(&mut self, k: usize, cx: &mut Context<Self>) -> bool {
        let (Some(l), Some(&(_, y))) = (self.layout.as_ref(), self.sticky.shown.get(k)) else { return false };
        let p = gpui::point(l.text_left + gpui::px(40.), l.bounds.top() + gpui::px(y.max(0.0) + line_height() / 2.0));
        self.sticky_click(p, cx)
    }

    /// For smokes: put the view's top at `y` (no glide).
    pub fn set_scroll_for_smoke(&mut self, y: f32, cx: &mut Context<Self>) {
        self.motion.glide = None;
        self.scroll_y = y.clamp(0.0, self.max_scroll_y());
        cx.notify();
    }

    /// For smokes: where `line` is drawn, from the editor's top.
    pub fn line_top_for_smoke(&self, line: usize) -> f32 {
        self.row_of(line) as f32 * line_height() - self.scroll_y
    }
}

impl Editor {
    /// For smokes: the mouse over sticky line `k`.
    pub fn hover_sticky_for_smoke(&mut self, k: usize, cx: &mut Context<Self>) {
        self.sticky.hover = (k < self.sticky.shown.len()).then_some(k);
        cx.notify();
    }
}
