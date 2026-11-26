//! Project model: lazily-loaded file tree (Project tool window) and a flat file list
//! for Go to File / Search Everywhere with fuzzy matching.

use nucleo_matcher::pattern::{CaseMatching, Normalization, Pattern};
use nucleo_matcher::{Config, Matcher, Utf32Str};
use std::path::{Path, PathBuf};

pub mod editorconfig;
pub mod folders;
mod structure;
pub use structure::{rel_path, Mark, Structure};

/// Names hidden from the tree (PyCharm's default "Ignored files and folders", plus the IDE
/// folders: Brisk's own `.brisk` and PyCharm's `.idea`).
const HIDDEN: &[&str] = &[".git", ".hg", ".svn", "__pycache__", ".DS_Store", ".mypy_cache", ".ruff_cache", ".pytest_cache", ".brisk", ".idea"];
const HIDDEN_EXT: &[&str] = &["pyc", "pyo"];
/// Shown but marked excluded and never indexed.
const EXCLUDED: &[&str] = &[".venv", "venv", "env", "node_modules", ".tox", "dist", "build", ".next", ".nuxt", "target"];

pub fn is_hidden(name: &str) -> bool {
    HIDDEN.contains(&name) || Path::new(name).extension().and_then(|e| e.to_str()).is_some_and(|e| HIDDEN_EXT.contains(&e))
}

pub fn is_excluded_dir(name: &str) -> bool {
    EXCLUDED.contains(&name)
}

pub type NodeId = usize;

#[derive(Debug, Clone)]
pub struct Node {
    pub name: String,
    pub path: PathBuf,
    pub is_dir: bool,
    pub excluded: bool,
    pub depth: usize,
    pub parent: Option<NodeId>,
    pub children: Option<Vec<NodeId>>, // None = not loaded yet
    pub expanded: bool,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Row {
    pub id: NodeId,
    pub depth: usize,
}

/// Tree that only reads a directory when it is expanded.
pub struct ProjectTree {
    pub nodes: Vec<Node>,
    /// Top-level nodes: the project (0), then extra roots such as "Scratches and Consoles".
    roots: Vec<NodeId>,
    rows: Vec<Row>,
    rows_dirty: bool,
    /// The user's folder marks for the project root.
    pub structure: Structure,
}

/// One folder entry: (name, path, is_dir).
pub type Entry = (String, PathBuf, bool);

/// A folder's visible entries, folders first, then case-insensitive name (as PyCharm shows
/// them). Plain filesystem work, safe to call off the UI thread.
pub fn read_entries(dir: &Path) -> Vec<Entry> {
    let mut entries: Vec<Entry> = std::fs::read_dir(dir)
        .map(|rd| {
            rd.filter_map(|e| e.ok())
                .filter_map(|e| {
                    let name = e.file_name().to_string_lossy().into_owned();
                    if is_hidden(&name) {
                        return None;
                    }
                    let is_dir = e.file_type().map(|t| t.is_dir()).unwrap_or(false);
                    Some((name, e.path(), is_dir))
                })
                .collect()
        })
        .unwrap_or_default();
    entries.sort_by_cached_key(|e| (!e.2, e.0.to_lowercase()));
    entries
}

impl ProjectTree {
    pub fn new(root: &Path) -> Self {
        let name = root.file_name().map(|n| n.to_string_lossy().into_owned()).unwrap_or_else(|| root.display().to_string());
        let mut t = Self {
            nodes: vec![Node { name, path: root.to_path_buf(), is_dir: true, excluded: false, depth: 0, parent: None, children: None, expanded: false }],
            roots: vec![0],
            rows: Vec::new(),
            rows_dirty: true,
            structure: Structure::load(root),
        };
        t.expand(0);
        t
    }

    pub fn root(&self) -> &Path {
        &self.nodes[0].path
    }

    /// A top-level folder shown after the project (collapsed), named `name`, listing `dir`.
    pub fn add_root(&mut self, name: &str, dir: &Path) -> NodeId {
        if let Some(&id) = self.roots.iter().find(|&&r| self.nodes[r].path == dir) {
            return id;
        }
        let id = self.nodes.len();
        self.nodes.push(Node {
            name: name.into(),
            path: dir.to_path_buf(),
            is_dir: true,
            excluded: false,
            depth: 0,
            parent: None,
            children: None,
            expanded: false,
        });
        self.roots.push(id);
        self.rows_dirty = true;
        id
    }

    /// A folder excluded by its name or by the user's mark.
    fn excluded_dir(&self, name: &str, path: &Path) -> bool {
        is_excluded_dir(name) || rel_path(self.root(), path).is_some_and(|r| self.structure.is_excluded(&r))
    }

    /// The folder's own mark, for its icon.
    pub fn mark(&self, id: NodeId) -> Option<Mark> {
        let n = self.nodes.get(id)?;
        if !n.is_dir {
            return None;
        }
        self.structure.mark_of(&rel_path(self.root(), &n.path)?)
    }

    /// New marks: the excluded flags of loaded folders follow them.
    pub fn set_structure(&mut self, s: Structure) {
        self.structure = s;
        // Parents come before their children in `nodes`.
        for id in 0..self.nodes.len() {
            if self.roots.contains(&id) {
                continue;
            }
            let parent_excluded = self.nodes[id].parent.is_some_and(|p| self.nodes[p].excluded);
            let n = &self.nodes[id];
            let excluded = parent_excluded || (n.is_dir && self.excluded_dir(&n.name.clone(), &n.path.clone()));
            self.nodes[id].excluded = excluded;
        }
        self.rows_dirty = true;
    }

    pub fn is_root(&self, id: NodeId) -> bool {
        self.roots.contains(&id)
    }

    /// The top-level node whose folder holds `path` (the deepest one), and `path` relative to it.
    fn root_of<'a>(&self, path: &'a Path) -> Option<(NodeId, &'a Path)> {
        self.roots.iter().filter_map(|&r| path.strip_prefix(&self.nodes[r].path).ok().map(|rel| (r, rel))).min_by_key(|(_, rel)| rel.components().count())
    }

    fn load_children(&mut self, id: NodeId) {
        if self.nodes[id].children.is_some() {
            return;
        }
        let dir = self.nodes[id].path.clone();
        let depth = self.nodes[id].depth + 1;
        let parent_excluded = self.nodes[id].excluded;
        let entries = read_entries(&dir);
        let mut ids = Vec::with_capacity(entries.len());
        for (name, path, is_dir) in entries {
            let excluded = parent_excluded || (is_dir && self.excluded_dir(&name, &path));
            ids.push(self.nodes.len());
            self.nodes.push(Node { name, path, is_dir, excluded, depth, parent: Some(id), children: None, expanded: false });
        }
        self.nodes[id].children = Some(ids);
    }

    pub fn expand(&mut self, id: NodeId) {
        if !self.nodes[id].is_dir {
            return;
        }
        self.load_children(id);
        self.nodes[id].expanded = true;
        // PyCharm auto-expands chains of single folders ("compact middle packages" is off by default,
        // but single-child chains are expanded on open).
        if let Some(ch) = &self.nodes[id].children {
            if ch.len() == 1 && self.nodes[ch[0]].is_dir && !self.nodes[ch[0]].excluded {
                let c = ch[0];
                self.expand(c);
            }
        }
        self.rows_dirty = true;
    }

    pub fn collapse(&mut self, id: NodeId) {
        self.nodes[id].expanded = false;
        self.rows_dirty = true;
    }

    pub fn toggle(&mut self, id: NodeId) {
        if self.nodes[id].expanded {
            self.collapse(id)
        } else {
            self.expand(id)
        }
    }

    /// Drop cached children so the next expand re-reads the directory.
    pub fn refresh(&mut self, id: NodeId) {
        let was = self.nodes[id].expanded;
        self.nodes[id].children = None;
        if was {
            self.expand(id);
        }
        self.rows_dirty = true;
    }

    /// The loaded node for a path, if its folder has been read.
    pub fn find(&self, path: &Path) -> Option<NodeId> {
        let (mut cur, rel) = self.root_of(path)?;
        for comp in rel.components() {
            let name = comp.as_os_str().to_string_lossy();
            cur = *self.nodes[cur].children.as_ref()?.iter().find(|&&c| self.nodes[c].name == name)?;
        }
        Some(cur)
    }

    /// Re-read a folder and its expanded subfolders after files changed in them, keeping
    /// expanded subfolders expanded.
    pub fn reload(&mut self, id: NodeId) {
        let entries = read_entries(&self.nodes[id].path);
        self.set_entries(id, entries);
        let open: Vec<NodeId> = self.nodes[id].children.iter().flatten().copied().filter(|&c| self.nodes[c].expanded).collect();
        for c in open {
            self.reload(c);
        }
    }

    /// Put a folder's fresh listing (from `read_entries`, which can run off the UI thread) in
    /// place, merging by name: an entry that was there keeps its node (so its expanded state,
    /// and the children of an expanded subfolder, stay), only new entries get nodes. A folder
    /// of 50k files that gains one file costs one node, not 50k. True if the set of names
    /// changed (an entry added, removed, or turned from a file into a folder); a file saved
    /// through a temporary file changes the folder's time but not its names.
    pub fn set_entries(&mut self, id: NodeId, entries: Vec<Entry>) -> bool {
        let depth = self.nodes[id].depth + 1;
        let parent_excluded = self.nodes[id].excluded;
        let mut old: std::collections::HashMap<String, NodeId> =
            self.nodes[id].children.take().unwrap_or_default().into_iter().map(|c| (std::mem::take(&mut self.nodes[c].name), c)).collect();
        let mut ids = Vec::with_capacity(entries.len());
        let mut changed = false;
        for (name, path, is_dir) in entries {
            match old.remove(&name) {
                Some(c) if self.nodes[c].is_dir == is_dir => {
                    let n = &mut self.nodes[c];
                    n.name = name;
                    // A collapsed subfolder is read again when it is expanded.
                    if is_dir && !n.expanded {
                        n.children = None;
                    }
                    ids.push(c);
                }
                gone => {
                    changed = true;
                    if let Some(c) = gone {
                        self.nodes[c].expanded = false;
                        self.nodes[c].children = None;
                    }
                    let excluded = parent_excluded || (is_dir && self.excluded_dir(&name, &path));
                    ids.push(self.nodes.len());
                    self.nodes.push(Node { name, path, is_dir, excluded, depth, parent: Some(id), children: None, expanded: false });
                }
            }
        }
        // Removed entries: their nodes stay unreachable (ids are not reused).
        for c in old.into_values() {
            changed = true;
            self.nodes[c].expanded = false;
            self.nodes[c].children = None;
        }
        self.nodes[id].children = Some(ids);
        self.rows_dirty = true;
        changed
    }

    /// Expanded, loaded folders reachable from the root (the ones to watch for changes).
    pub fn expanded_dirs(&self) -> Vec<NodeId> {
        let mut out = Vec::new();
        let mut stack = self.roots.clone();
        while let Some(id) = stack.pop() {
            let n = &self.nodes[id];
            if let (true, true, Some(ch)) = (n.is_dir, n.expanded, &n.children) {
                out.push(id);
                stack.extend(ch.iter().copied().filter(|&c| self.nodes[c].is_dir));
            }
        }
        out
    }

    /// Flattened visible rows; the UI renders only the slice in view.
    pub fn rows(&mut self) -> &[Row] {
        if self.rows_dirty {
            let mut rows = Vec::new();
            let mut stack: Vec<NodeId> = self.roots.iter().rev().copied().collect();
            while let Some(id) = stack.pop() {
                let n = &self.nodes[id];
                rows.push(Row { id, depth: n.depth });
                if n.expanded {
                    if let Some(ch) = &n.children {
                        stack.extend(ch.iter().rev());
                    }
                }
            }
            self.rows = rows;
            self.rows_dirty = false;
        }
        &self.rows
    }

    /// Expand parents and return the node for a path (Alt+F1 "Select in Project View").
    pub fn reveal(&mut self, path: &Path) -> Option<NodeId> {
        let (mut cur, rel) = self.root_of(path)?;
        let rel = rel.to_path_buf();
        for comp in rel.components() {
            self.expand(cur);
            let name = comp.as_os_str().to_string_lossy();
            cur = *self.nodes[cur].children.as_ref()?.iter().find(|&&c| self.nodes[c].name == name)?;
        }
        self.rows_dirty = true;
        Some(cur)
    }
}

/// The last [`FileList::search_narrowing`] on this thread: its list, query and hits.
struct LastSearch {
    list: std::sync::Weak<FileList>,
    query: String,
    hits: Vec<u32>,
}

thread_local! {
    static MATCHER: std::cell::RefCell<Option<Matcher>> = const { std::cell::RefCell::new(None) };
    static LAST_SEARCH: std::cell::RefCell<Option<LastSearch>> = const { std::cell::RefCell::new(None) };
}

/// Flat list of project files for Go to File. Built off the UI thread.
#[derive(Default, Clone)]
pub struct FileList {
    pub root: PathBuf,
    /// Paths relative to root with '/' separators.
    pub files: Vec<String>,
}

impl FileList {
    pub fn scan(root: &Path) -> Self {
        let mut files = Self::walk(root, root);
        files.sort();
        Self { root: root.to_path_buf(), files }
    }

    /// The list after files were added or removed in `dirs` (absolute folders under the
    /// root): each folder's part of the list is walked again, the rest is kept. A folder with
    /// no files in the list (new, ignored or excluded, or empty before) and the root itself
    /// fall back to a full scan, so the result is what `scan` would give.
    pub fn rescan_dirs(&self, dirs: &[PathBuf]) -> Self {
        let mut files = self.files.clone();
        for d in dirs {
            let Some(rel) = d.strip_prefix(&self.root).ok().map(|r| r.to_string_lossy().replace('\\', "/")).filter(|r| !r.is_empty()) else {
                return Self::scan(&self.root);
            };
            // Paths under one prefix are contiguous in a sorted list.
            let prefix = format!("{rel}/");
            let start = files.partition_point(|f| f.as_str() < prefix.as_str());
            let end = start + files[start..].partition_point(|f| f.starts_with(&prefix));
            if start == end {
                return Self::scan(&self.root);
            }
            let mut fresh = Self::walk(&self.root, d);
            fresh.sort();
            files.splice(start..end, fresh);
        }
        Self { root: self.root.clone(), files }
    }

    /// Paths in `new` but not in `self`, and in `self` but not in `new` (both sorted).
    pub fn changes<'a>(&'a self, new: &'a FileList) -> (Vec<&'a str>, Vec<&'a str>) {
        let (mut added, mut removed) = (Vec::new(), Vec::new());
        let (mut a, mut b) = (self.files.iter().peekable(), new.files.iter().peekable());
        loop {
            match (a.peek(), b.peek()) {
                (Some(x), Some(y)) if x == y => {
                    a.next();
                    b.next();
                }
                (Some(x), Some(y)) if x < y => removed.extend(a.next().map(String::as_str)),
                (Some(_), Some(_)) | (None, Some(_)) => added.extend(b.next().map(String::as_str)),
                (Some(_), None) => removed.extend(a.next().map(String::as_str)),
                (None, None) => break,
            }
        }
        (added, removed)
    }

    /// Files under `start` (the root or a folder in it), relative to `root`, unsorted.
    fn walk(root: &Path, start: &Path) -> Vec<String> {
        let structure = Structure::load(root);
        let base = root.to_path_buf();
        let walker = ignore::WalkBuilder::new(start)
            .hidden(false)
            .git_ignore(true)
            .git_exclude(true)
            .parents(true)
            .filter_entry(move |e| {
                let name = e.file_name().to_string_lossy();
                let dir = e.file_type().is_some_and(|t| t.is_dir());
                !(is_hidden(&name)
                    || (dir && (is_excluded_dir(&name) || rel_path(&base, e.path()).is_some_and(|r| !r.is_empty() && structure.is_excluded(&r)))))
            })
            .build_parallel();
        let (tx, rx) = std::sync::mpsc::channel::<String>();
        walker.run(|| {
            let tx = tx.clone();
            let root = root.to_path_buf();
            Box::new(move |e| {
                if let Ok(e) = e {
                    if e.file_type().is_some_and(|t| t.is_file()) {
                        if let Ok(rel) = e.path().strip_prefix(&root) {
                            let _ = tx.send(rel.to_string_lossy().replace('\\', "/"));
                        }
                    }
                }
                ignore::WalkState::Continue
            })
        });
        drop(tx);
        rx.into_iter().collect()
    }

    /// Fuzzy match, best first. Filename matches rank above directory-only matches.
    pub fn search(&self, query: &str, limit: usize) -> Vec<(usize, u32)> {
        if query.trim().is_empty() {
            return Vec::new();
        }
        let mut out = self.score_all(query, None);
        self.best(&mut out, limit);
        out
    }

    /// [`FileList::search`] for typing: when `query` extends the previous query on this list
    /// (on this thread) and has no pattern syntax, only the previous hits are scored again, as
    /// a path that matches `abc` also matches `ab`.
    pub fn search_narrowing(self: &std::sync::Arc<Self>, query: &str, limit: usize) -> Vec<(usize, u32)> {
        if query.trim().is_empty() {
            return Vec::new();
        }
        let plain = |q: &str| q.is_ascii() && !q.contains([' ', '\\', '^', '$', '!', '\'']);
        let me = std::sync::Arc::downgrade(self);
        let prev = LAST_SEARCH.with_borrow_mut(|l| {
            l.take().filter(|l| std::sync::Weak::ptr_eq(&l.list, &me) && plain(&l.query) && plain(query) && query.starts_with(l.query.as_str()))
        });
        let mut out = self.score_all(query, prev.as_ref().map(|p| p.hits.as_slice()));
        LAST_SEARCH.with_borrow_mut(|l| *l = Some(LastSearch { list: me, query: query.to_string(), hits: out.iter().map(|h| h.0 as u32).collect() }));
        self.best(&mut out, limit);
        out
    }

    /// Every match of `query` among `candidates` (all files by default), in file order.
    fn score_all(&self, query: &str, candidates: Option<&[u32]>) -> Vec<(usize, u32)> {
        let pattern = Pattern::parse(query, CaseMatching::Smart, Normalization::Smart);
        // Matcher::new allocates its scoring matrix; one per thread is reused.
        MATCHER.with_borrow_mut(|m| {
            let matcher = m.get_or_insert_with(|| Matcher::new(Config::DEFAULT.match_paths()));
            let mut buf = Vec::new();
            let mut score = |i: usize| {
                let f = &self.files[i];
                let score = pattern.score(Utf32Str::new(f, &mut buf), matcher)?;
                let name = f.rsplit('/').next().unwrap_or(f);
                let bonus = pattern.score(Utf32Str::new(name, &mut buf), matcher).map_or(0, |s| s / 2);
                // Shorter paths win ties.
                Some((i, (score + bonus).saturating_sub(f.len() as u32 / 8)))
            };
            match candidates {
                Some(c) => c.iter().filter_map(|&i| score(i as usize)).collect(),
                None => (0..self.files.len()).filter_map(score).collect(),
            }
        })
    }

    /// The best `limit` hits, best first: score, then the shorter path, then file order.
    fn best(&self, hits: &mut Vec<(usize, u32)>, limit: usize) {
        let cmp = |a: &(usize, u32), b: &(usize, u32)| b.1.cmp(&a.1).then_with(|| self.files[a.0].len().cmp(&self.files[b.0].len())).then(a.0.cmp(&b.0));
        if limit == 0 {
            hits.clear();
            return;
        }
        if hits.len() > limit {
            hits.select_nth_unstable_by(limit - 1, cmp);
            hits.truncate(limit);
        }
        hits.sort_unstable_by(cmp);
    }

    pub fn abs(&self, i: usize) -> PathBuf {
        self.root.join(&self.files[i])
    }
}

#[cfg(test)]
mod tests {
    #[test]
    fn set_entries_reuses_nodes() {
        let d = std::env::temp_dir().join(format!("brisk-merge-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&d);
        std::fs::create_dir_all(&d).unwrap();
        for i in 0..100 {
            std::fs::write(d.join(format!("f{i:03}.py")), "").unwrap();
        }
        let mut t = super::ProjectTree::new(&d);
        let n0 = t.nodes.len();
        let first = t.nodes[0].children.clone().unwrap();
        t.set_entries(0, super::read_entries(&d));
        assert_eq!(t.nodes.len(), n0, "no change: no new nodes");
        assert_eq!(t.nodes[0].children.as_ref().unwrap(), &first);
        std::fs::write(d.join("f050b.py"), "").unwrap();
        std::fs::remove_file(d.join("f000.py")).unwrap();
        t.set_entries(0, super::read_entries(&d));
        assert_eq!(t.nodes.len(), n0 + 1, "one new file: one new node");
        let names: Vec<_> = t.nodes[0].children.as_ref().unwrap().iter().map(|&c| t.nodes[c].name.clone()).collect();
        assert_eq!(names.len(), 100);
        assert_eq!(names[0], "f001.py");
        assert!(names.contains(&"f050b.py".to_string()));
        let _ = std::fs::remove_dir_all(&d);
    }

    #[test]
    fn set_entries_reports_name_changes() {
        let d = std::env::temp_dir().join(format!("brisk-names-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&d);
        std::fs::create_dir_all(&d).unwrap();
        std::fs::write(d.join("a.py"), "").unwrap();
        let mut t = super::ProjectTree::new(&d);
        std::fs::write(d.join("a.py"), "x = 1").unwrap();
        assert!(!t.set_entries(0, super::read_entries(&d)), "same names");
        std::fs::write(d.join("b.py"), "").unwrap();
        assert!(t.set_entries(0, super::read_entries(&d)), "added");
        std::fs::remove_file(d.join("a.py")).unwrap();
        assert!(t.set_entries(0, super::read_entries(&d)), "removed");
        std::fs::remove_file(d.join("b.py")).unwrap();
        std::fs::create_dir(d.join("b.py")).unwrap();
        assert!(t.set_entries(0, super::read_entries(&d)), "file became a folder");
        assert!(!t.set_entries(0, super::read_entries(&d)));
        let _ = std::fs::remove_dir_all(&d);
    }

    #[test]
    fn rescan_dirs_matches_a_full_scan() {
        let d = std::env::temp_dir().join(format!("brisk-rescan-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&d);
        for p in ["a/b", "a.b", "a-c", "z", "a/node_modules"] {
            std::fs::create_dir_all(d.join(p)).unwrap();
        }
        for f in ["a/x.py", "a/b/y.py", "a.b/k.py", "a-c/k.py", "z/w.py", "top.py", "a/node_modules/m.js"] {
            std::fs::write(d.join(f), "").unwrap();
        }
        let list = super::FileList::scan(&d);
        assert!(!list.files.iter().any(|f| f.contains("node_modules")));
        std::fs::write(d.join("a/new.py"), "").unwrap();
        std::fs::create_dir_all(d.join("a/sub")).unwrap();
        std::fs::write(d.join("a/sub/s.py"), "").unwrap();
        std::fs::remove_file(d.join("a/b/y.py")).unwrap();
        std::fs::write(d.join("a/node_modules/n.js"), "").unwrap();
        let part = list.rescan_dirs(&[d.join("a")]);
        let full = super::FileList::scan(&d);
        assert_eq!(part.files, full.files);
        let (added, removed) = list.changes(&part);
        assert_eq!(added, vec!["a/new.py", "a/sub/s.py"]);
        assert_eq!(removed, vec!["a/b/y.py"]);
        // A folder that had no files in the list: a full scan.
        std::fs::create_dir_all(d.join("fresh")).unwrap();
        std::fs::write(d.join("fresh/f.py"), "").unwrap();
        assert_eq!(part.rescan_dirs(&[d.join("fresh")]).files, super::FileList::scan(&d).files);
        let _ = std::fs::remove_dir_all(&d);
    }

    #[test]
    fn reload_keeps_expanded_folders() {
        let d = std::env::temp_dir().join(format!("brisk-reload-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&d);
        std::fs::create_dir_all(d.join("a/b")).unwrap();
        std::fs::create_dir_all(d.join("c")).unwrap();
        std::fs::write(d.join("a/b/x.py"), "").unwrap();
        let mut t = super::ProjectTree::new(&d);
        let b = t.reveal(&d.join("a/b/x.py")).unwrap();
        assert!(!t.nodes[b].is_dir);
        std::fs::write(d.join("a/new.py"), "").unwrap();
        let a = t.find(&d.join("a")).unwrap();
        t.reload(a);
        assert!(t.find(&d.join("a/new.py")).is_some());
        let bdir = t.find(&d.join("a/b")).unwrap();
        assert!(t.nodes[bdir].expanded, "a/b stays expanded");
        assert!(t.find(&d.join("a/b/x.py")).is_some());
        let _ = std::fs::remove_dir_all(&d);
    }

    use super::*;
    use std::fs;

    /// A fresh sample project; `tag` keeps tests running in parallel apart.
    fn sample(tag: &str) -> PathBuf {
        let d = std::env::temp_dir().join(format!("brisk-proj-{tag}-{}", std::process::id()));
        let _ = fs::remove_dir_all(&d);
        fs::create_dir_all(d.join("app/models")).unwrap();
        fs::create_dir_all(d.join(".venv/lib")).unwrap();
        fs::create_dir_all(d.join("__pycache__")).unwrap();
        fs::create_dir_all(d.join(".brisk")).unwrap();
        fs::create_dir_all(d.join(".idea")).unwrap();
        fs::write(d.join("app/models/user.py"), "class User: ...").unwrap();
        fs::write(d.join("app/main.py"), "").unwrap();
        fs::write(d.join("README.md"), "").unwrap();
        fs::write(d.join(".venv/lib/site.py"), "").unwrap();
        d
    }

    #[test]
    fn tree_lazy_sorted_hidden_excluded() {
        let d = sample("tree");
        let mut t = ProjectTree::new(&d);
        let rows = t.rows().to_vec();
        let names: Vec<_> = rows.iter().skip(1).map(|r| t.nodes[r.id].name.clone()).collect();
        assert_eq!(names, vec![".venv", "app", "README.md"]);
        let venv = t.nodes.iter().position(|n| n.name == ".venv").unwrap();
        assert!(t.nodes[venv].excluded);
        assert!(t.nodes[venv].children.is_none(), "not loaded until expanded");
        let id = t.reveal(&d.join("app/models/user.py")).unwrap();
        assert_eq!(t.nodes[id].name, "user.py");
        assert!(t.rows().iter().any(|r| r.id == id));
    }

    #[test]
    fn file_list_and_fuzzy() {
        let d = sample("fuzzy");
        let fl = FileList::scan(&d);
        assert!(fl.files.contains(&"app/models/user.py".to_string()));
        assert!(!fl.files.iter().any(|f| f.starts_with(".venv")));
        let r = fl.search("usr", 10);
        assert_eq!(fl.files[r[0].0], "app/models/user.py");
        let r = fl.search("main", 10);
        assert_eq!(fl.files[r[0].0], "app/main.py");
    }

    /// The old full sort, for comparison.
    fn search_by_sorting(fl: &FileList, q: &str, limit: usize) -> Vec<(usize, u32)> {
        let mut all = fl.score_all(q, None);
        all.sort_by(|a, b| b.1.cmp(&a.1).then_with(|| fl.files[a.0].len().cmp(&fl.files[b.0].len())));
        all.truncate(limit);
        all
    }

    #[test]
    fn narrowing_search_matches_a_full_one() {
        let mut files: Vec<String> = Vec::new();
        for d in ["app", "app/models", "tests", "docs", "src/api_v2", "Src/Models"] {
            for f in ["user.py", "user_model.py", "main.py", "models.py", "README.md", "m.py", "u_s_e_r.txt", "UserView.py"] {
                files.push(format!("{d}/{f}"));
            }
        }
        files.sort();
        let fl = std::sync::Arc::new(FileList { root: PathBuf::from("/p"), files });
        for typed in ["user", "models", "srcapi", "Us", "app/m"] {
            for n in 1..=typed.len() {
                let q = &typed[..n];
                for limit in [1, 3, 50] {
                    assert_eq!(fl.search_narrowing(q, limit), search_by_sorting(&fl, q, limit), "{q} / {limit}");
                    assert_eq!(fl.search(q, limit), search_by_sorting(&fl, q, limit));
                }
            }
        }
        // Pattern syntax and a different list start over.
        assert_eq!(fl.search_narrowing("user !md", 50), search_by_sorting(&fl, "user !md", 50));
        let other = std::sync::Arc::new(FileList { root: PathBuf::from("/p"), files: vec!["user.py".into()] });
        assert_eq!(other.search_narrowing("user", 5).len(), 1);
        assert_eq!(fl.search_narrowing("users", 5), search_by_sorting(&fl, "users", 5));
    }
}
