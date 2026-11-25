//! Large files (over `BIG_LINES` lines): whole-file work that runs on every edit is throttled
//! so typing stays fast. The longest-line width refreshes at most once a second while
//! typing, completion words come from the lines around the caret, and inlay hints are off.
//! From `LAZY_FOLD_LINES` lines on, fold markers are recomputed on a background thread
//! `FOLD_DEBOUNCE` after the last edit instead of on every frame, and so are the ▶ run marks.
//! The syntax tree of a large file is not parsed on the UI thread after an edit: the old tree
//! is shifted to the new positions (highlights stay in place) and the incremental parse runs
//! on a background thread (`background_parse`). Only a first parse without a tree runs here,
//! within `PARSE_BUDGET`.

use super::Editor;
use gpui::Context;
use std::time::{Duration, Instant};

/// Files longer than this count as large.
pub const BIG_LINES: usize = 20_000;

/// How often throttled whole-file work may run while the text keeps changing.
const REFRESH: Duration = Duration::from_millis(1000);

/// From this many lines on, fold markers are computed off the UI thread.
pub const LAZY_FOLD_LINES: usize = 5_000;

/// Pause in typing before the fold markers are recomputed.
pub(super) const FOLD_DEBOUNCE: Duration = Duration::from_millis(300);

/// Longest reparse on the UI thread in a large file.
pub const PARSE_BUDGET: Duration = Duration::from_millis(30);
/// Opening a file larger than this parses only its first [`OPEN_PREVIEW_LINES`] lines on the
/// UI thread (highlighting what is in view), the rest in the background.
pub const OPEN_PARSE_BYTES: usize = 64 * 1024;
pub const OPEN_PREVIEW_LINES: usize = 300;

/// Lines around the caret that word completion looks at in a large file.
pub const WORD_WINDOW: usize = 2_000;

#[derive(Default)]
pub struct BigFile {
    /// Buffer version a fold recompute is scheduled for, and whether one is running.
    folds_for: Option<u64>,
    folds_running: bool,
    /// Buffer version a run-marks recompute is scheduled for.
    pub(super) marks_for: Option<u64>,
    extent_at: Option<Instant>,
    /// A background parse is running.
    parsing: bool,
    /// Collapse the imports once the first complete tree lands (a long file opens with only
    /// its first lines parsed).
    pub(super) collapse_pending: bool,
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
            let tree = brisk_syntax::parse_full(lang, &text, tree.as_ref());
            (text, tree)
        });
        cx.spawn(async move |this, cx| {
            let (text, tree) = job.await;
            let _ = this.update(cx, |e, cx| {
                e.big.parsing = false;
                if let (Some(s), Some(tree)) = (e.syntax.as_mut(), tree) {
                    s.adopt(&text, tree);
                }
                // Repaint with the new tree; fold markers from it too (also those computed
                // while there was no tree yet). If the text moved on, the next frame starts
                // another job.
                e.big.folds_for = None;
                if let Some(c) = e.folds.candidates.0.as_mut() {
                    *c = u64::MAX;
                }
                // The outline (Structure, breadcrumbs) was read without a complete tree.
                e.scope_cache = Default::default();
                if e.big.collapse_pending && e.syntax.as_ref().is_some_and(|s| !s.is_stale()) {
                    e.big.collapse_pending = false;
                    e.collapse_imports();
                }
                cx.notify();
            });
        })
        .detach();
    }

    /// Smokes: a complete syntax tree for the current text (highlighting is not stale).
    #[cfg(feature = "smoke")]
    pub fn syntax_ready(&self) -> bool {
        self.syntax.as_ref().is_some_and(|s| s.tree().is_some() && !s.is_stale())
    }

    /// Smokes: the fold markers are up to date with the text.
    #[cfg(feature = "smoke")]
    pub fn folds_fresh(&self) -> bool {
        self.folds.candidates.0 == Some(self.buffer.version())
    }

    /// Fold markers come from `schedule_folds` (a long file with a grammar).
    pub(super) fn lazy_folds(&self) -> bool {
        self.syntax.is_some() && self.buffer.len_lines() >= LAZY_FOLD_LINES
    }

    /// Prepaint: once typing pauses for `FOLD_DEBOUNCE`, recompute the fold regions from a
    /// copy of the syntax tree on a background thread.
    pub(super) fn schedule_folds(&mut self, cx: &mut Context<Self>) {
        let v = self.buffer.version();
        let fresh = self.folds.candidates.0 == Some(v);
        if !self.lazy_folds() || fresh || self.folds.candidates.0.is_none() || self.big.folds_running || self.big.folds_for == Some(v) {
            return;
        }
        self.big.folds_for = Some(v);
        cx.spawn(async move |this, cx| {
            cx.background_executor().timer(FOLD_DEBOUNCE).await;
            let job = this
                .update(cx, |e, cx| {
                    // Still typing (a newer edit scheduled its own), or the tree is being
                    // finished in the background (its arrival schedules again).
                    if e.buffer.version() != v || e.big.folds_running {
                        return None;
                    }
                    e.reparse();
                    let s = e.syntax.as_ref().filter(|s| !s.is_stale())?;
                    let (lang, tree) = (s.lang(), s.tree()?.clone());
                    e.big.folds_running = true;
                    Some(cx.background_executor().spawn(async move { brisk_syntax::fold_ranges_of(lang, &tree) }))
                })
                .ok()
                .flatten();
            let Some(job) = job else { return };
            let regions = job.await;
            let _ = this.update(cx, |e, cx| {
                e.big.folds_running = false;
                e.folds.candidates = (Some(v), regions);
                cx.notify();
            });
        })
        .detach();
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
