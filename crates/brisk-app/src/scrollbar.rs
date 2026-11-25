//! A vertical scrollbar overlay for panels (Project tree, Run/Terminal, Python Console) that
//! looks and works like the editor's: an arrow pointer over it, a lighter and wider thumb while
//! hovered or dragged, dragging the thumb, and a click on the track pages toward the click.
//! The panel keeps a [`BarState`] and puts [`scrollbar`] last inside a `relative()` container.

use crate::theme::Theme;
use gpui::{
    canvas, div, fill, point, px, size, App, Bounds, CursorStyle, DispatchPhase, Hitbox, HitboxBehavior, IntoElement, MouseButton, MouseDownEvent,
    MouseMoveEvent, MouseUpEvent, ParentElement, Pixels, ScrollHandle, Styled, Window,
};
use std::cell::RefCell;
use std::rc::Rc;

pub const BAR: f32 = 12.0;
const MIN_THUMB: f32 = 24.0;

/// Scroll position in the source's own units (pixels, terminal lines).
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Metrics {
    /// Distance from the top, 0..=max.
    pub top: f32,
    pub max: f32,
    /// Visible extent.
    pub view: f32,
    /// One track click scrolls this much.
    pub page: f32,
}

/// Hover and drag of one scrollbar, kept by the panel across frames.
#[derive(Default, Debug)]
pub struct Bar {
    pub hover: bool,
    /// Dragging the thumb: offset of the grab point inside it.
    pub drag: Option<f32>,
    /// Track bounds as last painted (window coordinates).
    pub bounds: Option<Bounds<Pixels>>,
    /// Thumb (top, height) relative to the track, as last painted.
    pub thumb: Option<(f32, f32)>,
}

pub type BarState = Rc<RefCell<Bar>>;

impl Bar {
    pub fn hot(&self) -> bool {
        self.hover || self.drag.is_some()
    }
}

/// Where the scrollbar reads and sets the position.
#[derive(Clone)]
pub enum Source {
    /// A GPUI scroll container (`overflow_y_scroll`, or a `uniform_list`'s base handle).
    Handle(ScrollHandle),
    Custom {
        metrics: GetMetrics,
        set: SetTop,
    },
}

/// Reads a custom source's position.
type GetMetrics = Rc<dyn Fn(&App) -> Option<Metrics>>;
/// Scrolls a custom source to a top (in its own units).
type SetTop = Rc<dyn Fn(f32, &mut Window, &mut App)>;

impl Source {
    pub fn list(h: &gpui::UniformListScrollHandle) -> Self {
        Source::Handle(h.0.borrow().base_handle.clone())
    }

    /// A variable-height `gpui::list` (console output). Heights of rows not yet in view are
    /// unknown, so the thumb is approximate, as in any lazily measured list.
    pub fn list_state(s: &gpui::ListState) -> Self {
        let (a, b) = (s.clone(), s.clone());
        Source::Custom {
            metrics: Rc::new(move |_| {
                let view = f32::from(a.viewport_bounds().size.height);
                let max = f32::from(a.max_offset_for_scrollbar().height);
                let top = (-f32::from(a.scroll_px_offset_for_scrollbar().y)).clamp(0., max.max(0.));
                (max > 0.5 && view > 0.).then(|| Metrics { top, max, view, page: (view - 20.).max(view * 0.5) })
            }),
            set: Rc::new(move |top, window, _| {
                b.set_offset_from_scrollbar(gpui::point(gpui::px(0.), gpui::px(-top)));
                window.refresh();
            }),
        }
    }

    pub fn metrics(&self, cx: &App) -> Option<Metrics> {
        match self {
            Source::Handle(h) => {
                let view = f32::from(h.bounds().size.height);
                let max = f32::from(h.max_offset().height);
                (max > 0.5 && view > 0.).then(|| Metrics { top: (-f32::from(h.offset().y)).clamp(0., max) + 0., max, view, page: (view - 20.).max(view * 0.5) })
            }
            Source::Custom { metrics, .. } => metrics(cx),
        }
    }

    fn set(&self, top: f32, window: &mut Window, cx: &mut App) {
        match self {
            Source::Handle(h) => {
                let max = f32::from(h.max_offset().height);
                h.set_offset(point(h.offset().x, px(-top.clamp(0., max))));
                window.refresh();
            }
            Source::Custom { set, .. } => set(top, window, cx),
        }
    }
}

/// Thumb (top, height) on a track `track` long.
fn thumb(m: Metrics, track: f32) -> (f32, f32) {
    let h = (track * m.view / (m.max + m.view)).clamp(MIN_THUMB.min(track), track);
    ((m.top / m.max).clamp(0., 1.) * (track - h), h)
}

/// The overlay: absolutely placed along the right edge of its `relative()` parent.
pub fn scrollbar(source: Source, state: BarState) -> impl IntoElement {
    div().absolute().top_0().right_0().bottom_0().w(px(BAR)).child(
        canvas(
            |bounds, window, _| window.insert_hitbox(bounds, HitboxBehavior::BlockMouseExceptScroll),
            move |bounds, hitbox: Hitbox, window, cx| paint(bounds, hitbox, source, state, window, cx),
        )
        .size_full(),
    )
}

fn paint(bounds: Bounds<Pixels>, hitbox: Hitbox, source: Source, state: BarState, window: &mut Window, cx: &mut App) {
    let Some(m) = source.metrics(cx) else {
        let mut s = state.borrow_mut();
        (s.hover, s.drag, s.bounds, s.thumb) = (false, None, None, None);
        return;
    };
    let track = f32::from(bounds.size.height);
    let (top, h) = thumb(m, track);
    let hot = {
        let mut s = state.borrow_mut();
        s.bounds = Some(bounds);
        s.thumb = Some((top, h));
        s.hot()
    };
    window.set_cursor_style(CursorStyle::Arrow, &hitbox);
    let (color, t) = if hot { (Theme::scroll_thumb_hot(), BAR - 2.) } else { (Theme::scroll_thumb(), BAR - 4.) };
    if hot {
        window.paint_quad(fill(bounds, Theme::scroll_track_hot()));
    }
    window.paint_quad(fill(Bounds::new(point(bounds.right() - px(t + 1.), bounds.top() + px(top)), size(px(t), px(h))), color).corner_radii(px(t / 2.)));

    // Thumb top (in track pixels) -> source position.
    let to_top = move |thumb_top: f32| if track > h { thumb_top / (track - h) * m.max } else { 0. };
    {
        let (state, source) = (state.clone(), source.clone());
        window.on_mouse_event(move |e: &MouseDownEvent, phase, window, cx| {
            if phase != DispatchPhase::Bubble || e.button != MouseButton::Left || !bounds.contains(&e.position) {
                return;
            }
            cx.stop_propagation();
            let y = f32::from(e.position.y - bounds.top());
            if y >= top && y < top + h {
                state.borrow_mut().drag = Some(y - top);
                window.refresh();
            } else {
                let step = if y < top { -m.page } else { m.page };
                source.set((m.top + step).clamp(0., m.max), window, cx);
            }
        });
    }
    {
        let (state, source) = (state.clone(), source.clone());
        window.on_mouse_event(move |e: &MouseMoveEvent, phase, window, cx| {
            if phase != DispatchPhase::Bubble {
                return;
            }
            let grab = state.borrow().drag;
            if let Some(grab) = grab {
                if !e.dragging() {
                    state.borrow_mut().drag = None;
                    window.refresh();
                    return;
                }
                let y = f32::from(e.position.y - bounds.top()) - grab;
                source.set(to_top(y.clamp(0., track - h)).clamp(0., m.max), window, cx);
                cx.stop_propagation();
                return;
            }
            let over = bounds.contains(&e.position);
            if state.borrow().hover != over {
                state.borrow_mut().hover = over;
                window.refresh();
            }
        });
    }
    window.on_mouse_event(move |e: &MouseUpEvent, phase, window, _| {
        if phase == DispatchPhase::Bubble && e.button == MouseButton::Left && state.borrow().drag.is_some() {
            let mut s = state.borrow_mut();
            s.drag = None;
            s.hover = bounds.contains(&e.position);
            window.refresh();
        }
    });
}
