//! Scrolling with the middle button held, as in Windows and PyCharm: press the wheel, move
//! the mouse away from the press point (an anchor mark shows it) and the text scrolls that
//! way, faster the further the mouse is, on both axes; releasing the button stops. A press
//! and release without moving past `THRESHOLD` is a click: Go to Declaration (middle_click.rs).

use super::{Editor, EditorEvent};
use crate::theme::Theme;
use gpui::{prelude::*, px, AnyElement, Context, Pixels, Point, Window};
use std::time::Duration;

/// Movement (px) that turns a middle click into scrolling.
const THRESHOLD: f32 = 6.0;
const TICK: Duration = Duration::from_millis(16);
/// Scroll per tick for each pixel beyond the threshold.
const SPEED: f32 = 0.12;

pub(super) struct MiddleScroll {
    origin: Point<Pixels>,
    mouse: Point<Pixels>,
    moved: bool,
}

impl Editor {
    /// Middle button pressed in the text: arm scrolling around this point.
    pub(super) fn middle_scroll_start(&mut self, at: Point<Pixels>, window: &mut Window, cx: &mut Context<Self>) {
        self.middle = Some(MiddleScroll { origin: at, mouse: at, moved: false });
        self.motion.glide = None;
        cx.spawn_in(window, async move |this, cx| loop {
            cx.background_executor().timer(TICK).await;
            if !this.update(cx, |e, cx| e.middle_tick(cx)).unwrap_or(false) {
                break;
            }
        })
        .detach();
        cx.notify();
    }

    fn middle_tick(&mut self, cx: &mut Context<Self>) -> bool {
        let Some(m) = self.middle.as_mut() else { return false };
        let (dx, dy) = (f32::from(m.mouse.x - m.origin.x), f32::from(m.mouse.y - m.origin.y));
        if !m.moved && (dx.abs() > THRESHOLD || dy.abs() > THRESHOLD) {
            m.moved = true;
        }
        if !m.moved {
            return true;
        }
        let v = |d: f32| if d.abs() <= THRESHOLD { 0.0 } else { d.signum() * (d.abs() - THRESHOLD) * SPEED };
        let (vx, vy) = (v(dx), v(dy));
        if vx != 0.0 || vy != 0.0 {
            self.scroll_y = (self.scroll_y + vy).clamp(0.0, self.max_scroll_y());
            self.scroll_x = (self.scroll_x + vx).clamp(0.0, self.max_scroll_x());
            cx.notify();
        }
        true
    }

    /// The mouse moved anywhere in the window while scrolling is armed.
    pub(super) fn middle_move(&mut self, p: Point<Pixels>) {
        if let Some(m) = self.middle.as_mut() {
            m.mouse = p;
        }
    }

    pub(super) fn middle_active(&self) -> bool {
        self.middle.is_some()
    }

    /// Middle button released: stop; without movement it was a click (Go to Declaration).
    pub(super) fn middle_up(&mut self, click: bool, cx: &mut Context<Self>) {
        let Some(m) = self.middle.take() else { return };
        if click && !m.moved {
            if let Some(pos) = self.pos_for_point(m.origin) {
                cx.emit(EditorEvent::CtrlClick(pos));
            }
        }
        cx.notify();
    }

    /// The anchor mark at the press point, once the mouse has moved away from it.
    pub(super) fn render_middle_anchor(&self) -> Option<AnyElement> {
        let m = self.middle.as_ref().filter(|m| m.moved)?;
        let l = self.layout.as_ref()?;
        let rel = m.origin - l.bounds.origin;
        let arrow = |s: &'static str| gpui::div().h(px(9.)).line_height(px(9.)).child(s);
        Some(
            gpui::div()
                .absolute()
                .left(rel.x - px(15.))
                .top(rel.y - px(15.))
                .size(px(30.))
                .rounded_full()
                .border_1()
                .border_color(Theme::text_muted())
                .bg(Theme::panel())
                .shadow_md()
                .flex()
                .flex_col()
                .items_center()
                .justify_center()
                .text_size(px(8.))
                .text_color(Theme::text())
                .child(arrow("▲"))
                .child(gpui::div().flex().gap(px(3.)).h(px(9.)).line_height(px(9.)).child("◀").child("•").child("▶"))
                .child(arrow("▼"))
                .into_any_element(),
        )
    }

    /// Smokes: the scrollbar under the pointer, whether the vertical one is drawn hot, the
    /// vertical thumb (top, height), and whether middle-button scrolling is armed / moving.
    pub fn mouse_state_for_smoke(&self) -> (Option<String>, bool, Option<(f32, f32)>, Option<bool>) {
        let vh = self.layout.as_ref().map(|l| f32::from(l.bounds.size.height)).unwrap_or(0.0);
        (
            self.bar_hover.map(|b| format!("{b:?}")),
            self.bar_hot(super::scroll::Bar::Vertical),
            self.v_thumb(vh),
            self.middle.as_ref().map(|m| m.moved),
        )
    }
}
