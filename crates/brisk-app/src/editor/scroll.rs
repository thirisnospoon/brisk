//! Scrolling: horizontal scroll for long lines, the scrollbars (with error, change and search
//! marks on the vertical one), dragging them, and auto-scroll while selecting past an edge.
//! Over a scrollbar the pointer is an arrow and the thumb gets lighter and wider (also while
//! it is dragged); a click on the track beside the thumb scrolls a page toward the click.

use super::{line_height, Editor};
use gpui::{Context, MouseMoveEvent, Pixels, Point, Window};

/// Scrollbar track thickness.
pub(super) const BAR: f32 = 12.0;
const MIN_THUMB: f32 = 24.0;

/// A scrollbar.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(super) enum Bar {
    Vertical,
    Horizontal,
}

#[derive(Clone, Copy, Debug)]
pub(super) enum Drag {
    /// Dragging the vertical thumb; offset of the grab point inside it.
    Vertical(f32),
    Horizontal(f32),
}

impl Editor {
    /// Longest line in chars, cached per buffer version.
    pub(super) fn refresh_line_extent(&mut self) {
        let v = self.buffer.version();
        if self.max_line.0 != v && !(self.max_line.0 != u64::MAX && self.defer_extent()) {
            let n = self.buffer.len_lines();
            let longest = (0..n).map(|l| self.buffer.line_len(l)).max().unwrap_or(0);
            self.max_line = (v, longest);
        }
    }

    /// Width of the area that shows text (between the gutter and the vertical scrollbar).
    fn view_width(&self) -> f32 {
        self.layout.as_ref().map(|l| f32::from(l.bounds.right() - l.text_left) - BAR).unwrap_or(600.0).max(1.0)
    }

    fn view_height(&self) -> f32 {
        self.layout.as_ref().map(|l| f32::from(l.bounds.size.height)).unwrap_or(600.0).max(1.0)
    }

    fn char_w(&self) -> f32 {
        self.layout.as_ref().map(|l| f32::from(l.char_w)).unwrap_or(8.0)
    }

    pub(super) fn max_scroll_x(&self) -> f32 {
        let extent = (self.max_line.1 + 4) as f32 * self.char_w();
        (extent - self.view_width()).max(0.0)
    }

    pub(super) fn max_scroll_y(&self) -> f32 {
        self.display_rows().saturating_sub(1) as f32 * line_height()
    }

    pub(super) fn clamp_scroll_x(&mut self) {
        self.refresh_line_extent();
        self.scroll_x = self.scroll_x.clamp(0.0, self.max_scroll_x());
    }

    /// Scroll sideways so the primary caret stays in view.
    pub(super) fn ensure_caret_visible_x(&mut self) {
        let Some(l) = self.layout.as_ref() else { return };
        let (line, col) = self.buffer.pos_to_line_col(self.buffer.primary().head);
        let text = self.buffer.line(line);
        let x = if let Some(shaped) = l.shaped(line) {
            f32::from(shaped.x_for_index(self.display_byte(line, &text, super::char_to_byte(&text, col))))
        } else {
            col as f32 * f32::from(l.char_w)
        };
        let (w, cw) = (self.view_width(), self.char_w());
        if x < self.scroll_x + cw {
            self.scroll_x = (x - 4.0 * cw).max(0.0);
        } else if x > self.scroll_x + w - 2.0 * cw {
            self.scroll_x = x - w + 4.0 * cw;
        }
        self.scroll_x = self.scroll_x.max(0.0);
    }

    /// Vertical thumb: (top, height) relative to the editor's top; None when all text fits.
    pub(super) fn v_thumb(&self, vh: f32) -> Option<(f32, f32)> {
        let max = self.max_scroll_y();
        if (self.display_rows() as f32) * line_height() <= vh || max <= 0.0 {
            return None;
        }
        let h = (vh * vh / (max + vh)).clamp(MIN_THUMB.min(vh), vh);
        Some(((self.scroll_y / max).clamp(0.0, 1.0) * (vh - h), h))
    }

    /// Horizontal thumb: (left, width) relative to the text area's left; None when lines fit.
    pub(super) fn h_thumb(&self, vw: f32) -> Option<(f32, f32)> {
        let max = self.max_scroll_x();
        if max <= 0.0 {
            return None;
        }
        let w = (vw * vw / (max + vw)).clamp(MIN_THUMB.min(vw), vw);
        Some(((self.scroll_x / max).clamp(0.0, 1.0) * (vw - w), w))
    }

    /// The scrollbar under `p`, if it is shown there.
    pub(super) fn scrollbar_at(&self, p: Point<Pixels>) -> Option<Bar> {
        let l = self.layout.as_ref()?;
        if !self.show_gutter {
            return None;
        }
        let b = l.bounds;
        let (vh, vw) = (self.view_height(), self.view_width());
        let (x, y) = (f32::from(p.x - b.left()), f32::from(p.y - b.top()));
        if !(0.0..f32::from(b.size.width)).contains(&x) || !(0.0..vh).contains(&y) {
            return None;
        }
        if x >= f32::from(b.size.width) - BAR {
            return self.v_thumb(vh).map(|_| Bar::Vertical);
        }
        if y >= vh - BAR && p.x >= l.text_left {
            return self.h_thumb(vw).map(|_| Bar::Horizontal);
        }
        None
    }

    /// Mouse moves over the editor: track which scrollbar is under the pointer.
    pub(super) fn hover_scrollbar(&mut self, p: Option<Point<Pixels>>, cx: &mut Context<Self>) {
        let bar = p.and_then(|p| self.scrollbar_at(p));
        if bar != self.bar_hover {
            self.bar_hover = bar;
            cx.notify();
        }
    }

    /// A scrollbar is hovered or dragged: drawn lighter and wider.
    pub(super) fn bar_hot(&self, bar: Bar) -> bool {
        self.bar_hover == Some(bar)
            || matches!((self.drag, bar), (Some(Drag::Vertical(_)), Bar::Vertical) | (Some(Drag::Horizontal(_)), Bar::Horizontal))
    }

    /// A press on a scrollbar: on the thumb start dragging it; beside it scroll a page that way.
    pub(super) fn scrollbar_down(&mut self, p: Point<Pixels>) -> bool {
        let Some(l) = self.layout.as_ref() else { return false };
        if !self.show_gutter {
            return false;
        }
        let (b, text_left) = (l.bounds, l.text_left);
        let (vh, vw) = (self.view_height(), self.view_width());
        let (x, y) = (f32::from(p.x - b.left()), f32::from(p.y - b.top()));
        if x >= f32::from(b.size.width) - BAR {
            let Some((top, h)) = self.v_thumb(vh) else { return true };
            if y >= top && y <= top + h {
                self.drag = Some(Drag::Vertical(y - top));
                self.drag_scrollbar(p);
            } else {
                let page = (vh - line_height()).max(line_height());
                self.scroll_y = (self.scroll_y + if y < top { -page } else { page }).clamp(0.0, self.max_scroll_y());
            }
            return true;
        }
        if y >= vh - BAR && p.x >= text_left {
            let Some((left, w)) = self.h_thumb(vw) else { return false };
            let hx = f32::from(p.x - text_left);
            if hx >= left && hx <= left + w {
                self.drag = Some(Drag::Horizontal(hx - left));
                self.drag_scrollbar(p);
            } else {
                let page = (vw - 4.0 * self.char_w()).max(self.char_w());
                self.scroll_x = (self.scroll_x + if hx < left { -page } else { page }).clamp(0.0, self.max_scroll_x());
            }
            return true;
        }
        false
    }

    fn drag_scrollbar(&mut self, p: Point<Pixels>) {
        let Some(l) = self.layout.as_ref() else { return };
        let (b, text_left) = (l.bounds, l.text_left);
        let (vh, vw) = (self.view_height(), self.view_width());
        match self.drag {
            Some(Drag::Vertical(grab)) => {
                if let Some((_, h)) = self.v_thumb(vh) {
                    let top = f32::from(p.y - b.top()) - grab;
                    let room = (vh - h).max(1.0);
                    self.scroll_y = (top / room).clamp(0.0, 1.0) * self.max_scroll_y();
                }
            }
            Some(Drag::Horizontal(grab)) => {
                if let Some((_, w)) = self.h_thumb(vw) {
                    let left = f32::from(p.x - text_left) - grab;
                    let room = (vw - w).max(1.0);
                    self.scroll_x = (left / room).clamp(0.0, 1.0) * self.max_scroll_x();
                }
            }
            None => {}
        }
    }

    /// Mouse moves anywhere in the window while the left button is down: scrollbar drags and
    /// selection drags keep working outside the editor.
    pub(super) fn global_drag(&mut self, e: &MouseMoveEvent, window: &mut Window, cx: &mut Context<Self>) {
        if self.exec_dragging() {
            self.exec_drag_to(e.position);
            cx.notify();
            return;
        }
        if self.drag.is_some() {
            self.drag_scrollbar(e.position);
            cx.notify();
            return;
        }
        if !self.selecting {
            return;
        }
        self.drag_pos = Some(e.position);
        if self.column_anchor.is_some() {
            self.drag_column(e.position);
        } else if let Some(pos) = self.pos_for_point(e.position) {
            self.buffer.set_caret(pos, true);
        }
        let outside = self.layout.as_ref().is_some_and(|l| {
            let b = l.bounds;
            e.position.y < b.top() || e.position.y > b.bottom() || e.position.x < l.text_left || e.position.x > b.right()
        });
        if outside && !self.autoscrolling {
            self.start_autoscroll(window, cx);
        } else if !outside {
            self.ensure_caret_visible_x();
        }
        cx.notify();
    }

    pub(super) fn end_drag(&mut self) {
        self.drag = None;
        self.selecting = false;
        self.drag_pos = None;
        self.column_anchor = None;
    }

    /// While a selection drag is held past an edge, keep scrolling (faster further out).
    fn start_autoscroll(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        self.autoscrolling = true;
        let bg = cx.background_executor().clone();
        cx.spawn_in(window, async move |this, cx| {
            loop {
                bg.timer(std::time::Duration::from_millis(30)).await;
                let go_on = this
                    .update(cx, |ed, cx| {
                        let (Some(p), Some(l)) = (ed.drag_pos, ed.layout.as_ref()) else { return false };
                        if !ed.selecting {
                            return false;
                        }
                        let b = l.bounds;
                        let text_left = l.text_left;
                        let dy = if p.y < b.top() { f32::from(p.y - b.top()) } else if p.y > b.bottom() { f32::from(p.y - b.bottom()) } else { 0.0 };
                        let dx = if p.x < text_left { f32::from(p.x - text_left) } else if p.x > b.right() { f32::from(p.x - b.right()) } else { 0.0 };
                        if dy == 0.0 && dx == 0.0 {
                            return false;
                        }
                        ed.scroll_y = (ed.scroll_y + dy.clamp(-200.0, 200.0) * 0.5).clamp(0.0, ed.max_scroll_y());
                        ed.scroll_x = (ed.scroll_x + dx.clamp(-200.0, 200.0) * 0.5).clamp(0.0, ed.max_scroll_x());
                        if ed.column_anchor.is_some() {
                            ed.drag_column(p);
                        } else if let Some(pos) = ed.pos_for_point(p) {
                            ed.buffer.set_caret(pos, true);
                        }
                        cx.notify();
                        true
                    })
                    .unwrap_or(false);
                if !go_on {
                    break;
                }
            }
            let _ = this.update(cx, |ed, _| ed.autoscrolling = false);
        })
        .detach();
    }

    /// Wheel / touchpad: Shift turns vertical wheel motion sideways.
    pub(super) fn scroll_by(&mut self, dx: f32, dy: f32, shift: bool) {
        let (dx, dy) = if shift && dx == 0.0 { (dy, 0.0) } else { (dx, dy) };
        self.scroll_y = (self.scroll_y - dy).clamp(0.0, self.max_scroll_y());
        self.scroll_x = (self.scroll_x - dx).clamp(0.0, self.max_scroll_x());
    }
}
