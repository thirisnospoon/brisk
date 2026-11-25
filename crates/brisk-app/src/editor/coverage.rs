//! Coverage marks in the gutter (Run with Coverage): green for executed lines, red for
//! missed ones, like PyCharm. They belong to the text they were measured on, so an edit
//! hides them until the next coverage run.

use super::Editor;
use std::collections::HashSet;

#[derive(Clone, Debug, Default)]
pub struct CoverageMarks {
    /// Buffer version the marks were set for.
    pub version: u64,
    pub covered: HashSet<usize>,
    pub missed: HashSet<usize>,
}

impl Editor {
    /// 0-based covered and missed lines of the current text; `None` clears the marks.
    pub fn set_coverage(&mut self, lines: Option<(&[usize], &[usize])>) {
        self.coverage =
            lines.map(|(c, m)| CoverageMarks { version: self.buffer.version(), covered: c.iter().copied().collect(), missed: m.iter().copied().collect() });
    }

    /// `Some(true)` covered, `Some(false)` missed, `None` no mark (or the text changed).
    pub fn coverage_at(&self, line: usize) -> Option<bool> {
        let c = self.coverage.as_ref().filter(|c| c.version == self.buffer.version())?;
        if c.covered.contains(&line) {
            Some(true)
        } else if c.missed.contains(&line) {
            Some(false)
        } else {
            None
        }
    }
}
