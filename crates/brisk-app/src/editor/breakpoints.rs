//! Line breakpoints (Ctrl+F8, a click on the line numbers) and the execution point of a
//! paused debugger. Breakpoints follow edits like bookmarks.

use super::{Editor, EditorEvent};

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Breakpoint {
    pub condition: Option<String>,
    /// What the debugger said: None until a session reports, Some(false) = never stops.
    pub verified: Option<bool>,
}

#[derive(Default)]
pub struct Breakpoints {
    /// (char position of the line start, breakpoint).
    marks: Vec<(usize, Breakpoint)>,
    version: u64,
}

impl Editor {
    fn sync_breakpoints(&mut self) {
        let v = self.buffer.version();
        if self.breakpoints.version == v {
            return;
        }
        let marks = self.breakpoints_mapped();
        self.breakpoints = Breakpoints { marks: marks.into_iter().map(|(l, b)| (self.buffer.line_start(l), b)).collect(), version: v };
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
        self.breakpoints = Breakpoints { marks, version: self.buffer.version() };
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
                self.breakpoints.marks.push((at, Breakpoint { condition: None, verified: None }));
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
                self.breakpoints.marks.push((at, Breakpoint { condition, verified: None }));
                self.breakpoints.marks.sort_by_key(|m| m.0);
            }
        }
        cx.emit(EditorEvent::BreakpointsChanged);
        cx.notify();
    }

    /// What the debugger reported, in the order of `breakpoint_list`.
    pub fn set_breakpoints_verified(&mut self, verified: &[Option<usize>]) {
        self.sync_breakpoints();
        for (m, v) in self.breakpoints.marks.iter_mut().zip(verified) {
            m.1.verified = Some(v.is_some());
        }
    }

    pub fn clear_breakpoints_verified(&mut self) {
        for m in &mut self.breakpoints.marks {
            m.1.verified = None;
        }
    }

    /// Click on the line numbers: toggle a breakpoint there.
    pub(super) fn breakpoint_click(&mut self, p: gpui::Point<gpui::Pixels>, cx: &mut gpui::Context<Self>) -> bool {
        let Some(l) = self.layout.as_ref() else { return false };
        if !self.show_gutter || self.is_field || !self.can_debug() {
            return false;
        }
        let numbers_right = l.text_left - gpui::px(8. + 25.) - self.run_slot();
        let numbers_left = l.bounds.left() + gpui::px(18.) + self.annotation_width(l.char_w);
        if p.x < numbers_left || p.x > numbers_right {
            return false;
        }
        let y = f32::from(p.y - l.bounds.top()) + self.scroll_y;
        let line = self.line_of_row((y / super::line_height()).floor().max(0.0) as usize);
        if line >= self.buffer.len_lines() {
            return false;
        }
        self.toggle_breakpoint(line, cx);
        true
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
