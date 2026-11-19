//! Annotate with Git Blame: author and date of every line in a gutter column, shaded by
//! age (newer is brighter). Hover shows the commit, a click opens its diff for the file.

use super::{line_height, Editor, EditorEvent};
use crate::theme::{ui_px, Theme};
use gpui::prelude::*;
use gpui::{point, px, Pixels, Point};

/// Characters of an annotation label: "2026-11-12 Alice Smith…".
pub const LABEL_CHARS: usize = 24;

#[derive(Clone, Debug)]
pub struct AnnLine {
    pub hash: String,
    pub author: String,
    pub email: String,
    pub time: i64,
    pub summary: String,
    /// 0.0 for the newest commit of the file, 1.0 for the oldest.
    pub age: f32,
}

impl AnnLine {
    pub fn label(&self) -> String {
        let mut author: String = self.author.chars().take(LABEL_CHARS - 11).collect();
        if self.author.chars().count() > LABEL_CHARS - 11 {
            author.pop();
            author.push('…');
        }
        format!("{} {author}", date(self.time))
    }
}

/// `YYYY-MM-DD` of a Unix time (UTC).
pub fn date(t: i64) -> String {
    // Civil-from-days (Howard Hinnant).
    let z = t.div_euclid(86_400) + 719_468;
    let era = z.div_euclid(146_097);
    let doe = z - era * 146_097;
    let yoe = (doe - doe / 1460 + doe / 36_524 - doe / 146_096) / 365;
    let y = yoe + era * 400;
    let doy = doe - (365 * yoe + yoe / 4 - yoe / 100);
    let mp = (5 * doy + 2) / 153;
    let d = doy - (153 * mp + 2) / 5 + 1;
    let m = if mp < 10 { mp + 3 } else { mp - 9 };
    format!("{:04}-{m:02}-{d:02}", if m <= 2 { y + 1 } else { y })
}

#[derive(Clone, Debug, Default)]
pub struct Annotation {
    /// One entry per buffer line; None for lines not committed yet.
    pub lines: Vec<Option<AnnLine>>,
    /// Buffer version the blame was computed for.
    pub version: u64,
}

impl Annotation {
    pub fn from_blame(blame: Vec<brisk_git::BlameLine>) -> Self {
        let mut times: Vec<i64> = blame.iter().filter(|b| !b.is_uncommitted()).map(|b| b.time).collect();
        times.sort_unstable();
        times.dedup();
        let n = times.len().max(2) - 1;
        let lines = blame
            .into_iter()
            .map(|b| {
                if b.is_uncommitted() {
                    return None;
                }
                let rank = times.len() - 1 - times.binary_search(&b.time).unwrap_or(0);
                Some(AnnLine { age: rank as f32 / n as f32, hash: b.hash, author: b.author, email: b.email, time: b.time, summary: b.summary })
            })
            .collect();
        Annotation { lines, version: 0 }
    }
}

impl Editor {
    pub fn annotation_shown(&self) -> bool {
        self.annotation.is_some()
    }

    /// Show (`Some`) or close the blame column.
    pub fn set_annotation(&mut self, a: Option<Annotation>, cx: &mut gpui::Context<Self>) {
        self.annotation = a;
        self.ann_hover = None;
        cx.notify();
    }

    /// Width the annotation column adds to the gutter.
    pub(super) fn annotation_width(&self, char_w: Pixels) -> Pixels {
        if self.annotation.is_some() && self.show_gutter {
            char_w * LABEL_CHARS as f32 + px(12.)
        } else {
            px(0.)
        }
    }

    /// Left edge of the annotation column (after the bookmark slot).
    pub(super) fn annotation_left(&self) -> Pixels {
        px(20.)
    }

    /// The annotated line under a point in the column, if any.
    pub(crate) fn annotation_at(&self, p: Point<Pixels>) -> Option<usize> {
        let l = self.layout.as_ref()?;
        let a = self.annotation.as_ref()?;
        let x0 = l.bounds.left() + self.annotation_left();
        if p.x < x0 || p.x > x0 + self.annotation_width(l.char_w) {
            return None;
        }
        let y = f32::from(p.y - l.bounds.top()) + self.scroll_y;
        let line = self.line_of_row((y / line_height()).floor().max(0.0) as usize);
        a.lines.get(line).and_then(|x| x.as_ref()).map(|_| line)
    }

    /// A click on an annotation: open that commit's diff of this file.
    pub(super) fn annotation_click(&mut self, p: Point<Pixels>, cx: &mut gpui::Context<Self>) -> bool {
        let Some(line) = self.annotation_at(p) else { return false };
        let Some(hash) = self.annotation.as_ref().and_then(|a| a.lines.get(line)).and_then(|x| x.as_ref()).map(|x| x.hash.clone()) else { return false };
        cx.emit(EditorEvent::ShowCommit(hash));
        true
    }

    /// Mouse move: remember the hovered annotation for the tooltip.
    pub(super) fn annotation_hover(&mut self, p: Point<Pixels>, cx: &mut gpui::Context<Self>) {
        let hit = self.annotation_at(p);
        if hit != self.ann_hover.map(|h| h.0) {
            self.ann_hover = hit.map(|l| (l, p));
            cx.notify();
        }
    }

    pub(super) fn render_annotation_tip(&self) -> Option<gpui::AnyElement> {
        let (line, at) = self.ann_hover?;
        let l = self.layout.as_ref()?;
        let a = self.annotation.as_ref()?.lines.get(line)?.as_ref()?;
        let rel = at - l.bounds.origin + point(px(12.), px(line_height()));
        Some(
            gpui::div()
                .absolute()
                .left(rel.x)
                .top(rel.y)
                .max_w(ui_px(520.))
                .px(px(10.))
                .py(px(6.))
                .rounded(px(6.))
                .border_1()
                .border_color(Theme::divider())
                .bg(Theme::panel())
                .shadow_md()
                .text_size(ui_px(12.5))
                .text_color(Theme::text())
                .child(gpui::div().text_color(Theme::text_muted()).child(format!("{}  {} <{}>  {}", &a.hash[..a.hash.len().min(8)], a.author, a.email, date(a.time))))
                .child(gpui::div().pt(px(4.)).child(a.summary.clone()))
                .child(gpui::div().pt(px(4.)).text_size(ui_px(11.)).text_color(Theme::text_dim()).child("Click to show the diff of this commit"))
                .into_any_element(),
        )
    }

    /// The commit of an annotated line.
    pub fn annotation_hash(&self, line: usize) -> Option<String> {
        Some(self.annotation.as_ref()?.lines.get(line)?.as_ref()?.hash.clone())
    }

    /// A window point on the annotation of `line` (smokes: the right-click menu there).
    pub fn annotation_point(&self, line: usize) -> Option<Point<Pixels>> {
        let l = self.layout.as_ref()?;
        let y = l.bounds.top() + px((self.row_of(line) as f32 + 0.5) * line_height() - self.scroll_y);
        Some(point(l.bounds.left() + self.annotation_left() + px(10.), y))
    }

    /// Annotation of a buffer line (smoke tests): (short hash, label).
    pub fn annotation_line(&self, line: usize) -> Option<(String, String)> {
        let a = self.annotation.as_ref()?.lines.get(line)?.as_ref()?;
        Some((a.hash[..8.min(a.hash.len())].to_string(), a.label()))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn dates_and_ages() {
        assert_eq!(date(0), "1970-01-01");
        assert_eq!(date(1_700_000_000), "2023-11-14");
        assert_eq!(date(951_782_400), "2000-02-29");
        let bl = |h: &str, t: i64| brisk_git::BlameLine { hash: h.repeat(40), author: "Alice".into(), email: String::new(), time: t, summary: String::new() };
        let a = Annotation::from_blame(vec![bl("a", 100), bl("b", 300), bl("0", 0), bl("c", 200)]);
        let ages: Vec<Option<f32>> = a.lines.iter().map(|l| l.as_ref().map(|l| l.age)).collect();
        assert_eq!(ages, vec![Some(1.0), Some(0.0), None, Some(0.5)]);
        assert_eq!(a.lines[0].as_ref().unwrap().label(), "1970-01-01 Alice");
    }
}
