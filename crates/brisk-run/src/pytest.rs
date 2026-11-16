//! pytest support: the reporting plugin, the live test tree PyCharm shows in the Run window,
//! and helpers for context configurations and "Rerun Failed Tests".

use crate::Command;
use serde::Deserialize;
use std::collections::{HashMap, HashSet};
use std::io::{Read, Seek, SeekFrom};
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU64, Ordering};

pub const PLUGIN_MODULE: &str = "brisk_pytest";
const PLUGIN_SRC: &str = include_str!("brisk_pytest.py");

/// pytest's default discovery pattern: `test_*.py` or `*_test.py`.
pub fn is_test_file(p: &Path) -> bool {
    let Some(n) = p.file_name().and_then(|n| n.to_str()) else { return false };
    n.ends_with(".py") && (n.starts_with("test_") || n.ends_with("_test.py"))
}

/// Whether a virtualenv interpreter has `package` installed; `None` when it can't be told
/// without starting Python (system or conda interpreters).
pub fn has_package(python: &Path, package: &str) -> Option<bool> {
    let venv = crate::Interpreter::venv_dir_of(python)?;
    let win = venv.join("Lib").join("site-packages");
    if win.is_dir() {
        return Some(win.join(package).is_dir());
    }
    let lib = std::fs::read_dir(venv.join("lib")).ok()?;
    let found = lib.flatten().any(|d| d.path().join("site-packages").join(package).is_dir());
    Some(found)
}

fn plugin_dir() -> std::io::Result<PathBuf> {
    let dir = std::env::temp_dir().join("brisk-pytest");
    std::fs::create_dir_all(&dir)?;
    let f = dir.join(format!("{PLUGIN_MODULE}.py"));
    if std::fs::read_to_string(&f).ok().as_deref() != Some(PLUGIN_SRC) {
        std::fs::write(&f, PLUGIN_SRC)?;
    }
    Ok(dir)
}

/// Load the reporting plugin into a pytest command for one run. Returns the events file the
/// plugin writes to; read it with [`TestFeed`].
pub fn attach(cmd: &mut Command) -> std::io::Result<PathBuf> {
    static N: AtomicU64 = AtomicU64::new(0);
    let dir = plugin_dir()?;
    let events = dir.join(format!("events-{}-{}.jsonl", std::process::id(), N.fetch_add(1, Ordering::Relaxed)));
    std::fs::write(&events, "")?;
    let sep = if cfg!(windows) { ";" } else { ":" };
    let pp = match cmd.env.get("PYTHONPATH").cloned().or_else(|| std::env::var("PYTHONPATH").ok()) {
        Some(old) if !old.is_empty() => format!("{}{sep}{old}", dir.display()),
        _ => dir.display().to_string(),
    };
    cmd.env.insert("PYTHONPATH".into(), pp);
    let plugins = match std::env::var("PYTEST_PLUGINS") {
        Ok(p) if !p.is_empty() => format!("{p},{PLUGIN_MODULE}"),
        _ => PLUGIN_MODULE.to_string(),
    };
    cmd.env.insert("PYTEST_PLUGINS".into(), plugins);
    cmd.env.insert("BRISK_TEST_EVENTS".into(), events.display().to_string());
    Ok(events)
}

/// Split a node id on `::`, but not inside a parametrize id (`test_x[a::b]`).
pub fn split_nodeid(id: &str) -> Vec<&str> {
    let mut parts = Vec::new();
    let (mut depth, mut start) = (0i32, 0);
    let b = id.as_bytes();
    let mut i = 0;
    while i < b.len() {
        match b[i] {
            b'[' => depth += 1,
            b']' => depth -= 1,
            b':' if depth == 0 && b.get(i + 1) == Some(&b':') => {
                parts.push(&id[start..i]);
                i += 2;
                start = i;
                continue;
            }
            _ => {}
        }
        i += 1;
    }
    parts.push(&id[start..]);
    parts
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum TestStatus {
    Pending,
    Running,
    Passed,
    Failed,
    Error,
    Skipped,
}

#[derive(Clone, Debug)]
pub struct TestNode {
    /// Display name: module stem, class or test (with its parametrize id).
    pub name: String,
    /// pytest node id of this level, e.g. `tests/test_x.py::TestA`.
    pub id: String,
    pub depth: usize,
    pub parent: Option<usize>,
    pub children: Vec<usize>,
    /// A collected test item (leaf), as opposed to a module or class.
    pub is_test: bool,
    pub status: TestStatus,
    /// Seconds.
    pub duration: f64,
    pub file: Option<PathBuf>,
    /// 1-based.
    pub line: Option<usize>,
    /// Assertion / error report or skip reason.
    pub message: String,
    /// Captured stdout / stderr / log sections.
    pub output: String,
}

#[derive(Default, Clone, Copy, Debug, PartialEq, Eq)]
pub struct Counts {
    pub total: usize,
    pub passed: usize,
    pub failed: usize,
    pub skipped: usize,
    pub pending: usize,
}

#[derive(Clone, Debug)]
pub struct TestTree {
    pub nodes: Vec<TestNode>,
    index: HashMap<String, usize>,
    pub rootdir: Option<PathBuf>,
    /// pytest's exit status once the session finished.
    pub done: Option<i32>,
}

impl Default for TestTree {
    fn default() -> Self {
        Self::new()
    }
}

#[derive(Deserialize)]
struct Item {
    id: String,
    #[serde(default)]
    file: String,
    line: Option<usize>,
}

#[derive(Deserialize)]
#[serde(tag = "event", rename_all = "lowercase")]
enum Event {
    Session { rootdir: String },
    Collected { tests: Vec<Item> },
    Collecterror { id: String, message: String },
    Start { id: String },
    Finish { id: String, outcome: String, duration: f64, message: String, output: String },
    Done { exitstatus: i32 },
}

impl TestTree {
    pub fn new() -> Self {
        let root = TestNode {
            name: "Test Results".into(),
            id: String::new(),
            depth: 0,
            parent: None,
            children: Vec::new(),
            is_test: false,
            status: TestStatus::Pending,
            duration: 0.0,
            file: None,
            line: None,
            message: String::new(),
            output: String::new(),
        };
        TestTree { nodes: vec![root], index: HashMap::new(), rootdir: None, done: None }
    }

    fn file_for(&self, module: &str) -> Option<PathBuf> {
        self.rootdir.as_ref().map(|r| r.join(module))
    }

    /// The node for `id`, creating it and its parents (module, classes) as needed.
    fn ensure(&mut self, id: &str) -> usize {
        if id.is_empty() {
            return 0;
        }
        if let Some(&i) = self.index.get(id) {
            return i;
        }
        let parts = split_nodeid(id);
        let parent = if parts.len() > 1 { self.ensure(&id[..id.len() - parts[parts.len() - 1].len() - 2]) } else { 0 };
        let last = parts[parts.len() - 1];
        let name = if parts.len() == 1 {
            // Module: PyCharm shows its name without the directory and extension.
            Path::new(last).file_stem().map(|s| s.to_string_lossy().into_owned()).unwrap_or_else(|| last.to_string())
        } else {
            last.to_string()
        };
        let file = self.file_for(parts[0]);
        let i = self.nodes.len();
        self.nodes.push(TestNode {
            name,
            id: id.to_string(),
            depth: self.nodes[parent].depth + 1,
            parent: Some(parent),
            children: Vec::new(),
            is_test: false,
            status: TestStatus::Pending,
            duration: 0.0,
            file,
            line: None,
            message: String::new(),
            output: String::new(),
        });
        self.nodes[parent].children.push(i);
        self.index.insert(id.to_string(), i);
        i
    }

    /// Apply one line of the plugin's output. Returns false for lines it doesn't understand.
    pub fn apply(&mut self, line: &str) -> bool {
        let Ok(ev) = serde_json::from_str::<Event>(line) else { return false };
        match ev {
            Event::Session { rootdir } => self.rootdir = Some(PathBuf::from(rootdir)),
            Event::Collected { tests } => {
                for t in tests {
                    let i = self.ensure(&t.id);
                    let n = &mut self.nodes[i];
                    n.is_test = true;
                    if !t.file.is_empty() {
                        n.file = Some(PathBuf::from(t.file));
                    }
                    n.line = t.line;
                }
            }
            Event::Collecterror { id, message } => {
                let i = self.ensure(&id);
                let n = &mut self.nodes[i];
                n.is_test = n.children.is_empty();
                n.status = TestStatus::Error;
                n.message = message;
            }
            Event::Start { id } => {
                let i = self.ensure(&id);
                self.nodes[i].is_test = true;
                self.nodes[i].status = TestStatus::Running;
            }
            Event::Finish { id, outcome, duration, message, output } => {
                let i = self.ensure(&id);
                let n = &mut self.nodes[i];
                n.is_test = true;
                n.status = match outcome.as_str() {
                    "passed" => TestStatus::Passed,
                    "failed" => TestStatus::Failed,
                    "skipped" => TestStatus::Skipped,
                    _ => TestStatus::Error,
                };
                n.duration = duration;
                n.message = message;
                n.output = output;
            }
            Event::Done { exitstatus } => self.done = Some(exitstatus),
        }
        self.aggregate();
        true
    }

    /// Parent status and duration from the children. Children always come after parents.
    fn aggregate(&mut self) {
        for i in (0..self.nodes.len()).rev() {
            if self.nodes[i].is_test || self.nodes[i].children.is_empty() {
                continue;
            }
            let mut seen = HashSet::new();
            let mut dur = 0.0;
            for &c in &self.nodes[i].children {
                seen.insert(self.nodes[c].status);
                dur += self.nodes[c].duration;
            }
            let own_error = self.nodes[i].status == TestStatus::Error && !self.nodes[i].message.is_empty();
            let has = |s| seen.contains(&s);
            self.nodes[i].status = if own_error || has(TestStatus::Error) && !has(TestStatus::Failed) {
                TestStatus::Error
            } else if has(TestStatus::Failed) || has(TestStatus::Error) {
                TestStatus::Failed
            } else if has(TestStatus::Running) {
                TestStatus::Running
            } else if has(TestStatus::Pending) {
                if has(TestStatus::Passed) || has(TestStatus::Skipped) {
                    TestStatus::Running
                } else {
                    TestStatus::Pending
                }
            } else if has(TestStatus::Passed) {
                TestStatus::Passed
            } else {
                TestStatus::Skipped
            };
            self.nodes[i].duration = dur;
        }
    }

    pub fn counts(&self) -> Counts {
        let mut c = Counts::default();
        for n in self.nodes.iter().filter(|n| n.is_test) {
            c.total += 1;
            match n.status {
                TestStatus::Passed => c.passed += 1,
                TestStatus::Failed | TestStatus::Error => c.failed += 1,
                TestStatus::Skipped => c.skipped += 1,
                TestStatus::Pending | TestStatus::Running => c.pending += 1,
            }
        }
        c
    }

    pub fn is_empty(&self) -> bool {
        self.nodes.len() == 1
    }

    /// PyCharm's status line: "Tests failed: 1, passed: 5 of 6 tests – 120 ms".
    /// `finished` is whether the process has exited.
    pub fn summary(&self, finished: bool) -> String {
        let c = self.counts();
        let dur = format_duration(self.nodes[0].duration);
        if c.total == 0 {
            return if finished { "No tests were found".into() } else { "Collecting tests…".into() };
        }
        // The first count goes right after the heading: "Tests failed: 1, passed: 5".
        let mut parts: Vec<(&str, usize)> = Vec::new();
        if c.failed > 0 {
            parts.push(("failed", c.failed));
        }
        if c.passed > 0 || c.failed == 0 && c.skipped == 0 {
            parts.push(("passed", c.passed));
        }
        if c.skipped > 0 {
            parts.push(("ignored", c.skipped));
        }
        let stopped = finished && (self.done.is_none() || c.pending > 0);
        let head = if !finished || stopped {
            String::from(if finished { "Tests stopped" } else { "Running" })
        } else {
            format!("Tests {}", parts[0].0)
        };
        let body: Vec<String> = parts
            .iter()
            .enumerate()
            .map(|(k, (name, n))| if k == 0 && finished && !stopped { n.to_string() } else { format!("{name}: {n}") })
            .collect();
        format!("{head}: {} of {} test{} – {dur}", body.join(", "), c.total, if c.total == 1 { "" } else { "s" })
    }

    /// Visible rows in tree order, skipping children of collapsed nodes.
    pub fn rows(&self, collapsed: &HashSet<usize>) -> Vec<usize> {
        let mut out = Vec::new();
        let mut stack = vec![0];
        while let Some(i) = stack.pop() {
            out.push(i);
            if !collapsed.contains(&i) {
                stack.extend(self.nodes[i].children.iter().rev());
            }
        }
        out
    }

    /// First failed test in tree order (PyCharm selects it when the run finishes).
    pub fn first_failed(&self) -> Option<usize> {
        self.rows(&HashSet::new()).into_iter().find(|&i| self.nodes[i].is_test && matches!(self.nodes[i].status, TestStatus::Failed | TestStatus::Error))
    }

    /// Absolute pytest targets (`C:\p\tests\test_x.py::TestA::test_b`) of failed tests and
    /// broken modules, for "Rerun Failed Tests".
    pub fn failed_targets(&self) -> Vec<String> {
        let mut v = Vec::new();
        for n in self.nodes.iter().filter(|n| n.is_test && matches!(n.status, TestStatus::Failed | TestStatus::Error)) {
            let parts = split_nodeid(&n.id);
            let file = n.file.clone().or_else(|| self.file_for(parts[0]));
            let Some(file) = file else { continue };
            let mut t = file.display().to_string();
            for p in &parts[1..] {
                t.push_str("::");
                t.push_str(p);
            }
            v.push(t);
        }
        v
    }

    /// What PyCharm prints for a selected test: captured output, then the failure.
    pub fn details(&self, i: usize) -> String {
        let n = &self.nodes[i];
        let mut s = String::new();
        if !n.output.is_empty() {
            s.push_str(&n.output);
            if !s.ends_with('\n') {
                s.push('\n');
            }
        }
        if !n.message.is_empty() {
            s.push_str(&n.message);
            s.push('\n');
        }
        if s.is_empty() {
            s = match n.status {
                TestStatus::Passed => format!("PASSED in {}", format_duration(n.duration)),
                TestStatus::Pending => "Not run".into(),
                TestStatus::Running => "Running…".into(),
                TestStatus::Skipped => "SKIPPED".into(),
                _ => String::new(),
            };
        }
        s
    }
}

/// "120 ms", "2 sec 340 ms", like PyCharm's test runner.
pub fn format_duration(secs: f64) -> String {
    let ms = (secs * 1000.0).round() as u64;
    if ms < 1000 {
        format!("{ms} ms")
    } else if ms < 60_000 {
        format!("{} sec {} ms", ms / 1000, ms % 1000)
    } else {
        format!("{} min {} sec", ms / 60_000, (ms / 1000) % 60)
    }
}

/// Tails the plugin's events file into a [`TestTree`].
pub struct TestFeed {
    pub tree: TestTree,
    path: PathBuf,
    offset: u64,
    partial: Vec<u8>,
}

impl TestFeed {
    pub fn new(path: PathBuf) -> Self {
        TestFeed { tree: TestTree::new(), path, offset: 0, partial: Vec::new() }
    }

    /// Read whatever was appended since the last call. Returns whether the tree changed.
    pub fn poll(&mut self) -> bool {
        let Ok(mut f) = std::fs::File::open(&self.path) else { return false };
        if f.seek(SeekFrom::Start(self.offset)).is_err() {
            return false;
        }
        let mut buf = Vec::new();
        let Ok(n) = f.read_to_end(&mut buf) else { return false };
        if n == 0 {
            return false;
        }
        self.offset += n as u64;
        self.partial.extend_from_slice(&buf);
        let Some(end) = self.partial.iter().rposition(|&b| b == b'\n') else { return false };
        let lines: Vec<u8> = self.partial.drain(..=end).collect();
        let mut changed = false;
        for l in String::from_utf8_lossy(&lines).lines() {
            changed |= self.tree.apply(l);
        }
        changed
    }
}

impl Drop for TestFeed {
    fn drop(&mut self) {
        let _ = std::fs::remove_file(&self.path);
    }
}
