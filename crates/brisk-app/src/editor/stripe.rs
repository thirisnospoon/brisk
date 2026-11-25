//! The error stripe, as in PyCharm: marks on the vertical scrollbar for errors, warnings, TODO
//! comments, search hits and changed lines, and a tick for the caret line. The marks are built
//! from cached data (problems, VCS changes, find matches, a TODO scan per text version) only when
//! one of those changes; a frame just places them. A click on a mark goes to it, hovering shows
//! what is there.

use super::{line_height, scroll::BAR, Editor};
use crate::theme::Theme;
use brisk_lint::Severity;
#[cfg(feature = "smoke")]
use gpui::point;
use gpui::{px, Context, Hsla, Pixels, Point};

/// Files with more lines are not scanned for TODO comments on every edit.
const TODO_MAX_LINES: usize = 50_000;
/// How far (px) from a mark the pointer still hits it.
const HIT: f32 = 3.0;

#[derive(Clone, Copy, PartialEq, Eq, Debug, PartialOrd, Ord)]
pub(super) enum Kind {
    Added,
    Modified,
    Deleted,
    Search,
    Todo,
    Warning,
    Error,
}

impl Kind {
    pub(super) fn color(self) -> Hsla {
        match self {
            Kind::Added => Theme::vcs_added(),
            Kind::Modified => Theme::vcs_modified(),
            Kind::Deleted => Theme::vcs_deleted(),
            Kind::Search => Theme::search_current(),
            Kind::Todo => Theme::todo(),
            Kind::Warning => super::severity_color(Severity::Warning),
            Kind::Error => super::severity_color(Severity::Error),
        }
    }

    fn vcs(self) -> bool {
        matches!(self, Kind::Added | Kind::Modified | Kind::Deleted)
    }
}

pub(super) struct Mark {
    pub(super) kind: Kind,
    /// Display rows it spans (fractional for nothing: whole rows), first and past the last.
    pub(super) rows: (f32, f32),
    pub(super) line: usize,
    pub(super) col: usize,
    pub(super) tip: String,
}

#[derive(PartialEq, Eq, Clone, Copy, Default)]
struct Key {
    version: u64,
    diags: (u64, usize, usize),
    vcs: (usize, usize),
    find: (usize, usize, bool),
    rows: usize,
}

#[derive(Default)]
pub(super) struct Stripe {
    key: Option<Key>,
    /// Painted in order: changes first, errors last (on top).
    pub(super) marks: Vec<Mark>,
    /// TODO comments for a text version: (line, column, text).
    todo: (Option<u64>, Vec<(usize, usize, String)>),
    /// Marks under the pointer (indices, the most important first) and its y in the editor.
    pub(super) hover: Option<(Vec<usize>, f32)>,
    /// How many times the marks were rebuilt (smoke: idle frames must not rebuild them).
    builds: usize,
}

impl Editor {
    fn stripe_key(&self) -> Key {
        let d = &self.diagnostics;
        let vcs_sum = self.vcs_changes.iter().map(|c| c.lines.start * 31 + c.lines.end).sum();
        let find = self
            .find
            .as_ref()
            .map_or((0, 0, false), |f| (f.matches.len(), f.matches.first().map_or(0, |m| m.start) ^ f.matches.last().map_or(0, |m| m.end), true));
        Key {
            version: self.buffer.version(),
            diags: (self.diag_version, d.len(), d.iter().map(|d| d.start.line).sum()),
            vcs: (self.vcs_changes.len(), vcs_sum),
            find,
            rows: self.display_rows(),
        }
    }

    /// Before a frame: rebuild the marks if their sources changed.
    pub(super) fn update_stripe(&mut self) {
        if !self.show_gutter {
            return;
        }
        let key = self.stripe_key();
        if self.stripe.key == Some(key) {
            return;
        }
        self.stripe.key = Some(key);
        self.stripe.builds += 1;
        self.refresh_todo();
        let row = |ed: &Editor, l: usize| ed.row_of(l) as f32;
        let mut marks = Vec::new();
        for ch in &self.vcs_changes {
            use brisk_git::LineChangeKind as K;
            let (kind, tip) = match ch.kind {
                K::Added => (Kind::Added, "Added lines"),
                K::Modified => (Kind::Modified, "Modified lines"),
                K::Deleted => (Kind::Deleted, "Deleted lines"),
            };
            let (a, z) = (row(self, ch.lines.start), row(self, ch.lines.end.max(ch.lines.start)));
            marks.push(Mark { kind, rows: (a, z.max(a)), line: ch.lines.start, col: 0, tip: tip.into() });
        }
        if let Some(f) = &self.find {
            let mut last = usize::MAX;
            for m in &f.matches {
                let (l, c) = self.buffer.pos_to_line_col(m.start);
                if l != last {
                    last = l;
                    let r = row(self, l);
                    marks.push(Mark { kind: Kind::Search, rows: (r, r + 1.0), line: l, col: c, tip: format!("{}: {}", l + 1, self.buffer.line(l).trim()) });
                }
            }
        }
        for (l, c, text) in &self.stripe.todo.1 {
            let r = row(self, *l);
            marks.push(Mark { kind: Kind::Todo, rows: (r, r + 1.0), line: *l, col: *c, tip: text.clone() });
        }
        for d in &self.diagnostics {
            let kind = match d.severity {
                Severity::Error => Kind::Error,
                Severity::Warning => Kind::Warning,
                // Unused code is greyed out in the text; PyCharm marks it in the stripe as a
                // weak warning.
                Severity::Unused => Kind::Warning,
            };
            let r = row(self, d.start.line);
            marks.push(Mark { kind, rows: (r, r + 1.0), line: d.start.line, col: d.start.col, tip: d.title() });
        }
        marks.sort_by_key(|m| m.kind);
        self.stripe.marks = marks;
        self.stripe.hover = None;
    }

    fn refresh_todo(&mut self) {
        let v = self.buffer.version();
        if self.stripe.todo.0 == Some(v) {
            return;
        }
        let mut out = Vec::new();
        if self.buffer.len_lines() <= TODO_MAX_LINES {
            let plain = self
                .path
                .as_ref()
                .and_then(|p| p.extension())
                .and_then(|e| e.to_str())
                .is_some_and(|e| matches!(e.to_ascii_lowercase().as_str(), "txt" | "md" | "rst"));
            for (l, line) in self.text().split('\n').enumerate() {
                if let Some((at, text)) = crate::todo_scan::find_in_line(line, plain) {
                    out.push((l, line[..at].chars().count(), text));
                }
            }
        }
        self.stripe.todo = (Some(v), out);
    }

    /// Track geometry: (top of the editor, its height, the content height in px).
    fn stripe_track(&self) -> Option<(Pixels, f32, f32)> {
        let l = self.layout.as_ref()?;
        let vh = f32::from(l.bounds.size.height);
        Some((l.bounds.top(), vh, (self.display_rows() as f32 * line_height()).max(vh)))
    }

    /// Where a mark is drawn: (top, height) relative to the editor's top.
    pub(super) fn stripe_y(&self, m: &Mark, vh: f32, total: f32) -> (f32, f32) {
        let k = line_height() / total * vh;
        let top = m.rows.0 * k;
        let h = if m.kind.vcs() {
            ((m.rows.1 - m.rows.0) * k).max(2.0)
        } else if matches!(m.kind, Kind::Error | Kind::Warning) {
            3.0
        } else {
            2.0
        };
        (top, h)
    }

    /// The caret line's tick: its top relative to the editor's top.
    pub(super) fn stripe_caret_y(&self, vh: f32, total: f32) -> f32 {
        self.row_of(self.caret_line_col().0) as f32 * line_height() / total * vh
    }

    /// Marks at this point on the scrollbar, the most important first.
    fn stripe_at(&self, p: Point<Pixels>) -> Option<(Vec<usize>, f32)> {
        let l = self.layout.as_ref()?;
        if !self.show_gutter || self.stripe.marks.is_empty() {
            return None;
        }
        let x = f32::from(p.x - l.bounds.left());
        if x < f32::from(l.bounds.size.width) - BAR || x > f32::from(l.bounds.size.width) {
            return None;
        }
        let (top, vh, total) = self.stripe_track()?;
        let y = f32::from(p.y - top);
        let mut hit: Vec<(f32, usize)> = self
            .stripe
            .marks
            .iter()
            .enumerate()
            .filter_map(|(i, m)| {
                let (t, h) = self.stripe_y(m, vh, total);
                let d = if y < t {
                    t - y
                } else if y > t + h {
                    y - t - h
                } else {
                    0.0
                };
                (d <= HIT).then_some((d, i))
            })
            .collect();
        if hit.is_empty() {
            return None;
        }
        hit.sort_by(|a, b| self.stripe.marks[b.1].kind.cmp(&self.stripe.marks[a.1].kind).then(a.0.total_cmp(&b.0)));
        Some((hit.into_iter().map(|(_, i)| i).collect(), y))
    }

    /// A press on the scrollbar: on a mark go to it (before the thumb and the track).
    pub(super) fn stripe_click(&mut self, p: Point<Pixels>, cx: &mut Context<Self>) -> bool {
        let Some((hit, _)) = self.stripe_at(p) else { return false };
        let m = &self.stripe.marks[hit[0]];
        let line = m.line.min(self.buffer.len_lines().saturating_sub(1));
        let pos = self.buffer.line_start(line) + m.col.min(self.buffer.line_len(line));
        self.buffer.set_caret(pos, false);
        self.center_caret();
        self.ensure_caret_visible_x();
        self.stripe.hover = None;
        cx.notify();
        true
    }

    /// The pointer moved (None: it left the editor).
    pub(super) fn stripe_hover(&mut self, p: Option<Point<Pixels>>, cx: &mut Context<Self>) {
        let hover = p.and_then(|p| self.stripe_at(p));
        let same = match (&hover, &self.stripe.hover) {
            (Some((a, _)), Some((b, _))) => a == b,
            (None, None) => true,
            _ => false,
        };
        if !same {
            self.stripe.hover = hover;
            cx.notify();
        }
    }

    /// What the hovered marks are: a box left of the scrollbar.
    pub(super) fn render_stripe_tip(&self) -> Option<gpui::AnyElement> {
        use gpui::{div, prelude::*};
        let (hit, y) = self.stripe.hover.as_ref()?;
        let l = self.layout.as_ref()?;
        let mut seen: Vec<(Kind, &str)> = Vec::new();
        for m in hit.iter().map(|&i| &self.stripe.marks[i]) {
            if !seen.contains(&(m.kind, m.tip.as_str())) {
                seen.push((m.kind, m.tip.as_str()));
            }
        }
        let more = seen.len().saturating_sub(4);
        let rows: Vec<gpui::AnyElement> = seen
            .iter()
            .take(4)
            .map(|&(kind, tip)| {
                let icon = match kind {
                    Kind::Error => "circle-x",
                    Kind::Warning => "triangle-alert",
                    Kind::Todo => "list-todo",
                    Kind::Search => "search",
                    _ => "git-commit",
                };
                div()
                    .flex()
                    .items_center()
                    .gap(px(6.))
                    .child(crate::icons::icon(icon, 13., kind.color()))
                    .child(div().text_ellipsis().overflow_hidden().whitespace_nowrap().child(tip.to_string()))
                    .into_any_element()
            })
            .collect();
        // Below the pointer in the upper part of the editor, above it in the lower part.
        let vh = f32::from(l.bounds.size.height);
        let below = *y < vh * 0.6;
        Some(
            div()
                .absolute()
                .right(px(BAR + 6.))
                .when(below, |d| d.top(px((*y - 12.0).max(4.0))))
                .when(!below, |d| d.bottom(px((vh - *y - 12.0).max(4.0))))
                .max_w(px(520.))
                .flex()
                .flex_col()
                .gap(px(3.))
                .px(px(10.))
                .py(px(6.))
                .rounded(px(6.))
                .border_1()
                .border_color(Theme::divider())
                .bg(Theme::panel())
                .shadow_md()
                .text_size(px(12.5))
                .text_color(Theme::text())
                .children(rows)
                .when(more > 0, |d| d.child(div().text_color(Theme::text_dim()).child(format!("and {more} more"))))
                .into_any_element(),
        )
    }

    /// Smoke: how many times the marks were rebuilt.
    #[cfg(feature = "smoke")]
    pub fn stripe_builds_for_smoke(&self) -> usize {
        self.stripe.builds
    }

    /// Smoke: the marks as (kind, line), painted order.
    #[cfg(feature = "smoke")]
    pub fn stripe_for_smoke(&mut self) -> Vec<(String, usize)> {
        self.update_stripe();
        self.stripe.marks.iter().map(|m| (format!("{:?}", m.kind), m.line + 1)).collect()
    }

    /// Smoke: the editor-relative y of the first mark of this kind ("Error", "Todo"...).
    #[cfg(feature = "smoke")]
    pub fn stripe_point_for_smoke(&self, kind: &str) -> Option<Point<Pixels>> {
        let l = self.layout.as_ref()?;
        let (top, vh, total) = self.stripe_track()?;
        let m = self.stripe.marks.iter().find(|m| format!("{:?}", m.kind) == kind)?;
        let (t, h) = self.stripe_y(m, vh, total);
        Some(point(l.bounds.right() - px(4.), top + px(t + h / 2.)))
    }

    /// Smoke: hover / click the stripe at a window point.
    #[cfg(feature = "smoke")]
    pub fn stripe_hover_for_smoke(&mut self, p: Point<Pixels>, cx: &mut Context<Self>) -> Vec<String> {
        self.stripe_hover(Some(p), cx);
        self.stripe.hover.as_ref().map(|(h, _)| h.iter().map(|&i| self.stripe.marks[i].tip.clone()).collect()).unwrap_or_default()
    }

    #[cfg(feature = "smoke")]
    pub fn stripe_click_for_smoke(&mut self, p: Point<Pixels>, cx: &mut Context<Self>) -> bool {
        self.stripe_click(p, cx)
    }
}
