//! How problems look in the text, as in PyCharm: wavy underlines for errors and warnings that
//! cover only the code (from the first non-blank character to the end of the text on each line,
//! never the indentation of a continuation line), drawn on whole device pixels so they stay crisp
//! at 100 / 125 / 150 %; unused code greyed out; deprecated names struck through.

use brisk_lint::Diagnostic;
use gpui::{point, px, Hsla, Pixels, StrikethroughStyle, TextRun, UnderlineStyle, Window};
use std::ops::Range;

/// ty reports uses of deprecated functions and classes under this rule; PyCharm strikes them
/// through instead of underlining.
pub(super) fn is_deprecated(d: &Diagnostic) -> bool {
    d.code == "deprecated"
}

/// The columns a wave covers on one line of a problem. `from` / `to` are the problem's columns
/// on its first / last line (None on lines it spans whole). The wave starts no earlier than the
/// first non-blank character and ends no later than the end of the text; None when nothing is
/// left (a blank line inside the problem).
pub(super) fn wave_cols(line: &str, from: Option<usize>, to: Option<usize>) -> Option<(usize, usize)> {
    let chars: Vec<char> = line.trim_end_matches(['\n', '\r']).chars().collect();
    let first = chars.iter().position(|c| !c.is_whitespace()).unwrap_or(chars.len());
    let end = chars.iter().rposition(|c| !c.is_whitespace()).map_or(0, |i| i + 1);
    // A problem on one point (or past the end of the text) keeps a short wave where it is.
    if let (Some(a), Some(b)) = (from, to) {
        if b <= a || a >= end {
            return Some((a, a + 1));
        }
    }
    let a = from.map_or(first, |a| a.max(first));
    let b = to.map_or(end, |b| b.min(end));
    (b > a).then_some((a, b))
}

/// A wavy underline from `x0` to `x1` with its top at `y`, snapped to device pixels: the line is
/// one device pixel thick (two from 175 %), starts and ends on whole pixels.
pub(super) fn paint_wave(window: &mut Window, x0: Pixels, x1: Pixels, y: Pixels, color: Hsla) {
    let s = window.scale_factor();
    let snap = |v: Pixels| px((f32::from(v) * s).round() / s);
    let thickness = (s.round().max(1.0)) / s;
    let (a, b) = (snap(x0), snap(x1));
    if b <= a {
        return;
    }
    window.paint_underline(point(a, snap(y)), b - a, &UnderlineStyle { color: Some(color), thickness: px(thickness), wavy: true });
}

/// Strike a byte range of a line's text runs through (deprecated names).
pub(super) fn strike(runs: Vec<TextRun>, range: Range<usize>) -> Vec<TextRun> {
    let mut out = Vec::with_capacity(runs.len() + 2);
    let mut at = 0;
    for r in runs {
        let (s, e) = (at, at + r.len);
        at = e;
        let (a, b) = (range.start.clamp(s, e), range.end.clamp(s, e));
        if a >= b {
            out.push(r);
            continue;
        }
        if a > s {
            out.push(TextRun { len: a - s, ..r.clone() });
        }
        out.push(TextRun { len: b - a, strikethrough: Some(StrikethroughStyle { thickness: px(1.), color: Some(r.color) }), ..r.clone() });
        if e > b {
            out.push(TextRun { len: e - b, ..r });
        }
    }
    out
}

/// Where the wave's top goes in a row: just under the text's baseline area.
pub(super) fn wave_top(row_top: Pixels, line_h: Pixels) -> Pixels {
    row_top + line_h - px(3.)
}

#[cfg(test)]
mod tests {
    use super::wave_cols;

    #[test]
    fn waves_cover_code_not_indentation() {
        // A continuation line inside a multi-line problem: from the first character on.
        assert_eq!(wave_cols("                'type',", None, None), Some((16, 23)));
        // The first line: from the problem's start; the last: up to its end.
        assert_eq!(wave_cols("    x = ('name',", Some(8), None), Some((8, 16)));
        assert_eq!(wave_cols("            )", None, Some(13)), Some((12, 13)));
        // Trailing spaces and a blank line are not underlined.
        assert_eq!(wave_cols("    y = 1   ", None, None), Some((4, 9)));
        assert_eq!(wave_cols("      ", None, None), None);
        // A point problem keeps a one-column wave, even past the end of the text.
        assert_eq!(wave_cols("x = (", Some(5), Some(5)), Some((5, 6)));
        // A problem that starts inside the indentation is moved to the code.
        assert_eq!(wave_cols("        return", Some(0), Some(14)), Some((8, 14)));
    }
}

impl super::Editor {
    /// Smoke: the waves as (1-based line, first column, past the last column), per problem.
    pub fn waves_for_smoke(&self) -> Vec<(String, Vec<(usize, usize, usize)>)> {
        self.diagnostics
            .iter()
            .filter(|d| d.severity != brisk_lint::Severity::Unused && !is_deprecated(d))
            .map(|d| {
                let r = self.diag_range(d);
                let (l1, c1) = self.buffer.pos_to_line_col(r.start);
                let (l2, c2) = self.buffer.pos_to_line_col(r.end);
                let rows = (l1..=l2).filter_map(|l| wave_cols(&self.buffer.line(l), (l == l1).then_some(c1), (l == l2).then_some(c2)).map(|(a, b)| (l + 1, a, b))).collect();
                (d.code.clone(), rows)
            })
            .collect()
    }
}
