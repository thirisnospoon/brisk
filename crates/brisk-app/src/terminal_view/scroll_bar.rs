//! The scrollbar of the Run and Terminal consoles, over the scrollback (in lines). Hidden in
//! full-screen apps (alt screen). In Run, dragging up stops following the output and back at
//! the bottom it follows again, like the wheel.

use super::TerminalPanel;
use crate::scrollbar::{scrollbar, Metrics, Source};
use gpui::{Context, IntoElement};
use std::rc::Rc;

/// For smokes: (metrics, hovered, dragging, track bounds, thumb).
#[cfg(feature = "smoke")]
type SmokeBar = (Option<Metrics>, bool, bool, Option<gpui::Bounds<gpui::Pixels>>, Option<(f32, f32)>);

impl TerminalPanel {
    pub(super) fn scroll_metrics(&self) -> Option<Metrics> {
        let s = self.session()?;
        if s.alt_screen() {
            return None;
        }
        let (offset, rows, history) = s.scroll_pos();
        (history > 0).then(|| Metrics {
            top: history.saturating_sub(offset) as f32,
            max: history as f32,
            view: rows as f32,
            page: rows.saturating_sub(1).max(1) as f32,
        })
    }

    fn set_scroll_top(&mut self, top: f32, cx: &mut Context<Self>) {
        let Some(s) = self.session() else { return };
        let (offset, _, history) = s.scroll_pos();
        let target = (history as f32 - top.round()).clamp(0., history as f32) as i32;
        if target != offset as i32 {
            s.scroll(target - offset as i32);
        }
        if self.run_mode {
            self.scroll_to_end = target == 0;
        }
        cx.notify();
    }

    pub(super) fn scrollbar(&self, cx: &mut Context<Self>) -> impl IntoElement {
        let (read, write) = (cx.entity().downgrade(), cx.entity().downgrade());
        let source = Source::Custom {
            metrics: Rc::new(move |cx| read.upgrade()?.read(cx).scroll_metrics()),
            set: Rc::new(move |top, _, cx| {
                let _ = write.update(cx, |p, cx| p.set_scroll_top(top, cx));
            }),
        };
        scrollbar(source, self.bar.clone())
    }

    /// For smokes: (metrics, hovered, dragging, track bounds, thumb).
    #[allow(dead_code)]
    #[cfg(feature = "smoke")]
    pub fn bar_state_for_smoke(&self) -> SmokeBar {
        let b = self.bar.borrow();
        (self.scroll_metrics(), b.hover, b.drag.is_some(), b.bounds, b.thumb)
    }
}
