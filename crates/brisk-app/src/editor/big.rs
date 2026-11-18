//! Large files (over `BIG_LINES` lines): whole-file work that runs on every edit is throttled
//! so typing stays fast. Fold markers and the longest-line width refresh at most once a
//! second while typing, completion words come from the lines around the caret, and inlay
//! hints are off. The syntax tree itself is reparsed incrementally (brisk-syntax); a reparse
//! on the UI thread gets `PARSE_BUDGET`, and one that needs longer (error recovery after an
//! unclosed bracket can take seconds) finishes on a background thread while the shifted old
//! tree keeps the highlights in place.

use super::Editor;
use gpui::Context;
use std::time::{Duration, Instant};

/// Files longer than this count as large.
pub const BIG_LINES: usize = 20_000;

/// How often throttled whole-file work may run while the text keeps changing.
const REFRESH: Duration = Duration::from_millis(1000);

/// Longest reparse on the UI thread in a large file.
pub const PARSE_BUDGET: Duration = Duration::from_millis(30);

/// Lines around the caret that word completion looks at in a large file.
pub const WORD_WINDOW: usize = 2_000;

#[derive(Default)]
pub struct BigFile {
    folds_at: Option<Instant>,
    extent_at: Option<Instant>,
    /// A background parse is running.
    parsing: bool,
}

impl Editor {
    pub fn is_big(&self) -> bool {
        self.buffer.len_lines() > BIG_LINES
    }

    /// Starts a background parse when the last one on the UI thread ran out of budget.
    pub(super) fn background_parse(&mut self, cx: &mut Context<Self>) {
        if self.big.parsing {
            return;
        }
        let Some((lang, text, tree)) = self.syntax.as_ref().and_then(|s| s.stale_job()) else { return };
        self.big.parsing = true;
        let job = cx.background_executor().spawn(async move {
            let tree = brisk_syntax::parse_full(lang, &text, Some(&tree));
            (text, tree)
        });
        cx.spawn(async move |this, cx| {
            let (text, tree) = job.await;
            let _ = this.update(cx, |e, cx| {
                e.big.parsing = false;
                if let (Some(s), Some(tree)) = (e.syntax.as_mut(), tree) {
                    s.adopt(&text, tree);
                }
                // Repaint with the new tree; if the text moved on, the next frame starts another job.
                e.folds.candidates.0 = None;
                cx.notify();
            });
        })
        .detach();
    }

    /// Large file and the folds were refreshed less than a second ago: keep them for now.
    pub(super) fn defer_folds(&mut self) -> bool {
        defer(self.buffer.len_lines(), &mut self.big.folds_at)
    }

    /// Large file and the longest line was measured less than a second ago.
    pub(super) fn defer_extent(&mut self) -> bool {
        defer(self.buffer.len_lines(), &mut self.big.extent_at)
    }
}

/// True to skip the refresh now; otherwise records that it runs.
fn defer(lines: usize, at: &mut Option<Instant>) -> bool {
    if lines <= BIG_LINES {
        return false;
    }
    if at.is_some_and(|t| t.elapsed() < REFRESH) {
        return true;
    }
    *at = Some(Instant::now());
    false
}
