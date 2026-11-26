//! Projects, as in PyCharm's project widget: New Project…, Open…, recent projects, opening in
//! this window or a new one. Also the Classes and Symbols tabs of Search Everywhere, backed by a
//! tree-sitter index of the project's Python files.

use super::lsp_ui::NavPos;
use super::{PopupItem, PopupKind, Target, Workspace};
use brisk_project::FileList;
use brisk_syntax::{Lang, OutlineKind, SyntaxTree};
use gpui::{Context, Window};
use std::path::{Path, PathBuf};
use std::sync::Arc;

const RECENT_FILE: &str = "recentProjects.json";
const RECENT_LIMIT: usize = 20;
/// Files bigger than this are skipped by the symbol index (generated code, data dumps).
const MAX_INDEXED_FILE: u64 = 2 * 1024 * 1024;

#[derive(Clone, Debug)]
pub(super) enum ProjectCmd {
    New,
    Open,
    /// Open the folder; `true` keeps this window and opens another one.
    Go(PathBuf, bool),
}

fn recent_path() -> PathBuf {
    brisk_db::config_dir().join(RECENT_FILE)
}

fn same_path(a: &Path, b: &Path) -> bool {
    if cfg!(windows) {
        a.to_string_lossy().eq_ignore_ascii_case(&b.to_string_lossy())
    } else {
        a == b
    }
}

/// Recently opened projects, newest first; folders that no longer exist are left out.
pub(crate) fn recent_projects() -> Vec<PathBuf> {
    std::fs::read_to_string(recent_path())
        .ok()
        .and_then(|s| serde_json::from_str::<Vec<String>>(&s).ok())
        .unwrap_or_default()
        .into_iter()
        .map(PathBuf::from)
        .filter(|p| p.is_dir())
        .collect()
}

pub(crate) fn remember_project(root: &Path) {
    let mut v = recent_projects();
    v.retain(|p| !same_path(p, root));
    v.insert(0, root.to_path_buf());
    v.truncate(RECENT_LIMIT);
    let list: Vec<String> = v.iter().map(|p| p.to_string_lossy().into_owned()).collect();
    let path = recent_path();
    if let Some(d) = path.parent() {
        let _ = std::fs::create_dir_all(d);
    }
    if let Ok(json) = serde_json::to_string_pretty(&list) {
        let _ = std::fs::write(path, json);
    }
}

fn folder_name(p: &Path) -> String {
    p.file_name().map(|n| n.to_string_lossy().into_owned()).unwrap_or_else(|| p.to_string_lossy().into_owned())
}

/// One symbol of a file. Symbols with the same container share its string, and top-level
/// ones share one empty string.
pub(super) struct Sym {
    name: Arc<str>,
    kind: OutlineKind,
    /// Enclosing classes/functions, dotted.
    container: Arc<str>,
    line: u32,
    col: u32,
}

/// Symbols of one file with the size and mtime they were read at.
pub(super) struct FileSyms {
    stamp: (u64, Option<std::time::SystemTime>),
    syms: Box<[Sym]>,
}

/// A symbol with the index of its file in the file list.
#[derive(Clone, Copy)]
pub(super) struct SymRef<'a> {
    pub name: &'a str,
    pub kind: OutlineKind,
    pub container: &'a str,
    pub file: usize,
    pub line: usize,
    pub col: usize,
}

/// The symbol index: each file's symbols (shared with the cache, not copied), in file-list
/// order, which is path order.
#[derive(Default)]
pub(super) struct SymIndex {
    files: Vec<(u32, Arc<FileSyms>)>,
}

impl SymIndex {
    #[cfg(feature = "smoke")]
    pub fn len(&self) -> usize {
        self.files.iter().map(|f| f.1.syms.len()).sum()
    }

    fn get(&self, slot: u32, i: u32) -> SymRef<'_> {
        let (file, syms) = &self.files[slot as usize];
        let s = &syms.syms[i as usize];
        SymRef { name: &s.name, kind: s.kind, container: &s.container, file: *file as usize, line: s.line as usize, col: s.col as usize }
    }

    pub fn iter(&self) -> impl Iterator<Item = SymRef<'_>> {
        self.files.iter().enumerate().flat_map(move |(slot, (_, f))| (0..f.syms.len()).map(move |i| self.get(slot as u32, i as u32)))
    }
}

/// A search hit: score, slot in `SymIndex::files`, symbol in that file.
type Hit = (i32, u32, u32);

/// The last search's hits, for narrowing when the next query extends it.
struct LastHits {
    list: std::sync::Weak<SymIndex>,
    query: String,
    classes_only: bool,
    hits: Vec<(u32, u32)>,
}

/// Per-file symbols of the last index by relative path. A rescan (after any change in an
/// expanded folder, a save, a `__pycache__` write) re-parses only files whose size or mtime
/// changed instead of the whole project, which took minutes of one core on a 24k-file tree.
pub(super) type SymCache = std::collections::HashMap<String, Arc<FileSyms>>;

#[derive(Default)]
pub(super) struct Symbols {
    pub list: Option<Arc<SymIndex>>,
    pub files: Option<Arc<FileList>>,
    pub cache: Arc<SymCache>,
    pub building: bool,
    /// A file was saved since the index was built.
    pub stale: bool,
    last: std::cell::RefCell<Option<LastHits>>,
}

fn empty_str() -> Arc<str> {
    static EMPTY: std::sync::OnceLock<Arc<str>> = std::sync::OnceLock::new();
    EMPTY.get_or_init(|| "".into()).clone()
}

fn file_symbols(path: &Path) -> Vec<Sym> {
    let mut out = Vec::new();
    let Some(text) = brisk_search::read_text(path) else { return out };
    let mut tree = SyntaxTree::new(Lang::Python);
    tree.parse(&text);
    let col = |byte: usize| {
        let line_start = text[..byte].rfind('\n').map(|n| n + 1).unwrap_or(0);
        text[line_start..byte].chars().count() as u32
    };
    // `containers[k]`: the first k names of `stack`, dotted, made once per container.
    let mut stack: Vec<Arc<str>> = Vec::new();
    let mut containers: Vec<Arc<str>> = vec![empty_str()];
    for o in tree.outline(&text) {
        stack.truncate(o.depth);
        containers.truncate(stack.len() + 1);
        let name: Arc<str> = o.name.as_str().into();
        let container = containers[stack.len()].clone();
        out.push(Sym { name: name.clone(), kind: o.kind, container: container.clone(), line: o.line as u32, col: col(o.byte) });
        containers.push(if container.is_empty() { name.clone() } else { format!("{container}.{name}").into() });
        stack.push(name);
    }
    for o in tree.module_variables(&text) {
        out.push(Sym { name: o.name.as_str().into(), kind: o.kind, container: empty_str(), line: o.line as u32, col: col(o.byte) });
    }
    out
}

fn index(files: &FileList, old: &SymCache) -> (SymIndex, SymCache) {
    let py: Vec<usize> = (0..files.files.len()).filter(|&i| files.files[i].ends_with(".py") || files.files[i].ends_with(".pyi")).collect();
    // Files are parsed on several threads, each taking the next file from a shared counter.
    let next = std::sync::atomic::AtomicUsize::new(0);
    let read = || {
        let mut out = Vec::new();
        loop {
            let k = next.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
            let Some(&i) = py.get(k) else { break };
            let rel = &files.files[i];
            let path = files.abs(i);
            let Ok(meta) = std::fs::metadata(&path) else { continue };
            if meta.len() > MAX_INDEXED_FILE {
                continue;
            }
            let stamp = (meta.len(), meta.modified().ok());
            let entry = match old.get(rel) {
                Some(e) if e.stamp == stamp && stamp.1.is_some() => e.clone(),
                _ => Arc::new(FileSyms { stamp, syms: file_symbols(&path).into_boxed_slice() }),
            };
            out.push((i as u32, entry));
        }
        out
    };
    let threads = std::thread::available_parallelism().map_or(1, |n| n.get().saturating_sub(1)).clamp(1, 8).min(py.len() / 32 + 1);
    let mut parts: Vec<(u32, Arc<FileSyms>)> = if threads == 1 {
        read()
    } else {
        std::thread::scope(|s| {
            let handles: Vec<_> = (0..threads).map(|_| s.spawn(read)).collect();
            handles.into_iter().flat_map(|h| h.join().unwrap_or_default()).collect()
        })
    };
    parts.sort_unstable_by_key(|p| p.0);
    let cache: SymCache = parts.iter().map(|(i, e)| (files.files[*i as usize].clone(), e.clone())).collect();
    parts.retain(|p| !p.1.syms.is_empty());
    (SymIndex { files: parts }, cache)
}

/// A query for [`name_score`], lowercased once instead of per name.
pub(super) struct NameQuery {
    lower: String,
    /// The letters matched against word starts (`_` left out).
    letters: Vec<char>,
    ascii: bool,
}

impl NameQuery {
    pub(super) fn new(q: &str) -> Self {
        let lower = q.to_lowercase();
        let letters = lower.chars().filter(|c| *c != '_').collect();
        Self { ascii: lower.is_ascii(), lower, letters }
    }

    /// [`name_score`] with this query. ASCII names (nearly all) are compared in place, without
    /// lowercased copies.
    pub(super) fn score(&self, name: &str) -> Option<i32> {
        if !(self.ascii && name.is_ascii()) {
            return name_score_slow(name, self);
        }
        let (n, q) = (name.as_bytes(), self.lower.as_bytes());
        let len = n.len() as i32;
        if n.eq_ignore_ascii_case(q) {
            return Some(4000);
        }
        if n.len() >= q.len() && n[..q.len()].eq_ignore_ascii_case(q) {
            return Some(3000 - len);
        }
        // Word starts: first char, after `_`, and upper-case letters after lower-case ones.
        let mut it = self.letters.iter().peekable();
        for (i, &c) in n.iter().enumerate() {
            if (i == 0 || n[i - 1] == b'_' || (c.is_ascii_uppercase() && n[i - 1].is_ascii_lowercase()))
                && it.peek().is_some_and(|&&l| l == c.to_ascii_lowercase() as char)
            {
                it.next();
            }
        }
        if it.peek().is_none() {
            return Some(2000 - len);
        }
        if q.len() > n.len() {
            return None;
        }
        (0..=n.len() - q.len()).find(|&p| n[p..p + q.len()].eq_ignore_ascii_case(q)).map(|p| 1000 - p as i32 * 10 - len)
    }
}

/// PyCharm-like name matching: exact, prefix, word/camel-hump starts (`gcw` → `get_current_window`),
/// then substring. Higher is better.
pub(super) fn name_score(name: &str, q: &str) -> Option<i32> {
    NameQuery::new(q).score(name)
}

fn name_score_slow(name: &str, q: &NameQuery) -> Option<i32> {
    let (n, ql) = (name.to_lowercase(), q.lower.as_str());
    let len = name.chars().count() as i32;
    if n == ql {
        return Some(4000);
    }
    if n.starts_with(ql) {
        return Some(3000 - len);
    }
    let chars: Vec<char> = name.chars().collect();
    let starts: Vec<char> = chars
        .iter()
        .enumerate()
        .filter(|&(i, c)| i == 0 || chars[i - 1] == '_' || (c.is_uppercase() && chars[i - 1].is_lowercase()))
        .map(|(_, c)| c.to_ascii_lowercase())
        .collect();
    let mut it = q.letters.iter().peekable();
    for c in &starts {
        if it.peek() == Some(&c) {
            it.next();
        }
    }
    if it.peek().is_none() {
        return Some(2000 - len);
    }
    n.find(ql).map(|p| 1000 - p as i32 * 10 - len)
}

/// The best `k` hits, best first; equal scores in path order, then file order.
fn top(hits: &mut Vec<Hit>, k: usize) {
    let cmp = |a: &Hit, b: &Hit| b.0.cmp(&a.0).then(a.1.cmp(&b.1)).then(a.2.cmp(&b.2));
    if k == 0 {
        hits.clear();
        return;
    }
    if hits.len() > k {
        hits.select_nth_unstable_by(k - 1, cmp);
        hits.truncate(k);
    }
    hits.sort_unstable_by(cmp);
}

impl Workspace {
    /// Builds the symbol index in the background when it is missing or a file was saved.
    pub(super) fn ensure_symbols(&mut self, cx: &mut Context<Self>) {
        let s = &mut self.symbols;
        if s.building || (s.list.is_some() && !s.stale) {
            return;
        }
        let Some(files) = self.files.clone() else { return };
        s.building = true;
        s.stale = false;
        let for_task = files.clone();
        let old = s.cache.clone();
        let task = cx.background_executor().spawn(async move { index(&for_task, &old) });
        cx.spawn(async move |this, cx| {
            let (list, cache) = task.await;
            let _ = this.update(cx, |this, cx| {
                this.symbols.building = false;
                this.symbols.list = Some(Arc::new(list));
                this.symbols.cache = Arc::new(cache);
                this.symbols.files = Some(files);
                // Saved while building: catch up (cheap, only changed files are parsed).
                this.ensure_symbols(cx);
                this.refresh_popup();
                cx.notify();
            });
        })
        .detach();
    }

    pub(super) fn symbols_indexing(&self) -> bool {
        self.symbols.list.is_none()
    }

    pub(super) fn symbol_items(&self, q: &str, classes_only: bool, limit: usize) -> Vec<PopupItem> {
        let mut hits = self.symbol_hits(q, classes_only);
        top(&mut hits, limit);
        self.symbol_popup_items(&hits)
    }

    /// Search Everywhere's All tab from one pass over the index: the best `classes` classes,
    /// and of the best `others` symbols those that are not classes.
    pub(super) fn symbol_items_all(&self, q: &str, classes: usize, others: usize) -> (Vec<PopupItem>, Vec<PopupItem>) {
        let mut hits = self.symbol_hits(q, false);
        let Some(list) = &self.symbols.list else { return Default::default() };
        let is_class = |h: &Hit| list.get(h.1, h.2).kind == OutlineKind::Class;
        let mut c: Vec<Hit> = hits.iter().copied().filter(is_class).collect();
        top(&mut c, classes);
        top(&mut hits, others);
        hits.retain(|h| !is_class(h));
        (self.symbol_popup_items(&c), self.symbol_popup_items(&hits))
    }

    /// Every symbol matching `q`, unsorted. A query that extends the previous one (and has no
    /// `.`) only looks at the previous hits: a name matching `abc` also matches `ab`.
    fn symbol_hits(&self, q: &str, classes_only: bool) -> Vec<Hit> {
        let Some(list) = &self.symbols.list else { return Vec::new() };
        let q = q.trim();
        if q.is_empty() {
            return Vec::new();
        }
        // `Repo.add` (or `Re.ad`): the member part against the name, the rest against the
        // innermost container, as in PyCharm.
        let (outer, member) = match q.rsplit_once('.') {
            Some((o, m)) if !o.is_empty() => (Some(o.rsplit('.').next().unwrap_or(o)), m),
            _ => (None, q),
        };
        let (member, outer) = (NameQuery::new(member), outer.map(NameQuery::new));
        let score = |slot: u32, i: u32| {
            let s = list.get(slot, i);
            if classes_only && s.kind != OutlineKind::Class {
                return None;
            }
            let own = member.score(s.name)?;
            match &outer {
                Some(o) => Some(own + o.score(s.container.rsplit('.').next().unwrap_or(""))?),
                None => Some(own),
            }
        };
        let mut last = self.symbols.last.borrow_mut();
        let narrow = last.as_ref().filter(|l| {
            std::sync::Weak::ptr_eq(&l.list, &Arc::downgrade(list))
                && q.is_ascii()
                && l.classes_only == classes_only
                && q.starts_with(l.query.as_str())
                && !q.contains('.')
        });
        let hits: Vec<Hit> = match narrow {
            Some(l) => l.hits.iter().filter_map(|&(slot, i)| Some((score(slot, i)?, slot, i))).collect(),
            None => list
                .files
                .iter()
                .enumerate()
                .flat_map(|(slot, (_, f))| (0..f.syms.len() as u32).map(move |i| (slot as u32, i)))
                .filter_map(|(slot, i)| Some((score(slot, i)?, slot, i)))
                .collect(),
        };
        *last = Some(LastHits { list: Arc::downgrade(list), query: q.to_string(), classes_only, hits: hits.iter().map(|h| (h.1, h.2)).collect() });
        hits
    }

    fn symbol_popup_items(&self, hits: &[Hit]) -> Vec<PopupItem> {
        let (Some(list), Some(files)) = (&self.symbols.list, &self.symbols.files) else { return Vec::new() };
        hits.iter()
            .map(|h| {
                let s = list.get(h.1, h.2);
                let rel = &files.files[s.file];
                let detail = if s.container.is_empty() { format!("{rel}:{}", s.line + 1) } else { format!("{} · {rel}:{}", s.container, s.line + 1) };
                PopupItem { label: s.name.to_string(), detail, hint: String::new(), target: Target::Symbol(files.abs(s.file), s.line, s.col, s.kind) }
            })
            .collect()
    }

    pub(super) fn open_symbol(&mut self, path: PathBuf, line: usize, col: usize, window: &mut Window, cx: &mut Context<Self>) {
        self.navigate_to(NavPos { path, line, col }, window, cx);
    }

    // --- Projects --------------------------------------------------------------------

    pub(super) fn project_items(&self, q: &str) -> Vec<PopupItem> {
        let ql = q.to_lowercase();
        let item = |label: &str, detail: String, cmd: ProjectCmd| PopupItem { label: label.into(), detail, hint: String::new(), target: Target::Project(cmd) };
        let mut v = Vec::new();
        if "new project".contains(&ql) {
            v.push(item("New Project…", "create a folder and open it".into(), ProjectCmd::New));
        }
        if "open".contains(&ql) {
            v.push(item("Open…", "a folder with your code".into(), ProjectCmd::Open));
        }
        for p in recent_projects().into_iter().filter(|p| !same_path(p, &self.root)) {
            let name = folder_name(&p);
            let path = p.to_string_lossy().into_owned();
            if name.to_lowercase().contains(&ql) || path.to_lowercase().contains(&ql) {
                v.push(PopupItem { label: name, detail: path, hint: "new window".into(), target: Target::Project(ProjectCmd::Go(p, true)) });
            }
        }
        v
    }

    pub(super) fn open_where_items(&self, q: &str) -> Vec<PopupItem> {
        let Some(path) = self.project_pending.clone() else { return Vec::new() };
        let ql = q.to_lowercase();
        [("This Window", false), ("New Window", true)]
            .into_iter()
            .filter(|(l, _)| l.to_lowercase().contains(&ql))
            .map(|(l, new)| PopupItem {
                label: l.into(),
                detail: path.to_string_lossy().into_owned(),
                hint: String::new(),
                target: Target::Project(ProjectCmd::Go(path.clone(), new)),
            })
            .collect()
    }

    pub(super) fn project_cmd(&mut self, cmd: ProjectCmd, cx: &mut Context<Self>) {
        match cmd {
            ProjectCmd::Open => self.open_folder_picker(super::folder_picker_ui::Purpose::Open, cx),
            ProjectCmd::New => self.open_folder_picker(super::folder_picker_ui::Purpose::New, cx),
            ProjectCmd::Go(p, new_window) => self.open_project(&p, new_window, cx),
        }
    }

    pub(super) fn ask_open_where(&mut self, p: PathBuf, cx: &mut Context<Self>) {
        if same_path(&p, &self.root) {
            self.status = "This project is already open".into();
            cx.notify();
            return;
        }
        self.project_pending = Some(p);
        self.open_popup(PopupKind::OpenWhere);
        cx.notify();
    }

    /// Starts Brisk on `p`. "This Window" saves everything and closes this one.
    fn open_project(&mut self, p: &Path, new_window: bool, cx: &mut Context<Self>) {
        if new_window {
            self.save_all(cx);
        } else if !self.save_all_for_quit(cx) {
            return;
        }
        let exe = match std::env::current_exe() {
            Ok(e) => e,
            Err(e) => {
                self.status = format!("Cannot open project: {e}");
                return;
            }
        };
        let spawned = std::process::Command::new(exe)
            .arg(p)
            .current_dir(p)
            .stdin(std::process::Stdio::null())
            .stdout(std::process::Stdio::null())
            .stderr(std::process::Stdio::null())
            .spawn();
        match spawned {
            Ok(_) => {
                remember_project(p);
                if !new_window {
                    cx.quit();
                } else {
                    self.status = format!("Opening {} in a new window", folder_name(p));
                }
            }
            Err(e) => self.status = format!("Cannot open project: {e}"),
        }
        cx.notify();
    }
}

// --- Smoke check (BRISK_SMOKE_UI=1) ----------------------------------------------------
//
// Sends real mouse events to the window: clicks along the Search Everywhere tab row, a click
// outside the popup, then lists the Classes/Symbols results and the project widget's menu.

impl Workspace {
    #[cfg(feature = "smoke")]
    fn smoke_state(&self) -> String {
        match &self.popup {
            None => "closed".into(),
            Some(p) => format!("{:?} items={}", p.kind, p.items.len()),
        }
    }

    #[cfg(feature = "smoke")]
    pub(super) fn ui_smoke(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let bg = cx.background_executor().clone();
        cx.spawn_in(window, async move |this, cx| {
            let pause = |ms| bg.timer(std::time::Duration::from_millis(ms));
            // Real platform input: WM_LBUTTONDOWN/UP posted to our own window, in physical pixels.
            let scale = cx.update(|window, _| window.scale_factor()).unwrap_or(1.);
            let click = |_: &mut gpui::AsyncWindowContext, x: f32, y: f32| super::smoke::win_input::click(x * scale, y * scale);
            // Wait for the file list and the symbol index.
            for _ in 0..200 {
                pause(50).await;
                if this.update(cx, |this, _| this.symbols.list.is_some()).unwrap_or(true) {
                    break;
                }
            }
            // The window's bounds, not its viewport: a background smoke's window is created
            // hidden, and its viewport can keep the CW_USEDEFAULT size (about -1.7e9 px).
            let width = cx.update(|window, _| f32::from(window.bounds().size.width)).unwrap_or(1400.);
            let _ = this.update(cx, |this, cx| {
                eprintln!("smoke-ui: symbols indexed: {}", this.symbols.list.as_ref().map_or(0, |l| l.len()));
                this.dispatch_smoke(brisk_keymap::Action::SearchEverywhere, cx);
            });
            pause(300).await;
            // Popup is 680 px wide and centered; the tab row is about 112 px from the top.
            let left = (width - 680.) / 2.;
            let mut seen = Vec::new();
            let mut x = left + 14.;
            while x < left + 420. {
                click(cx, x, 112.);
                pause(120).await;
                let s = this.update(cx, |this, _| this.smoke_state()).unwrap_or_default();
                if seen.last() != Some(&s) {
                    eprintln!("smoke-ui: click x={:.0} -> {s}", x - left);
                    seen.push(s);
                }
                x += 12.;
            }
            click(cx, left + 300., 300.);
            pause(150).await;
            eprintln!("smoke-ui: click inside list area -> {}", this.update(cx, |this, _| this.smoke_state()).unwrap_or_default());
            let _ = this.update(cx, |this, cx| {
                this.dispatch_smoke(brisk_keymap::Action::SearchEverywhere, cx);
            });
            pause(200).await;
            click(cx, 30., 700.);
            pause(150).await;
            eprintln!("smoke-ui: click outside -> {}", this.update(cx, |this, _| this.smoke_state()).unwrap_or_default());

            let _ = this.update(cx, |this, cx| {
                for (tab, q) in [
                    (super::SeTab::Classes, "user"),
                    (super::SeTab::Symbols, "fbp"),
                    (super::SeTab::All, "repo"),
                    (super::SeTab::Symbols, "Repo.ad"),
                    (super::SeTab::Symbols, "max_r"),
                ] {
                    this.open_popup(PopupKind::SearchEverywhere(tab));
                    if let Some(p) = this.popup.as_mut() {
                        p.query = q.into();
                    }
                    this.refresh_popup();
                    let p = this.popup.as_ref().unwrap();
                    eprintln!("smoke-ui: {:?} '{q}':", p.kind);
                    for it in p.items.iter().take(6) {
                        eprintln!("smoke-ui:   {} | {}", it.label, it.detail);
                    }
                    this.popup = None;
                }
                this.open_popup(PopupKind::Projects);
                eprintln!("smoke-ui: Projects:");
                for it in &this.popup.as_ref().unwrap().items {
                    eprintln!("smoke-ui:   {} | {} | {}", it.label, it.detail, it.hint);
                }
                cx.notify();
            });
            pause(300).await;
            // The project widget in the toolbar toggles the same menu: click it.
            let _ = this.update(cx, |this, cx| {
                this.popup = None;
                cx.notify();
            });
            pause(200).await;
            click(cx, 60., 20.);
            pause(200).await;
            eprintln!("smoke-ui: click project widget -> {}", this.update(cx, |this, _| this.smoke_state()).unwrap_or_default());
            eprintln!("smoke-ui: done");
            let _ = cx.update(|_, cx| cx.quit());
        })
        .detach();
    }

    #[cfg(feature = "smoke")]
    fn dispatch_smoke(&mut self, a: brisk_keymap::Action, cx: &mut Context<Self>) {
        self.ensure_symbols(cx);
        if a == brisk_keymap::Action::SearchEverywhere {
            self.open_popup(PopupKind::SearchEverywhere(super::SeTab::All));
        }
        cx.notify();
    }
}

#[cfg(test)]
mod tests {
    use super::name_score;

    #[test]
    fn symbol_matching() {
        assert!(name_score("add", "add").unwrap() > name_score("add_all", "add").unwrap());
        assert!(name_score("get_current_window", "gcw").is_some());
        assert!(name_score("HttpRequest", "hr").is_some());
        assert!(name_score("HttpRequest", "req").is_some());
        assert!(name_score("HttpRequest", "xyz").is_none());
        assert!(name_score("HttpRequest", "http").unwrap() > name_score("HttpRequest", "req").unwrap());
    }

    #[test]
    fn ascii_matching_scores_as_the_general_one() {
        let names = ["add", "add_all", "get_current_window", "HttpRequest", "_private", "__init__", "a", "URLParser", "x2y", "MyHTTPServer", "Ab_Cd"];
        let queries = ["add", "a", "gcw", "hr", "req", "http", "_", "__", "init", "urlp", "up", "xyz", "ADD", "mhs", "a_c", "ab_cd", "toolongquerythanname"];
        for n in names {
            for q in queries {
                let q = super::NameQuery::new(q);
                assert_eq!(q.score(n), super::name_score_slow(n, &q), "{n} / {}", q.lower);
            }
        }
        assert_eq!(name_score("Größe", "grö"), Some(3000 - 5));
    }

    #[test]
    fn top_keeps_the_best_in_order() {
        let mut hits = vec![(10, 2, 0), (30, 5, 1), (30, 1, 4), (20, 0, 0), (30, 1, 2), (5, 0, 1)];
        super::top(&mut hits, 3);
        assert_eq!(hits, vec![(30, 1, 2), (30, 1, 4), (30, 5, 1)]);
        let mut all = vec![(1, 0, 0), (2, 0, 1)];
        super::top(&mut all, 10);
        assert_eq!(all, vec![(2, 0, 1), (1, 0, 0)]);
    }

    #[test]
    fn file_symbols_share_containers() {
        let d = std::env::temp_dir().join(format!("brisk-syms-{}", std::process::id()));
        std::fs::create_dir_all(&d).unwrap();
        let f = d.join("m.py");
        std::fs::write(&f, "X = 1\nclass A:\n    def f(self): pass\n    def g(self): pass\n    class B:\n        def h(self): pass\ndef top(): pass\n")
            .unwrap();
        let syms = super::file_symbols(&f);
        let got: Vec<(&str, &str, u32)> = syms.iter().map(|s| (&*s.name, &*s.container, s.line)).collect();
        assert_eq!(got, vec![("A", "", 1), ("f", "A", 2), ("g", "A", 3), ("B", "A", 4), ("h", "A.B", 5), ("top", "", 6), ("X", "", 0)]);
        assert!(std::sync::Arc::ptr_eq(&syms[1].container, &syms[2].container), "one string per container");
        assert!(std::sync::Arc::ptr_eq(&syms[0].container, &syms[6].container), "one empty string");
        let _ = std::fs::remove_dir_all(&d);
    }
}
