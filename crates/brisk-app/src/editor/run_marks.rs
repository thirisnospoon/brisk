//! Run icons in the gutter, like PyCharm's: `if __name__ == "__main__":`, pytest tests and
//! `Test*` classes in test files, and `def main` when the file has no `__main__` block.
//! A click asks the workspace for the Run / Debug menu; pytest results color the icon.

use super::Editor;
use brisk_run::TestStatus;
use gpui::{px, Pixels, Point};
use std::collections::HashMap;

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum RunTarget {
    /// The whole file as a script.
    Script,
    /// A pytest node inside the file: `["TestA", "test_b"]`.
    Test(Vec<String>),
    /// A script of `package.json`.
    Npm(String),
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct RunMark {
    pub line: usize,
    pub target: RunTarget,
}

/// Width of the icon column right of the line numbers (Python editors only).
pub const SLOT: f32 = 16.;

fn ident(s: &str) -> &str {
    let end = s.find(|c: char| !(c.is_alphanumeric() || c == '_')).unwrap_or(s.len());
    &s[..end]
}

fn is_main_guard(line: &str) -> bool {
    let compact: String = line.chars().filter(|c| !c.is_whitespace()).collect();
    compact.starts_with("if__name__==\"__main__\":")
        || compact.starts_with("if__name__=='__main__':")
        || compact.starts_with("if\"__main__\"==__name__:")
        || compact.starts_with("if'__main__'==__name__:")
}

/// The run targets of a Python file's text.
pub fn run_marks(text: &str, test_file: bool) -> Vec<RunMark> {
    let mut out = Vec::new();
    let mut main_def = None;
    let mut has_guard = false;
    // The top-level class the following indented lines belong to: (name, is a test class).
    let mut class: Option<(String, bool)> = None;
    for (n, line) in text.lines().enumerate() {
        let t = line.trim_start();
        if t.is_empty() || t.starts_with('#') {
            continue;
        }
        let indent = line.len() - t.len();
        let def = t.strip_prefix("async def ").or_else(|| t.strip_prefix("def ")).map(|r| ident(r.trim_start()));
        if indent == 0 {
            class = None;
            if is_main_guard(t) {
                has_guard = true;
                out.push(RunMark { line: n, target: RunTarget::Script });
                continue;
            }
            if let Some(rest) = t.strip_prefix("class ") {
                let name = ident(rest.trim_start()).to_string();
                let test = test_file && name.starts_with("Test");
                if test {
                    out.push(RunMark { line: n, target: RunTarget::Test(vec![name.clone()]) });
                }
                class = Some((name, test));
                continue;
            }
            match def {
                Some(name) if test_file && name.starts_with("test") => out.push(RunMark { line: n, target: RunTarget::Test(vec![name.to_string()]) }),
                Some("main") if main_def.is_none() => main_def = Some(n),
                _ => {}
            }
        } else if let (Some((cls, true)), Some(name)) = (&class, def) {
            if name.starts_with("test") {
                out.push(RunMark { line: n, target: RunTarget::Test(vec![cls.clone(), name.to_string()]) });
            }
        }
    }
    if let (Some(n), false) = (main_def, has_guard) {
        out.push(RunMark { line: n, target: RunTarget::Script });
        out.sort_by_key(|m| m.line);
    }
    out
}

/// The marks of a file: `package.json` scripts, else Python's.
fn marks_for(text: &str, test_file: bool, package_json: bool) -> Vec<RunMark> {
    if package_json {
        brisk_run::node::scripts(text).into_iter().map(|s| RunMark { line: s.line, target: RunTarget::Npm(s.name) }).collect()
    } else {
        run_marks(text, test_file)
    }
}

/// The node key of a test result: `TestA::test_b` (parametrize ids dropped).
pub fn scope_key(scope: &[String]) -> String {
    scope.join("::")
}

impl Editor {
    pub fn run_slot(&self) -> Pixels {
        if self.show_gutter && !self.is_field && self.path.is_some() && (self.is_python() || self.is_package_json()) {
            px(SLOT)
        } else {
            px(0.)
        }
    }

    fn is_package_json(&self) -> bool {
        self.path.as_deref().and_then(|p| p.file_name()).is_some_and(|n| n == "package.json")
    }

    /// The marks of the current text (cached by buffer version).
    pub fn run_marks(&mut self) -> Vec<RunMark> {
        if self.run_slot() == px(0.) {
            return Vec::new();
        }
        let v = self.buffer.version();
        if self.run_cache.0 != v {
            let test = self.path.as_deref().is_some_and(brisk_run::is_test_file);
            self.text();
            self.run_cache = (v, marks_for(&self.text_cache.1, test, self.is_package_json()));
        }
        self.run_cache.1.clone()
    }

    /// Prepaint: keep the marks current. A long file recomputes them on a background thread
    /// once typing pauses; meanwhile the old marks stay (a click still recomputes them).
    pub(super) fn refresh_run_marks(&mut self, cx: &mut gpui::Context<Self>) {
        let v = self.buffer.version();
        if self.run_slot() == px(0.) || self.run_cache.0 == v {
            return;
        }
        // A long file: computed in the background, the first time too (opening a 30k-line
        // file must not wait for them; the gutter slot has its width already).
        if self.buffer.len_lines() < super::big::LAZY_FOLD_LINES {
            self.run_marks();
            return;
        }
        if self.big.marks_for == Some(v) {
            return;
        }
        self.big.marks_for = Some(v);
        // The first marks of a file come at once; after an edit, once typing pauses.
        let debounce = self.run_cache.0 != u64::MAX;
        cx.spawn(async move |this, cx| {
            if debounce {
                cx.background_executor().timer(super::big::FOLD_DEBOUNCE).await;
            }
            let job = this
                .update(cx, |e, cx| {
                    if e.buffer.version() != v {
                        return None;
                    }
                    let test = e.path.as_deref().is_some_and(brisk_run::is_test_file);
                    let npm = e.is_package_json();
                    let text = e.text().to_string();
                    Some(cx.background_executor().spawn(async move { marks_for(&text, test, npm) }))
                })
                .ok()
                .flatten();
            let Some(job) = job else { return };
            let marks = job.await;
            let _ = this.update(cx, |e, cx| {
                if e.buffer.version() == v {
                    e.run_cache = (v, marks);
                    cx.notify();
                }
            });
        })
        .detach();
    }

    /// The marks as last computed (paint must not mutate); a long file keeps showing the
    /// previous ones until the recompute lands.
    pub(super) fn run_marks_now(&self) -> &[RunMark] {
        if self.run_cache.0 == self.buffer.version() || (self.run_cache.0 != u64::MAX && self.buffer.len_lines() >= super::big::LAZY_FOLD_LINES) {
            &self.run_cache.1
        } else {
            &[]
        }
    }

    /// pytest results for this file, by node key (`TestA::test_b`).
    pub fn set_test_results(&mut self, results: HashMap<String, TestStatus>) {
        self.test_results = results;
    }

    pub fn test_status(&self, target: &RunTarget) -> Option<TestStatus> {
        match target {
            RunTarget::Test(scope) => self.test_results.get(&scope_key(scope)).copied(),
            RunTarget::Script | RunTarget::Npm(_) => None,
        }
    }

    /// Window position (logical pixels) of the icon of `line`, if it is on screen.
    #[cfg(feature = "smoke")]
    pub fn run_icon_point(&self, line: usize) -> Option<(f32, f32)> {
        let l = self.layout.as_ref()?;
        if !self.run_marks_now().iter().any(|m| m.line == line) {
            return None;
        }
        let lh = super::line_height();
        let y = l.bounds.top() + px(self.row_of(line) as f32 * lh - self.scroll_y + lh / 2.);
        let x = l.text_left - px(31.) - self.run_slot() / 2.;
        Some((f32::from(x), f32::from(y)))
    }

    /// A click on a run icon: its line.
    pub(super) fn run_mark_at(&self, p: Point<Pixels>) -> Option<usize> {
        let l = self.layout.as_ref()?;
        let slot = self.run_slot();
        if slot == px(0.) {
            return None;
        }
        let right = l.text_left - px(31.);
        if p.x < right - slot - px(2.) || p.x >= right {
            return None;
        }
        let y = f32::from(p.y - l.bounds.top()) + self.scroll_y;
        let line = self.line_of_row((y / super::line_height()).floor().max(0.0) as usize);
        self.run_marks_now().iter().any(|m| m.line == line).then_some(line)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn lines(m: &[RunMark]) -> Vec<(usize, RunTarget)> {
        m.iter().map(|m| (m.line + 1, m.target.clone())).collect()
    }

    #[test]
    fn scripts() {
        let src = "import sys\n\ndef main():\n    pass\n\nif __name__ == '__main__':\n    main()\n";
        assert_eq!(lines(&run_marks(src, false)), vec![(6, RunTarget::Script)]);
        // No guard: def main gets the icon.
        assert_eq!(lines(&run_marks("def main():\n    pass\n", false)), vec![(1, RunTarget::Script)]);
        assert_eq!(lines(&run_marks("if __name__==\"__main__\" :\n    run()\n", false)), vec![(1, RunTarget::Script)]);
        // Tests are only tests in test files; nested `def main` is not the entry point.
        assert!(run_marks("def test_x():\n    pass\nclass A:\n    def main(self):\n        pass\n", false).is_empty());
    }

    #[test]
    fn tests_and_classes() {
        let src = "import pytest\n\ndef test_add():\n    assert 1\n\nasync def test_io():\n    pass\n\nclass TestMath:\n    def helper(self):\n        pass\n\n    def test_mul(self):\n        pass\n\n    @pytest.mark.slow\n    def test_div(self):\n        pass\n\nclass Helper:\n    def test_not_collected(self):\n        pass\n\ndef util():\n    def test_inner():\n        pass\n";
        let t = |v: &[&str]| RunTarget::Test(v.iter().map(|s| s.to_string()).collect());
        assert_eq!(
            lines(&run_marks(src, true)),
            vec![(3, t(&["test_add"])), (6, t(&["test_io"])), (9, t(&["TestMath"])), (13, t(&["TestMath", "test_mul"])), (17, t(&["TestMath", "test_div"]))]
        );
    }
}
