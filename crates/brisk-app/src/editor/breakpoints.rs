//! Line breakpoints (Ctrl+F8, a click on the line numbers) and the execution point of a
//! paused debugger. Breakpoints follow edits like bookmarks.

use super::{Editor, EditorEvent};
use std::collections::HashMap;

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Breakpoint {
    pub condition: Option<String>,
    /// What the debugger said: None until a session reports, Some(false) = never stops.
    pub verified: Option<bool>,
    /// Disabled in View Breakpoints: kept, drawn grey, not sent to the debugger.
    pub enabled: bool,
}

impl Breakpoint {
    pub fn new(condition: Option<String>) -> Self {
        Self { condition, verified: None, enabled: true }
    }
}

#[derive(Default)]
pub struct Breakpoints {
    /// (char position of the line start, breakpoint).
    marks: Vec<(usize, Breakpoint)>,
    version: u64,
    /// Inline values of a stop: (buffer version they were made for, line → "a: 1, b: 2").
    inline: Option<(u64, HashMap<usize, String>)>,
    /// Mute Breakpoints: all are drawn grey (the debugger gets none).
    muted: bool,
    /// The execution arrow is being dragged: the line it would move to.
    exec_drag: Option<usize>,
    /// The line numbers' line under the pointer, where the pointer entered it, and whether
    /// it had a breakpoint then (its tooltip; a click that just set one shows none).
    hover: Option<(usize, gpui::Point<gpui::Pixels>, bool)>,
    /// What is under the pointer in the gutter takes a click: the hand cursor.
    hand: bool,
}

impl Editor {
    fn sync_breakpoints(&mut self) {
        let v = self.buffer.version();
        if self.breakpoints.version == v {
            return;
        }
        let marks = self.breakpoints_mapped();
        self.breakpoints.marks = marks.into_iter().map(|(l, b)| (self.buffer.line_start(l), b)).collect();
        self.breakpoints.version = v;
    }

    /// (line, breakpoint), following edits made since the last sync.
    fn breakpoints_mapped(&self) -> Vec<(usize, Breakpoint)> {
        let b = &self.buffer;
        let changes = (self.breakpoints.version != b.version()).then(|| b.changes_since(self.breakpoints.version)).flatten();
        let mut v: Vec<(usize, Breakpoint)> = self
            .breakpoints
            .marks
            .iter()
            .map(|(p, bp)| {
                let p = changes.as_deref().map_or(*p, |c| brisk_text::map_through(*p, c)).min(b.len_chars());
                (b.pos_to_line_col(p).0, bp.clone())
            })
            .collect();
        v.sort_by_key(|x| x.0);
        v.dedup_by_key(|x| x.0);
        v
    }

    /// Breakpoints as (line, breakpoint), by line.
    pub fn breakpoint_list(&self) -> Vec<(usize, Breakpoint)> {
        self.breakpoints_mapped()
    }

    pub fn set_breakpoints(&mut self, v: &[(usize, Breakpoint)]) {
        let n = self.buffer.len_lines();
        let mut marks: Vec<(usize, Breakpoint)> = v.iter().filter(|(l, _)| *l < n).map(|(l, b)| (self.buffer.line_start(*l), b.clone())).collect();
        marks.sort_by_key(|m| m.0);
        marks.dedup_by_key(|m| m.0);
        self.breakpoints.marks = marks;
        self.breakpoints.version = self.buffer.version();
    }

    pub fn set_breakpoints_muted(&mut self, muted: bool) {
        self.breakpoints.muted = muted;
    }

    pub fn breakpoints_muted(&self) -> bool {
        self.breakpoints.muted
    }

    /// Values shown after the ends of lines while the debugger is paused in this file.
    pub fn set_inline_values(&mut self, values: HashMap<usize, String>) {
        self.breakpoints.inline = Some((self.buffer.version(), values));
    }

    pub fn clear_inline_values(&mut self) -> bool {
        self.breakpoints.inline.take().is_some()
    }

    /// The inline value of `line`, while the text is what the debugger stopped on.
    pub fn inline_value(&self, line: usize) -> Option<&str> {
        let (v, m) = self.breakpoints.inline.as_ref()?;
        if *v != self.buffer.version() {
            return None;
        }
        m.get(&line).map(String::as_str)
    }

    /// Ctrl+F8 / a click on the line number: add or remove the breakpoint on `line`.
    pub fn toggle_breakpoint(&mut self, line: usize, cx: &mut gpui::Context<Self>) -> bool {
        self.sync_breakpoints();
        let at = self.buffer.line_start(line.min(self.buffer.len_lines().saturating_sub(1)));
        let added = match self.breakpoints.marks.iter().position(|m| m.0 == at) {
            Some(i) => {
                self.breakpoints.marks.remove(i);
                false
            }
            None => {
                self.breakpoints.marks.push((at, Breakpoint::new(None)));
                self.breakpoints.marks.sort_by_key(|m| m.0);
                true
            }
        };
        cx.emit(EditorEvent::BreakpointsChanged);
        cx.notify();
        added
    }

    /// The condition of the breakpoint on `line` (adds one if there is none).
    pub fn set_breakpoint_condition(&mut self, line: usize, condition: Option<String>, cx: &mut gpui::Context<Self>) {
        self.sync_breakpoints();
        let at = self.buffer.line_start(line.min(self.buffer.len_lines().saturating_sub(1)));
        let condition = condition.filter(|c| !c.trim().is_empty());
        match self.breakpoints.marks.iter_mut().find(|m| m.0 == at) {
            Some(m) => m.1.condition = condition,
            None => {
                self.breakpoints.marks.push((at, Breakpoint::new(condition)));
                self.breakpoints.marks.sort_by_key(|m| m.0);
            }
        }
        cx.emit(EditorEvent::BreakpointsChanged);
        cx.notify();
    }

    /// Enable or disable the breakpoint on `line` (View Breakpoints).
    pub fn set_breakpoint_enabled(&mut self, line: usize, enabled: bool, cx: &mut gpui::Context<Self>) {
        self.sync_breakpoints();
        let at = self.buffer.line_start(line.min(self.buffer.len_lines().saturating_sub(1)));
        if let Some(m) = self.breakpoints.marks.iter_mut().find(|m| m.0 == at) {
            m.1.enabled = enabled;
            if !enabled {
                m.1.verified = None;
            }
            cx.emit(EditorEvent::BreakpointsChanged);
            cx.notify();
        }
    }

    /// What the debugger reported, in the order of `breakpoint_list`.
    pub fn set_breakpoints_verified(&mut self, verified: &[Option<usize>]) {
        self.sync_breakpoints();
        for (m, v) in self.breakpoints.marks.iter_mut().filter(|m| m.1.enabled).zip(verified) {
            m.1.verified = Some(v.is_some());
        }
    }

    pub fn clear_breakpoints_verified(&mut self) {
        for m in &mut self.breakpoints.marks {
            m.1.verified = None;
        }
    }

    /// The line under `p` in the line numbers, where a click sets a breakpoint.
    pub(super) fn breakpoint_zone_line(&self, p: gpui::Point<gpui::Pixels>) -> Option<usize> {
        let l = self.layout.as_ref()?;
        if !self.show_gutter || self.is_field || !self.can_debug() {
            return None;
        }
        let numbers_right = l.text_left - gpui::px(8. + 25.) - self.run_slot();
        let numbers_left = l.bounds.left() + gpui::px(18.) + self.annotation_width(l.char_w);
        if p.x < numbers_left || p.x > numbers_right {
            return None;
        }
        let y = f32::from(p.y - l.bounds.top()) + self.scroll_y;
        let line = self.line_of_row((y / super::line_height()).floor().max(0.0) as usize);
        (line < self.buffer.len_lines()).then_some(line)
    }

    /// Click on the line numbers: toggle a breakpoint there.
    pub(super) fn breakpoint_click(&mut self, p: gpui::Point<gpui::Pixels>, cx: &mut gpui::Context<Self>) -> bool {
        let Some(line) = self.breakpoint_zone_line(p) else { return false };
        self.toggle_breakpoint(line, cx);
        true
    }

    /// The pointer moved (`in_gutter`: over the gutter). Over the line numbers: a faint
    /// breakpoint where a click would set one, or the tooltip of the one there, as in PyCharm;
    /// the hand cursor over whatever takes a click (line numbers, ▶, fold markers). True when
    /// something to draw changed.
    pub(super) fn gutter_pointer_move(&mut self, p: gpui::Point<gpui::Pixels>, in_gutter: bool) -> bool {
        let zone = if in_gutter { self.breakpoint_zone_line(p) } else { None };
        let hand = zone.is_some() || in_gutter && (self.run_mark_at(p).is_some() || self.fold_marker_at(p));
        let mut changed = hand != self.breakpoints.hand;
        self.breakpoints.hand = hand;
        if zone != self.breakpoints.hover.map(|h| h.0) {
            self.breakpoints.hover = zone.map(|l| (l, p, self.breakpoint_list().iter().any(|(b, _)| *b == l)));
            changed = true;
        }
        changed
    }

    /// The pointer left the editor. True when something to draw changed.
    pub(super) fn gutter_pointer_leave(&mut self) -> bool {
        let had = self.breakpoints.hover.is_some() || self.breakpoints.hand;
        self.breakpoints.hover = None;
        self.breakpoints.hand = false;
        had
    }

    /// The editor's cursor: the hand over what takes a click in the gutter, an arrow over the
    /// rest of it and the scrollbars, the I-beam over the text.
    pub fn pointer_style(&self) -> gpui::CursorStyle {
        if self.middle_active() {
            gpui::CursorStyle::ResizeUpDown
        } else if self.bar_hover.is_some() || self.drag.is_some() {
            gpui::CursorStyle::Arrow
        } else if self.breakpoints.hand {
            gpui::CursorStyle::PointingHand
        } else if self.gutter_hover {
            gpui::CursorStyle::Arrow
        } else {
            gpui::CursorStyle::IBeam
        }
    }

    /// The line that shows a faint breakpoint under the pointer: one without a breakpoint.
    pub(super) fn breakpoint_preview_line(&self) -> Option<usize> {
        let (line, _, _) = self.breakpoints.hover?;
        (!self.breakpoint_list().iter().any(|(b, _)| *b == line)).then_some(line)
    }

    /// The tooltip of the breakpoint under the pointer: where it is, its condition, its state.
    fn breakpoint_tip_lines(&self) -> Option<Vec<String>> {
        let (line, _, tip) = self.breakpoints.hover?;
        if !tip {
            return None;
        }
        let bp = self.breakpoint_list().into_iter().find(|(b, _)| *b == line)?.1;
        let file = self.path.as_ref().and_then(|p| p.file_name()).map(|n| n.to_string_lossy().into_owned()).unwrap_or_default();
        let mut v = vec![format!("Line {} in {file}", line + 1)];
        if let Some(c) = bp.condition.as_ref().filter(|c| !c.trim().is_empty()) {
            v.push(format!("Condition: {c}"));
        }
        if self.breakpoints_muted() {
            v.push("Breakpoints are muted".into());
        } else if !bp.enabled {
            v.push("Disabled".into());
        } else if bp.verified == Some(false) {
            v.push("The debugger cannot stop on this line".into());
        }
        Some(v)
    }

    pub(super) fn render_breakpoint_tip(&self) -> Option<gpui::AnyElement> {
        use crate::theme::Theme;
        use gpui::prelude::FluentBuilder as _;
        use gpui::{px, IntoElement, ParentElement, Styled};
        let lines = self.breakpoint_tip_lines()?;
        let (_, at, _) = self.breakpoints.hover?;
        let rel = at - self.layout.as_ref()?.bounds.origin;
        Some(
            gpui::div()
                .absolute()
                .left(rel.x + px(12.))
                .top(rel.y + px(18.))
                .max_w(px(420.))
                .px(px(8.))
                .py(px(5.))
                .rounded(px(6.))
                .border_1()
                .border_color(Theme::divider())
                .bg(Theme::panel())
                .shadow_md()
                .font_family(crate::theme::ui_font())
                .text_size(px(12.5))
                .text_color(Theme::text())
                .children(lines.into_iter().enumerate().map(|(i, s)| gpui::div().when(i > 0, |d| d.text_color(Theme::text_muted())).child(s)))
                .into_any_element(),
        )
    }

    /// Smoke checks: the breakpoint zone line under the pointer, the line with a faint
    /// breakpoint, the hand cursor, the tooltip.
    pub fn gutter_state_for_smoke(&self) -> (Option<usize>, Option<usize>, bool, Option<String>) {
        (self.breakpoints.hover.map(|h| h.0), self.breakpoint_preview_line(), self.breakpoints.hand, self.breakpoint_tip_lines().map(|v| v.join(" | ")))
    }

    /// Smoke screenshots: the pointer over the gutter at `p` (window, logical pixels), held
    /// although the real pointer is outside the window and Windows reports it left.
    pub fn hold_gutter_hover_for_smoke(&mut self, p: (f32, f32)) {
        self.gutter_hover = true;
        self.breakpoints.hover = None;
        self.gutter_pointer_move(gpui::point(gpui::px(p.0), gpui::px(p.1)), true);
    }

    /// Smoke checks: window position (logical pixels) of `line`'s breakpoint circle.
    pub fn breakpoint_point(&self, line: usize) -> Option<(f32, f32)> {
        let l = self.layout.as_ref()?;
        let lh = super::line_height();
        let y = l.bounds.top() + gpui::px(self.row_of(line) as f32 * lh - self.scroll_y + lh / 2.);
        let x = l.text_left - gpui::px(36.5) - self.run_slot();
        Some((f32::from(x), f32::from(y)))
    }

    /// A press on the execution arrow starts dragging it (Set Next Statement).
    pub(super) fn exec_arrow_down(&mut self, p: gpui::Point<gpui::Pixels>) -> bool {
        let (Some(l), Some(exec)) = (self.layout.as_ref(), self.exec_line) else { return false };
        if !self.show_gutter || p.x < l.bounds.left() || p.x >= l.bounds.left() + gpui::px(18.) {
            return false;
        }
        let y = f32::from(p.y - l.bounds.top()) + self.scroll_y;
        if self.line_of_row((y / super::line_height()).floor().max(0.0) as usize) != exec {
            return false;
        }
        self.breakpoints.exec_drag = Some(exec);
        true
    }

    pub(super) fn exec_dragging(&self) -> bool {
        self.breakpoints.exec_drag.is_some()
    }

    /// Where a dragged execution arrow is drawn.
    pub(super) fn exec_drag_target(&self) -> Option<usize> {
        self.breakpoints.exec_drag.filter(|t| Some(*t) != self.exec_line)
    }

    pub(super) fn exec_drag_to(&mut self, p: gpui::Point<gpui::Pixels>) {
        let Some(l) = self.layout.as_ref() else { return };
        let y = f32::from(p.y - l.bounds.top()) + self.scroll_y;
        let line = self.line_of_row((y / super::line_height()).floor().max(0.0) as usize).min(self.buffer.len_lines().saturating_sub(1));
        self.breakpoints.exec_drag = Some(line);
    }

    /// The arrow was dropped on another line: ask the debugger to continue from there.
    pub(super) fn exec_drag_end(&mut self, cx: &mut gpui::Context<Self>) {
        if let Some(t) = self.exec_drag_target() {
            cx.emit(EditorEvent::SetNextStatement(t));
        }
        self.breakpoints.exec_drag = None;
    }

    /// Right click: the line of a breakpoint under the point in the line numbers, if any.
    pub(super) fn breakpoint_at_point(&self, p: gpui::Point<gpui::Pixels>) -> Option<usize> {
        let l = self.layout.as_ref()?;
        if !self.show_gutter || p.x < l.bounds.left() + gpui::px(18.) + self.annotation_width(l.char_w) || p.x > l.text_left - gpui::px(8. + 25.) - self.run_slot() {
            return None;
        }
        let y = f32::from(p.y - l.bounds.top()) + self.scroll_y;
        let line = self.line_of_row((y / super::line_height()).floor().max(0.0) as usize);
        self.breakpoint_list().iter().any(|(b, _)| *b == line).then_some(line)
    }

    /// Breakpoints make sense in Python files.
    pub fn can_debug(&self) -> bool {
        self.is_python()
    }
}
