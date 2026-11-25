//! Motion in the editor, drawn frame by frame (GPUI animation frames, so it follows the
//! display's refresh rate):
//! - smooth scrolling: wheel notches, Page Up / Page Down and jumps (Ctrl+Home, Go to Line,
//!   Find Next) glide there with an ease-out; touchpads scroll 1:1. Small moves (an arrow key
//!   at the edge, Enter on the last line) stay instant, as in PyCharm.
//! - the caret blinks by fading out and in (solid while typing or moving it), 2 px wide and
//!   snapped to device pixels;
//! - optional smooth caret movement: the caret slides to its new place in ~60 ms.
//! Between fades nothing is redrawn: a timer wakes the editor for the next one.

use super::{line_height, Editor};
use gpui::{point, App, Entity, Pixels, Point, Task, Window};
use std::time::{Duration, Instant};

/// A wheel notch settles in this time.
const WHEEL: Duration = Duration::from_millis(120);
/// Page Up / Page Down and jumps.
const JUMP: Duration = Duration::from_millis(150);
/// Changes up to this many rows do not animate.
const INSTANT_ROWS: f32 = 2.0;
/// Caret blink: PyCharm's 500 ms on, 500 ms off; the fades take part of each.
const BLINK: f32 = 1000.0;
const SOLID_UNTIL: f32 = 430.0;
const FADE: f32 = 140.0;
const CARET_SLIDE: Duration = Duration::from_millis(60);
const FADE_FRAME: Duration = Duration::from_millis(33);

/// A scroll animation: `scroll_y` goes from `from` to `to`; `last` is the value it last set,
/// so a scroll by anything else (a scrollbar drag, the touchpad) cancels it.
#[derive(Clone, Copy, Debug)]
pub(super) struct Glide {
    from: f32,
    pub(super) to: f32,
    start: Instant,
    dur: Duration,
    last: f32,
}

#[derive(Default)]
pub(super) struct Motion {
    pub(super) glide: Option<Glide>,
    /// The caret blink restarts (solid) from here.
    blink_epoch: Option<Instant>,
    /// What the carets looked like at the last paint: (buffer version, primary head, count).
    caret_key: (u64, usize, usize),
    /// Smooth caret movement: (from, to, start) in content coordinates (scroll not applied).
    slide: Option<(Point<Pixels>, Point<Pixels>, Instant)>,
    /// The timer that wakes the editor for the next fade, and when it fires.
    wake: Option<(Instant, Task<()>)>,
    /// For smokes: paints so far, the carets' last opacity and the primary caret's last x.
    paints: u64,
    alpha: f32,
    caret_x: f32,
}

/// What a smoke sees of the motion (`Editor::motion_probe`).
#[derive(Clone, Copy, Debug)]
#[cfg(feature = "smoke")]
pub struct MotionProbe {
    pub scroll_y: f32,
    /// Where a glide in progress is going.
    pub target: Option<f32>,
    pub paints: u64,
    pub alpha: f32,
    pub caret_x: f32,
    /// The primary caret's display row counted from the top of the view.
    pub caret_screen_row: f32,
    pub line_height: f32,
}

fn ease_out(t: f32) -> f32 {
    let t = t.clamp(0.0, 1.0);
    1.0 - (1.0 - t).powi(3)
}

fn smoothstep(t: f32) -> f32 {
    let t = t.clamp(0.0, 1.0);
    t * t * (3.0 - 2.0 * t)
}

/// The caret's opacity `ms` into a blink period, and how long it stays as it is (None while
/// it fades).
fn blink_at(ms: f32) -> (f32, Option<f32>) {
    let t = ms % BLINK;
    let (out_end, in_start) = (SOLID_UNTIL + FADE, BLINK - FADE);
    if t < SOLID_UNTIL {
        (1.0, Some(SOLID_UNTIL - t))
    } else if t < out_end {
        (1.0 - smoothstep((t - SOLID_UNTIL) / FADE), None)
    } else if t < in_start {
        (0.0, Some(in_start - t))
    } else {
        (smoothstep((t - in_start) / FADE), None)
    }
}

impl Editor {
    /// Start of a change to the scroll position: work on where the view is going (the target
    /// of a glide in progress) and return where it is now.
    pub(super) fn begin_scroll(&mut self) -> f32 {
        let shown = self.scroll_y;
        if let Some(g) = &self.motion.glide {
            self.scroll_y = g.to;
        }
        shown
    }

    /// End of a change started with `begin_scroll`: glide from `shown` to the new position, or
    /// jump there when it is close (or smooth scrolling is off).
    pub(super) fn end_scroll(&mut self, shown: f32, dur: Duration) {
        let target = self.scroll_y;
        if self.motion.glide.is_some_and(|g| g.to == target) {
            // Unchanged: the glide in progress goes on.
            self.scroll_y = shown;
            return;
        }
        let far = (target - shown).abs() > INSTANT_ROWS * line_height();
        if !crate::settings::smooth_scrolling() || self.layout.is_none() || !far {
            self.motion.glide = None;
            return;
        }
        self.scroll_y = shown;
        self.motion.glide = Some(Glide { from: shown, to: target, start: Instant::now(), dur, last: shown });
    }

    /// Mouse wheel: notches add up to one target.
    pub fn smooth_scroll(&mut self, dy: f32, cx: &mut gpui::Context<Self>) {
        let shown = self.begin_scroll();
        self.scroll_y = (self.scroll_y - dy).clamp(0.0, self.max_scroll_y());
        if crate::settings::smooth_scrolling() && self.layout.is_some() && self.scroll_y != shown {
            let target = self.scroll_y;
            self.scroll_y = shown;
            self.motion.glide = Some(Glide { from: shown, to: target, start: Instant::now(), dur: WHEEL, last: shown });
        } else {
            self.motion.glide = None;
        }
        cx.notify();
    }

    /// Page Up / Page Down: the view moves a page with the caret, which keeps its place on
    /// screen (PyCharm).
    pub(super) fn page_scroll(&mut self, down: bool, rows: usize) {
        let shown = self.begin_scroll();
        let d = rows as f32 * line_height();
        self.scroll_y = (self.scroll_y + if down { d } else { -d }).clamp(0.0, self.max_scroll_y());
        self.end_scroll(shown, JUMP);
    }

    pub(super) fn jump_duration() -> Duration {
        JUMP
    }

    /// Before laying out a frame: advance the scroll animation. True while it runs.
    pub(super) fn step_glide(&mut self) -> bool {
        let Some(g) = self.motion.glide else { return false };
        if self.scroll_y != g.last {
            self.motion.glide = None;
            return false;
        }
        let t = g.start.elapsed().as_secs_f32() / g.dur.as_secs_f32();
        if t >= 1.0 {
            self.scroll_y = g.to.clamp(0.0, self.max_scroll_y());
            self.motion.glide = None;
            return false;
        }
        self.scroll_y = g.from + (g.to - g.from) * ease_out(t);
        self.motion.glide = Some(Glide { last: self.scroll_y, ..g });
        true
    }

    #[cfg(feature = "smoke")]
    pub fn motion_probe(&self) -> MotionProbe {
        MotionProbe {
            scroll_y: self.scroll_y,
            target: self.motion.glide.map(|g| g.to),
            paints: self.motion.paints,
            alpha: self.motion.alpha,
            caret_x: self.motion.caret_x,
            caret_screen_row: self.caret_row() as f32 - self.scroll_y / line_height(),
            line_height: line_height(),
        }
    }
}

/// What `caret_look` decided for one paint.
pub(super) struct CaretLook {
    /// Opacity of the carets.
    pub alpha: f32,
    /// Where to draw the primary caret (screen coordinates), when it slides.
    pub primary: Option<Point<Pixels>>,
    /// Draw the next frame too.
    pub animating: bool,
    /// Where the primary caret is drawn (x), for smokes.
    pub drawn_x: Option<Pixels>,
    /// Wake the editor after this long (the next fade).
    pub wake_in: Option<Duration>,
    state: (Option<Instant>, (u64, usize, usize), Option<(Point<Pixels>, Point<Pixels>, Instant)>),
}

impl Motion {
    /// The carets for this paint: `key` identifies the text and carets (any change restarts the
    /// blink, solid), `target` is the primary caret's place on screen and `scroll` the view's
    /// offset (the slide runs in content coordinates, so scrolling does not make it slide).
    pub(super) fn caret_look(&self, key: (u64, usize, usize), target: Option<Point<Pixels>>, scroll: Point<Pixels>) -> CaretLook {
        let now = Instant::now();
        let epoch = match self.blink_epoch {
            Some(e) if self.caret_key == key => e,
            _ => now,
        };
        let mut animating = false;
        let mut primary = None;
        let mut slide = None;
        if let (true, Some(t)) = (crate::settings::smooth_caret(), target) {
            let to = point(t.x + scroll.x, t.y + scroll.y);
            let at = |s: &(Point<Pixels>, Point<Pixels>, Instant)| {
                let k = ease_out(now.duration_since(s.2).as_secs_f32() / CARET_SLIDE.as_secs_f32());
                point(s.0.x + (s.1.x - s.0.x) * k, s.0.y + (s.1.y - s.0.y) * k)
            };
            let s = match self.slide {
                Some(s) if s.1 == to => s,
                // Moved: slide on from where it is drawn now.
                Some(s) => (at(&s), to, now),
                None => (to, to, now),
            };
            if now.duration_since(s.2) < CARET_SLIDE && s.0 != s.1 {
                animating = true;
                let p = at(&s);
                primary = Some(point(p.x - scroll.x, p.y - scroll.y));
            }
            slide = Some(s);
        }
        let blink = crate::settings::caret_blinking() && !animating;
        let (alpha, still) = if blink { blink_at(now.duration_since(epoch).as_secs_f32() * 1000.0) } else { (1.0, None) };
        // A fade is drawn at ~30 frames a second (smooth enough for an opacity change, half
        // the cost of every display frame); between fades nothing is drawn until the next.
        let wake_in = match still {
            Some(ms) => Some(Duration::from_secs_f32(ms / 1000.0)),
            None if blink => Some(FADE_FRAME),
            None => None,
        };
        let drawn_x = primary.or(target).map(|p| p.x);
        CaretLook { alpha, primary, animating, wake_in, drawn_x, state: (Some(epoch), key, slide) }
    }
}

impl Editor {
    /// After painting the carets: keep their state and schedule the next frame or wake-up.
    pub(super) fn keep_caret_look(&mut self, look: CaretLook, me: &Entity<Editor>, window: &mut Window, cx: &mut App) {
        let (epoch, key, slide) = look.state;
        self.motion.blink_epoch = epoch;
        self.motion.caret_key = key;
        self.motion.slide = slide;
        self.motion.paints += 1;
        self.motion.alpha = look.alpha;
        if let Some(x) = look.drawn_x {
            self.motion.caret_x = f32::from(x);
        }
        if look.animating {
            self.motion.wake = None;
            window.request_animation_frame();
            return;
        }
        let Some(d) = look.wake_in else {
            self.motion.wake = None;
            return;
        };
        let at = Instant::now() + d;
        // One timer at a time; a repaint before it fires keeps it if it is still due then.
        if self
            .motion
            .wake
            .as_ref()
            .is_some_and(|(w, _)| w.saturating_duration_since(at) < Duration::from_millis(4) && at.saturating_duration_since(*w) < Duration::from_millis(4))
        {
            return;
        }
        let weak = me.downgrade();
        let task = window.spawn(cx, async move |cx| {
            cx.background_executor().timer(d).await;
            let _ = weak.update(cx, |_, cx| cx.notify());
        });
        self.motion.wake = Some((at, task));
    }

    /// The editor lost focus: no caret, nothing to wake for.
    pub(super) fn stop_caret_look(&mut self) {
        self.motion.paints += 1;
        self.motion.wake = None;
        self.motion.slide = None;
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn blink_fades_and_rests() {
        assert_eq!(blink_at(0.0).0, 1.0);
        assert!(blink_at(0.0).1.is_some());
        let (a, still) = blink_at(SOLID_UNTIL + FADE / 2.0);
        assert!(a > 0.3 && a < 0.7 && still.is_none());
        assert_eq!(blink_at(700.0).0, 0.0);
        assert!(blink_at(BLINK - 1.0).0 > 0.95);
        assert_eq!(blink_at(BLINK).0, 1.0);
        assert!((ease_out(1.0) - 1.0).abs() < 1e-6 && ease_out(0.5) > 0.8);
    }
}
