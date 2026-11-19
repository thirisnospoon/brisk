//! Project model: lazily-loaded file tree (Project tool window) and a flat file list
//! for Go to File / Search Everywhere with fuzzy matching.

use nucleo_matcher::pattern::{CaseMatching, Normalization, Pattern};
use nucleo_matcher::{Config, Matcher, Utf32Str};
use std::path::{Path, PathBuf};

/// Names hidden from the tree (PyCharm's default "Ignored files and folders").
const HIDDEN: &[&str] = &[".git", ".hg", ".svn", "__pycache__", ".DS_Store", ".mypy_cache", ".ruff_cache", ".pytest_cache"];
const HIDDEN_EXT: &[&str] = &["pyc", "pyo"];
/// Shown but marked excluded and never indexed.
const EXCLUDED: &[&str] = &[".venv", "venv", "env", "node_modules", ".tox", "dist", "build", ".next", ".nuxt", "target"];

pub fn is_hidden(name: &str) -> bool {
    HIDDEN.contains(&name)
        || Path::new(name).extension().and_then(|e| e.to_str()).is_some_and(|e| HIDDEN_EXT.contains(&e))
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
            nodes: vec![Node {
                name,
                path: root.to_path_buf(),
                is_dir: true,
                excluded: false,
                depth: 0,
                parent: None,
                children: None,
                expanded: false,
            }],
            roots: vec![0],
            rows: Vec::new(),
            rows_dirty: true,
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
        self.nodes.push(Node { name: name.into(), path: dir.to_path_buf(), is_dir: true, excluded: false, depth: 0, parent: None, children: None, expanded: false });
        self.roots.push(id);
        self.rows_dirty = true;
        id
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
            let excluded = parent_excluded || (is_dir && is_excluded_dir(&name));
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
    /// of 50k files that gains one file costs one node, not 50k.
    pub fn set_entries(&mut self, id: NodeId, entries: Vec<Entry>) {
        let depth = self.nodes[id].depth + 1;
        let parent_excluded = self.nodes[id].excluded;
        let mut old: std::collections::HashMap<String, NodeId> =
            self.nodes[id].children.take().unwrap_or_default().into_iter().map(|c| (std::mem::take(&mut self.nodes[c].name), c)).collect();
        let mut ids = Vec::with_capacity(entries.len());
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
                    if let Some(c) = gone {
                        self.nodes[c].expanded = false;
                        self.nodes[c].children = None;
                    }
                    let excluded = parent_excluded || (is_dir && is_excluded_dir(&name));
                    ids.push(self.nodes.len());
                    self.nodes.push(Node { name, path, is_dir, excluded, depth, parent: Some(id), children: None, expanded: false });
                }
            }
        }
        // Removed entries: their nodes stay unreachable (ids are not reused).
        for c in old.into_values() {
            self.nodes[c].expanded = false;
            self.nodes[c].children = None;
        }
        self.nodes[id].children = Some(ids);
        self.rows_dirty = true;
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

/// Flat list of project files for Go to File. Built off the UI thread.
#[derive(Default, Clone)]
pub struct FileList {
    pub root: PathBuf,
    /// Paths relative to root with '/' separators.
    pub files: Vec<String>,
}

impl FileList {
    pub fn scan(root: &Path) -> Self {
        let mut files = Vec::new();
        let walker = ignore::WalkBuilder::new(root)
            .hidden(false)
            .git_ignore(true)
            .git_exclude(true)
            .parents(true)
            .filter_entry(|e| {
                let name = e.file_name().to_string_lossy();
                !(is_hidden(&name) || (e.file_type().is_some_and(|t| t.is_dir()) && is_excluded_dir(&name)))
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
        files.extend(rx);
        files.sort();
        Self { root: root.to_path_buf(), files }
    }

    /// Fuzzy match, best first. Filename matches rank above directory-only matches.
    pub fn search(&self, query: &str, limit: usize) -> Vec<(usize, u32)> {
        if query.trim().is_empty() {
            return Vec::new();
        }
        let mut matcher = Matcher::new(Config::DEFAULT.match_paths());
        let pattern = Pattern::parse(query, CaseMatching::Smart, Normalization::Smart);
        let mut buf = Vec::new();
        let mut out: Vec<(usize, u32)> = Vec::new();
        for (i, f) in self.files.iter().enumerate() {
            if let Some(score) = pattern.score(Utf32Str::new(f, &mut buf), &mut matcher) {
                let name = f.rsplit('/').next().unwrap_or(f);
                let bonus = pattern.score(Utf32Str::new(name, &mut buf), &mut matcher).map_or(0, |s| s / 2);
                // Shorter paths win ties.
                let s = (score + bonus).saturating_sub(f.len() as u32 / 8);
                out.push((i, s));
            }
        }
        out.sort_by(|a, b| b.1.cmp(&a.1).then_with(|| self.files[a.0].len().cmp(&self.files[b.0].len())));
        out.truncate(limit);
        out
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
}
