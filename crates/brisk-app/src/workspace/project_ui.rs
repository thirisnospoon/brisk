//! Projects, as in PyCharm's project widget: New Project…, Open…, recent projects, opening in
//! this window or a new one. Also the Classes and Symbols tabs of Search Everywhere, backed by a
//! tree-sitter index of the project's Python files.

use super::lsp_ui::NavPos;
use super::{PopupItem, PopupKind, Target, Workspace};
use brisk_project::FileList;
use brisk_syntax::{Lang, OutlineKind, SyntaxTree};
use gpui::{Context, PathPromptOptions, Window};
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

pub(super) struct Sym {
    pub name: String,
    pub kind: OutlineKind,
    /// Enclosing classes/functions, dotted.
    pub container: String,
    pub file: usize,
    pub line: usize,
    pub col: usize,
}

#[derive(Default)]
pub(super) struct Symbols {
    pub list: Option<Arc<Vec<Sym>>>,
    pub files: Option<Arc<FileList>>,
    pub building: bool,
    /// A file was saved since the index was built.
    pub stale: bool,
}

fn index(files: &FileList) -> Vec<Sym> {
    let mut out = Vec::new();
    for (i, rel) in files.files.iter().enumerate() {
        if !(rel.ends_with(".py") || rel.ends_with(".pyi")) {
            continue;
        }
        let path = files.abs(i);
        if std::fs::metadata(&path).map(|m| m.len() > MAX_INDEXED_FILE).unwrap_or(true) {
            continue;
        }
        let Some(text) = brisk_search::read_text(&path) else { continue };
        let mut tree = SyntaxTree::new(Lang::Python);
        tree.parse(&text);
        let mut stack: Vec<String> = Vec::new();
        for o in tree.outline(&text) {
            stack.truncate(o.depth);
            let line_start = text[..o.byte].rfind('\n').map(|n| n + 1).unwrap_or(0);
            out.push(Sym {
                name: o.name.clone(),
                kind: o.kind,
                container: stack.join("."),
                file: i,
                line: o.line,
                col: text[line_start..o.byte].chars().count(),
            });
            stack.push(o.name);
        }
    }
    out
}

/// PyCharm-like name matching: exact, prefix, word/camel-hump starts (`gcw` → `get_current_window`),
/// then substring. Higher is better.
pub(super) fn name_score(name: &str, q: &str) -> Option<i32> {
    let (n, ql) = (name.to_lowercase(), q.to_lowercase());
    let len = name.chars().count() as i32;
    if n == ql {
        return Some(4000);
    }
    if n.starts_with(&ql) {
        return Some(3000 - len);
    }
    // Word starts: first char, after `_`, and upper-case letters after lower-case ones.
    let chars: Vec<char> = name.chars().collect();
    let starts: Vec<char> = chars
        .iter()
        .enumerate()
        .filter(|&(i, c)| i == 0 || chars[i - 1] == '_' || (c.is_uppercase() && chars[i - 1].is_lowercase()))
        .map(|(_, c)| c.to_ascii_lowercase())
        .collect();
    let mut it = ql.chars().filter(|c| *c != '_').peekable();
    for c in &starts {
        if it.peek() == Some(c) {
            it.next();
        }
    }
    if it.peek().is_none() {
        return Some(2000 - len);
    }
    n.find(&ql).map(|p| 1000 - p as i32 * 10 - len)
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
        let task = cx.background_executor().spawn(async move { index(&for_task) });
        cx.spawn(async move |this, cx| {
            let list = task.await;
            let _ = this.update(cx, |this, cx| {
                this.symbols.building = false;
                this.symbols.list = Some(Arc::new(list));
                this.symbols.files = Some(files);
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
        let (Some(list), Some(files)) = (&self.symbols.list, &self.symbols.files) else { return Vec::new() };
        let q = q.trim();
        if q.is_empty() {
            return Vec::new();
        }
        let mut scored: Vec<(i32, &Sym)> = list
            .iter()
            .filter(|s| !classes_only || s.kind == OutlineKind::Class)
            .filter_map(|s| name_score(&s.name, q).map(|sc| (sc, s)))
            .collect();
        scored.sort_by(|a, b| b.0.cmp(&a.0).then_with(|| files.files[a.1.file].cmp(&files.files[b.1.file])));
        scored
            .into_iter()
            .take(limit)
            .map(|(_, s)| {
                let rel = &files.files[s.file];
                let detail = if s.container.is_empty() { format!("{rel}:{}", s.line + 1) } else { format!("{} · {rel}:{}", s.container, s.line + 1) };
                PopupItem {
                    label: s.name.clone(),
                    detail,
                    hint: String::new(),
                    target: Target::Symbol(files.abs(s.file), s.line, s.col, s.kind),
                }
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

    pub(super) fn project_cmd(&mut self, cmd: ProjectCmd, window: &mut Window, cx: &mut Context<Self>) {
        match cmd {
            ProjectCmd::Open => {
                let rx = cx.prompt_for_paths(PathPromptOptions { files: false, directories: true, multiple: false, prompt: Some("Open".into()) });
                cx.spawn_in(window, async move |this, cx| {
                    if let Ok(Ok(Some(paths))) = rx.await {
                        if let Some(p) = paths.into_iter().next() {
                            let _ = this.update(cx, |this, cx| this.ask_open_where(p, cx));
                        }
                    }
                })
                .detach();
            }
            ProjectCmd::New => {
                let parent = self.root.parent().map(Path::to_path_buf).unwrap_or_else(|| self.root.clone());
                let rx = cx.prompt_for_new_path(&parent, Some("untitled"));
                cx.spawn_in(window, async move |this, cx| {
                    if let Ok(Ok(Some(p))) = rx.await {
                        let _ = this.update(cx, |this, cx| match std::fs::create_dir_all(&p) {
                            Ok(()) => this.ask_open_where(p, cx),
                            Err(e) => {
                                this.status = format!("Cannot create {}: {e}", p.display());
                                cx.notify();
                            }
                        });
                    }
                })
                .detach();
            }
            ProjectCmd::Go(p, new_window) => self.open_project(&p, new_window, cx),
        }
    }

    fn ask_open_where(&mut self, p: PathBuf, cx: &mut Context<Self>) {
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
        self.save_all(cx);
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
}

// --- Smoke check (BRISK_SMOKE_UI=1) ----------------------------------------------------
//
// Sends real mouse events to the window: clicks along the Search Everywhere tab row, a click
// outside the popup, then lists the Classes/Symbols results and the project widget's menu.

impl Workspace {
    fn smoke_state(&self) -> String {
        match &self.popup {
            None => "closed".into(),
            Some(p) => format!("{:?} items={}", p.kind, p.items.len()),
        }
    }

    pub(super) fn ui_smoke(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let bg = cx.background_executor().clone();
        cx.spawn_in(window, async move |this, cx| {
            let pause = |ms| bg.timer(std::time::Duration::from_millis(ms));
            // Real platform input: WM_LBUTTONDOWN/UP posted to our own window, in physical pixels.
            let scale = cx.update(|window, _| window.scale_factor()).unwrap_or(1.);
            let click = |_: &mut gpui::AsyncWindowContext, x: f32, y: f32| post_click(x * scale, y * scale);
            // Wait for the file list and the symbol index.
            for _ in 0..200 {
                pause(50).await;
                if this.update(cx, |this, _| this.symbols.list.is_some()).unwrap_or(true) {
                    break;
                }
            }
            let width = cx.update(|window, _| f32::from(window.viewport_size().width)).unwrap_or(1400.);
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
                for (tab, q) in [(super::SeTab::Classes, "user"), (super::SeTab::Symbols, "fbp"), (super::SeTab::All, "repo")] {
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

    fn dispatch_smoke(&mut self, a: brisk_keymap::Action, cx: &mut Context<Self>) {
        self.ensure_symbols(cx);
        if a == brisk_keymap::Action::SearchEverywhere {
            self.open_popup(PopupKind::SearchEverywhere(super::SeTab::All));
        }
        cx.notify();
    }
}

/// Posts a left click to the top-level window of this thread (the Brisk window).
#[cfg(windows)]
fn post_click(x: f32, y: f32) {
    #[link(name = "user32")]
    extern "system" {
        fn EnumThreadWindows(thread: u32, f: extern "system" fn(isize, isize) -> i32, l: isize) -> i32;
        fn IsWindowVisible(h: isize) -> i32;
        fn PostMessageW(h: isize, msg: u32, w: usize, l: isize) -> i32;
    }
    #[link(name = "kernel32")]
    extern "system" {
        fn GetCurrentThreadId() -> u32;
    }
    extern "system" fn found(h: isize, out: isize) -> i32 {
        unsafe {
            if IsWindowVisible(h) != 0 {
                *(out as *mut isize) = h;
                return 0;
            }
        }
        1
    }
    let mut hwnd: isize = 0;
    unsafe {
        EnumThreadWindows(GetCurrentThreadId(), found, &mut hwnd as *mut isize as isize);
        if hwnd == 0 {
            return;
        }
        let l = ((y as i32 as isize & 0xffff) << 16) | (x as i32 as isize & 0xffff);
        PostMessageW(hwnd, 0x0200, 0, l); // WM_MOUSEMOVE
        PostMessageW(hwnd, 0x0201, 1, l); // WM_LBUTTONDOWN
        PostMessageW(hwnd, 0x0202, 0, l); // WM_LBUTTONUP
    }
}

#[cfg(not(windows))]
fn post_click(_: f32, _: f32) {}
