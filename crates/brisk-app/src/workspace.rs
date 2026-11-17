//! Main window: tool window stripes, project tree, editor tabs, popups, status bar.
//! All shortcuts go through brisk-keymap's PyCharm keymap.

mod db_ui;
mod disk_ui;
mod edit2_smoke;
mod edit_smoke;
mod files_ui;
mod find_ui;
mod git_ui;
mod lsp_ui;
mod menu_ui;
mod problems_ui;
pub(crate) mod project_ui;
mod run_ui;
mod win_input;

use crate::diff_view::DiffView;
use crate::editor::Editor;
use crate::grid_view::GridView;
use crate::keys;
use crate::terminal_view::TerminalPanel;
use crate::theme::{Theme, MONO_FONT, UI_FONT};
use brisk_keymap::{pycharm_keymap, Action, KeyResolver, Resolution, ALL_ACTIONS};
use brisk_project::{FileList, NodeId, ProjectTree};
use gpui::prelude::*;
use gpui::{
    div, px, uniform_list, App, ClickEvent, Context, Entity, FocusHandle, Focusable, KeyDownEvent, ModifiersChangedEvent,
    MouseButton, SharedString, UniformListScrollHandle, Window,
};
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::time::Instant;

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum LeftTool {
    Project,
    Commit,
    Structure,
    Bookmarks,
}

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum BottomTool {
    Terminal,
    Run,
    Debug,
    Problems,
    Git,
    Services,
    Find,
}

impl BottomTool {
    fn title(self) -> &'static str {
        match self {
            BottomTool::Terminal => "Terminal",
            BottomTool::Run => "Run",
            BottomTool::Debug => "Debug",
            BottomTool::Problems => "Problems",
            BottomTool::Git => "Git",
            BottomTool::Services => "Services",
            BottomTool::Find => "Find",
        }
    }
    fn stage(self) -> &'static str {
        match self {
            BottomTool::Terminal => "",
            BottomTool::Run => "Nothing is running. Run a file with Ctrl+Shift+F10, the selected configuration with Shift+F10.",
            BottomTool::Debug => "Stage 2: debugpy.",
            BottomTool::Problems => "Stage 2: ruff + ty diagnostics.",
            BottomTool::Git => "",
            BottomTool::Services => "Stage 5: Docker and databases.",
            BottomTool::Find => "",
        }
    }
}

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
enum PopupKind {
    SearchEverywhere(SeTab),
    FindAction,
    RecentFiles,
    GotoLine,
    Branches,
    NewBranch,
    VcsOperations,
    Confirm,
    /// Run configurations; `true` runs the picked one (Alt+Shift+F10), `false` only selects it.
    RunConfigs(bool),
    Interpreters,
    /// Alt+Enter: quick fixes for the problem at the caret.
    Intentions,
    /// Shift+F6: the new name.
    Rename,
    /// The project widget: New Project…, Open…, recent projects.
    Projects,
    /// "Open Project": This Window / New Window.
    OpenWhere,
    /// New / rename / copy / move in the Project view: the name or target path.
    FileName,
    /// Delete confirmation.
    FileDelete,
}

#[derive(Clone)]
pub enum Tab {
    Editor(Entity<Editor>),
    Diff(Entity<DiffView>),
    Grid(Entity<GridView>),
}

impl Tab {
    fn focus(&self, cx: &App) -> FocusHandle {
        match self {
            Tab::Editor(e) => e.read(cx).focus_handle(cx),
            Tab::Diff(d) => d.read(cx).focus_handle(cx),
            Tab::Grid(g) => g.read(cx).focus_handle(cx),
        }
    }
}

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
enum SeTab {
    All,
    Classes,
    Files,
    Symbols,
    Actions,
}

impl SeTab {
    const ORDER: [SeTab; 5] = [SeTab::All, SeTab::Classes, SeTab::Files, SeTab::Symbols, SeTab::Actions];

    fn title(self) -> &'static str {
        match self {
            SeTab::All => "All",
            SeTab::Classes => "Classes",
            SeTab::Files => "Files",
            SeTab::Symbols => "Symbols",
            SeTab::Actions => "Actions",
        }
    }

    fn next(self) -> SeTab {
        let i = Self::ORDER.iter().position(|t| *t == self).unwrap_or(0);
        Self::ORDER[(i + 1) % Self::ORDER.len()]
    }
}

#[derive(Clone)]
enum Target {
    File(PathBuf),
    Action(Action),
    Line(usize),
    Branch(brisk_git::Branch),
    NewBranch,
    Confirm(git_ui::Pending),
    Cancel,
    RunConfig(Option<String>, bool),
    EditRunConfigs,
    Interpreter(PathBuf),
    Intention(Box<problems_ui::Intent>),
    Rename(String),
    Project(project_ui::ProjectCmd),
    /// A class or function from the symbol index: file, line, column.
    Symbol(PathBuf, usize, usize, brisk_syntax::OutlineKind),
    /// Run the pending file operation with this target.
    FileDo(PathBuf),
}

#[derive(Clone)]
struct PopupItem {
    label: String,
    detail: String,
    hint: String,
    target: Target,
}

struct Popup {
    kind: PopupKind,
    query: String,
    selected: usize,
    items: Vec<PopupItem>,
}

pub struct Workspace {
    root: PathBuf,
    branch: Option<String>,
    runs: run_ui::Runs,
    problems: problems_ui::Problems,
    lsp: lsp_ui::Lsp,
    nav: lsp_ui::Nav,
    symbols: project_ui::Symbols,
    /// The open context menu.
    menu: Option<menu_ui::ContextMenu>,
    /// The file operation waiting for a name or a confirmation.
    file_op: Option<files_ui::FileOp>,
    /// The folder waiting for the This Window / New Window choice.
    project_pending: Option<PathBuf>,
    tree: ProjectTree,
    tree_selected: Option<NodeId>,
    tree_scroll: UniformListScrollHandle,
    files: Option<Arc<FileList>>,
    tabs: Vec<Tab>,
    active: Option<usize>,
    git: Option<git_ui::Git>,
    /// Commit window state exists even without a repo (it offers "Create Git Repository").
    commit_focus: FocusHandle,
    toast: Option<(String, bool)>,
    toast_gen: u64,
    find: find_ui::Find,
    db: db_ui::Db,
    recent: Vec<PathBuf>,
    /// Expanded folders' modification times (the disk poll).
    dir_stamps: disk_ui::DirStamps,
    left: Option<LeftTool>,
    bottom: Option<BottomTool>,
    popup: Option<Popup>,
    /// Created on first Alt+F12 — no shell process until the user asks for one.
    terminal: Option<Entity<TerminalPanel>>,
    resolver: KeyResolver,
    focus: FocusHandle,
    tree_focus: FocusHandle,
    shift_alone: bool,
    status: String,
    started: Instant,
}

impl Workspace {
    pub fn new(root: PathBuf, window: &mut Window, cx: &mut Context<Self>) -> Self {
        let branch = std::fs::read_to_string(root.join(".git/HEAD"))
            .ok()
            .map(|h| h.trim().strip_prefix("ref: refs/heads/").map(str::to_string).unwrap_or_else(|| h.trim().chars().take(8).collect()));
        let (runs, imported_runs) = run_ui::Runs::new(&root);
        if imported_runs > 0 {
            cx.defer_in(window, move |this: &mut Self, _, cx| this.show_toast(format!("Imported {imported_runs} run configuration(s) from .idea"), false, cx));
        }
        cx.defer_in(window, |this: &mut Self, _, cx| this.start_interpreter_discovery(cx));
        if let Some(f) = std::env::var_os("BRISK_SMOKE_LINT") {
            let f = PathBuf::from(f);
            cx.defer_in(window, move |this: &mut Self, window, cx| this.lint_smoke(f, window, cx));
        }
        if let Some(f) = std::env::var_os("BRISK_SMOKE_LSP") {
            let f = PathBuf::from(f);
            cx.defer_in(window, move |this: &mut Self, window, cx| this.lsp_smoke(f, window, cx));
        }
        if let Some(f) = std::env::var_os("BRISK_SMOKE_EDIT") {
            let f = PathBuf::from(f);
            cx.defer_in(window, move |this: &mut Self, window, cx| this.edit_smoke(f, window, cx));
        }
        if std::env::var_os("BRISK_SMOKE_EDIT2").is_some() {
            cx.defer_in(window, |this: &mut Self, window, cx| this.edit2_smoke(window, cx));
        }
        if std::env::var_os("BRISK_SMOKE_DISK").is_some() {
            cx.defer_in(window, |this: &mut Self, window, cx| this.disk_smoke(window, cx));
        }
        if std::env::var_os("BRISK_SMOKE_UI").is_some() {
            cx.defer_in(window, |this: &mut Self, window, cx| this.ui_smoke(window, cx));
        }
        if let Some(f) = std::env::var_os("BRISK_SMOKE_RUN") {
            let f = PathBuf::from(f);
            cx.defer_in(window, move |this: &mut Self, window, cx| this.run_smoke(f, window, cx));
        }
        let tree = ProjectTree::new(&root);
        let focus = cx.focus_handle();
        let tree_focus = cx.focus_handle();

        // Index files off the UI thread.
        let scan_root = root.clone();
        let task = cx.background_executor().spawn(async move { FileList::scan(&scan_root) });
        cx.spawn(async move |this, cx| {
            let list = task.await;
            let n = list.files.len();
            let _ = this.update(cx, |this, cx| {
                this.files = Some(Arc::new(list));
                this.ensure_symbols(cx);
                this.status = format!("Indexed {n} files in {:.0?}", this.started.elapsed());
                this.refresh_popup();
                if this.find.open {
                    this.schedule_search(cx);
                }
                cx.notify();
            });
        })
        .detach();

        // Autosave when the window loses focus (PyCharm "Save files when switching to a different application").
        cx.observe_window_activation(window, |this: &mut Self, window, cx| {
            if !window.is_window_active() {
                this.save_all(cx);
            } else {
                // Back from another app: pick up external edits and git changes.
                this.reload_editors(cx);
                this.refresh_git(cx);
            }
        })
        .detach();
        let (db, imported) = db_ui::Db::new(&root, cx);
        if imported {
            let n = db.sources.len();
            cx.defer_in(window, move |this: &mut Self, _, cx| this.show_toast(format!("Imported {n} data source(s) from .idea/dataSources.xml"), false, cx));
        }
        if let Some(sql) = std::env::var_os("BRISK_SMOKE_DB") {
            let sql = sql.to_string_lossy().into_owned();
            cx.defer_in(window, move |this: &mut Self, window, cx| this.db_smoke(sql, window, cx));
        }
        let git = brisk_git::Repo::discover(&root).map(|repo| git_ui::Git::new(repo, cx));
        cx.defer_in(window, |this: &mut Self, _, cx| this.refresh_git(cx));

        window.focus(&tree_focus);
        cx.defer_in(window, |this: &mut Self, window, cx| this.start_disk_poll(window, cx));
        // Closing the window saves everything first (PyCharm never asks).
        let me = cx.entity().downgrade();
        window.on_window_should_close(cx, move |_, cx| {
            let _ = me.update(cx, |this, cx| this.save_all(cx));
            true
        });
        if std::env::var_os("BRISK_OPEN_TERMINAL").is_some() {
            cx.defer_in(window, |this: &mut Self, window, cx| this.toggle_terminal(window, cx));
        }
        // Smoke test hook: Find in Files for a query, then show results in the Find window.
        if let Some(q) = std::env::var_os("BRISK_SMOKE_FIND") {
            let q = q.to_string_lossy().into_owned();
            cx.defer_in(window, move |this: &mut Self, window, cx| {
                this.find.set_query_for_smoke(&q);
                this.open_find(false, window, cx);
                let bg = cx.background_executor().clone();
                let t0 = Instant::now();
                cx.spawn_in(window, async move |this, cx| {
                    for _ in 0..120 {
                        bg.timer(std::time::Duration::from_millis(250)).await;
                        let done = this.update(cx, |this, _| this.files.is_some() && this.find.idle()).unwrap_or(true);
                        if done {
                            break;
                        }
                    }
                    let _ = this.update_in(cx, |this, window, cx| {
                        eprintln!("smoke-find: {} (since start {:.1?}, files indexed {})", this.find.summary(), t0.elapsed(), this.files.as_ref().map_or(0, |f| f.files.len()));
                        this.dispatch_find_window_for_smoke(window, cx);
                    });
                })
                .detach();
            });
        }
        // Smoke test hook: open a file and exercise the git windows (used by scripts, no UI driving).
        if let Some(f) = std::env::var_os("BRISK_SMOKE_GIT") {
            let f = PathBuf::from(f);
            cx.defer_in(window, move |this: &mut Self, window, cx| {
                this.open_file(&f, window, cx);
                this.dispatch(Action::ToggleCommitWindow, window, cx);
                this.dispatch(Action::ToggleGitWindow, window, cx);
                this.focus_editor(window, cx);
                this.dispatch(Action::ShowDiff, window, cx);
                this.dispatch(Action::Branches, window, cx);
            });
        }
        Self {
            root,
            branch,
            runs,
            problems: problems_ui::Problems::new(cx),
            lsp: lsp_ui::Lsp::new(),
            nav: lsp_ui::Nav::default(),
            symbols: project_ui::Symbols::default(),
            menu: None,
            file_op: None,
            project_pending: None,
            tree,
            tree_selected: Some(0),
            tree_scroll: UniformListScrollHandle::new(),
            files: None,
            tabs: Vec::new(),
            active: None,
            git,
            commit_focus: cx.focus_handle(),
            toast: None,
            toast_gen: 0,
            find: find_ui::Find::new(),
            db,
            recent: Vec::new(),
            dir_stamps: Default::default(),
            left: Some(LeftTool::Project),
            bottom: None,
            popup: None,
            terminal: None,
            resolver: KeyResolver::new(pycharm_keymap()),
            focus,
            tree_focus,
            shift_alone: false,
            status: "Indexing…".into(),
            started: Instant::now(),
        }
    }

    fn project_name(&self) -> String {
        self.root.file_name().map(|n| n.to_string_lossy().into_owned()).unwrap_or_else(|| self.root.display().to_string())
    }

    fn active_editor(&self) -> Option<&Entity<Editor>> {
        match self.active.and_then(|i| self.tabs.get(i)) {
            Some(Tab::Editor(e)) => Some(e),
            _ => None,
        }
    }

    fn editors(&self) -> impl Iterator<Item = &Entity<Editor>> {
        self.tabs.iter().filter_map(|t| match t {
            Tab::Editor(e) => Some(e),
            _ => None,
        })
    }

    /// The editor that has keyboard focus: the active tab or the commit message field.
    fn focused_editor(&self, window: &Window, cx: &App) -> Option<Entity<Editor>> {
        if let Some(e) = self.active_editor() {
            if e.read(cx).focus_handle(cx).is_focused(window) {
                return Some(e.clone());
            }
            if let Some(f) = e.read(cx).find_field_focused(window, cx) {
                return Some(f);
            }
        }
        self.git.as_ref().map(|g| g.message.clone()).filter(|m| m.read(cx).focus_handle(cx).is_focused(window))
    }

    fn save_all(&mut self, cx: &mut Context<Self>) {
        let mut saved = 0;
        let mut problem = None;
        let eds: Vec<_> = self.editors().cloned().collect();
        for t in eds {
            t.update(cx, |e, cx| {
                match e.save() {
                    Ok(true) => saved += 1,
                    Ok(false) => {}
                    Err(err) => problem = Some(format!("Cannot save {}: {err}", e.title())),
                }
                if let Some(n) = e.encoding_note.take() {
                    problem = Some(format!("{}: {n}", e.title()));
                }
                if e.disk_conflict.is_some() {
                    cx.notify();
                }
            });
        }
        if let Some(p) = problem {
            self.status = p.clone();
            self.show_toast(p, true, cx);
        }
        if saved > 0 {
            self.symbols.stale = true;
            self.status = format!("Saved {saved} file(s)");
            self.refresh_git(cx);
            if self.bottom == Some(BottomTool::Problems) && self.problems.tab == problems_ui::ProblemsTab::Project {
                self.check_project(cx);
            }
        }
    }

    fn reload_editors(&mut self, cx: &mut Context<Self>) {
        let eds: Vec<_> = self.editors().cloned().collect();
        for e in eds {
            e.update(cx, |e, cx| {
                if e.reload_from_disk() {
                    cx.notify();
                }
            });
        }
    }

    pub fn open_file(&mut self, path: &Path, window: &mut Window, cx: &mut Context<Self>) {
        self.save_all(cx);
        let path = path.to_path_buf();
        self.recent.retain(|p| *p != path);
        self.recent.insert(0, path.clone());
        self.recent.truncate(50);
        if let Some(i) = self.tabs.iter().position(|t| matches!(t, Tab::Editor(e) if e.read(cx).path.as_deref() == Some(path.as_path()))) {
            self.active = Some(i);
        } else {
            let ed = cx.new(|cx| Editor::open(&path, cx));
            self.load_vcs_base(ed.clone(), cx);
            self.watch_editor(&ed, window, cx);
            self.add_tab(Tab::Editor(ed));
        }
        if let Some(e) = self.active_editor() {
            let f = e.read(cx).focus_handle(cx);
            window.focus(&f);
        }
        window.set_window_title(&format!("{} – {}", self.project_name(), path.file_name().map(|n| n.to_string_lossy()).unwrap_or_default()));
        cx.notify();
    }

    fn add_tab(&mut self, tab: Tab) {
        let insert_at = self.active.map(|i| i + 1).unwrap_or(self.tabs.len()).min(self.tabs.len());
        self.tabs.insert(insert_at, tab);
        self.active = Some(insert_at);
    }

    fn close_tab(&mut self, i: usize, window: &mut Window, cx: &mut Context<Self>) {
        if i >= self.tabs.len() {
            return;
        }
        if let Tab::Editor(e) = &self.tabs[i] {
            let saved = e.update(cx, |e, _| e.save());
            let ed = e.read(cx);
            if ed.buffer.is_dirty() && ed.read_error.is_none() && ed.path.is_some() {
                // Not written (conflict with the disk, or an error): closing would lose the edits.
                let msg = match saved {
                    Err(err) => format!("Cannot save {}: {err}", ed.title()),
                    Ok(_) => format!("{} changed on disk: choose Reload or Keep Mine first", ed.title()),
                };
                self.active = Some(i);
                self.show_toast(msg, true, cx);
                cx.notify();
                return;
            }
            if let Some(p) = e.read(cx).path.clone() {
                self.ty_close(&p);
            }
        }
        self.tabs.remove(i);
        self.active = if self.tabs.is_empty() { None } else { Some(i.min(self.tabs.len() - 1)) };
        match self.active.and_then(|i| self.tabs.get(i)) {
            Some(t) => {
                let f = t.focus(cx);
                window.focus(&f);
            }
            None => window.focus(&self.tree_focus),
        }
        cx.notify();
    }

    fn focus_editor(&self, window: &mut Window, cx: &mut Context<Self>) {
        if let Some(t) = self.active.and_then(|i| self.tabs.get(i)) {
            let f = t.focus(cx);
            window.focus(&f);
        }
    }

    fn toggle_left(&mut self, tool: LeftTool, window: &mut Window, cx: &mut Context<Self>) {
        if self.left == Some(tool) && (tool != LeftTool::Project || self.tree_focus.is_focused(window)) {
            self.left = None;
            self.focus_editor(window, cx);
        } else {
            self.left = Some(tool);
            if tool == LeftTool::Project {
                window.focus(&self.tree_focus);
            }
        }
    }

    fn terminal_focused(&self, window: &Window, cx: &App) -> bool {
        self.terminal.as_ref().is_some_and(|t| t.read(cx).focus_handle(cx).is_focused(window))
    }

    /// Alt+F12, PyCharm semantics: open+focus; if open but unfocused, focus; if focused, hide.
    fn toggle_terminal(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        if self.bottom == Some(BottomTool::Terminal) && self.terminal_focused(window, cx) {
            self.bottom = None;
            self.focus_editor(window, cx);
            return;
        }
        let root = self.root.clone();
        let term = match &self.terminal {
            Some(t) if !t.read(cx).is_empty() => t.clone(),
            _ => {
                let t = cx.new(|cx| TerminalPanel::new(root, cx));
                // Hide the tool window when the last shell exits.
                cx.observe(&t, |this: &mut Self, t, cx| {
                    if t.read(cx).is_empty() && this.bottom == Some(BottomTool::Terminal) {
                        this.bottom = None;
                        this.terminal = None;
                        cx.notify();
                    }
                })
                .detach();
                self.terminal = Some(t.clone());
                t
            }
        };
        self.bottom = Some(BottomTool::Terminal);
        let f = term.read(cx).focus_handle(cx);
        window.focus(&f);
    }

    fn toggle_bottom(&mut self, tool: BottomTool, window: &mut Window, cx: &mut Context<Self>) {
        if tool == BottomTool::Terminal {
            return self.toggle_terminal(window, cx);
        }
        if self.bottom == Some(tool) {
            self.bottom = None;
            self.focus_editor(window, cx);
        } else {
            self.bottom = Some(tool);
        }
    }

    // --- Key routing ---------------------------------------------------------

    fn contexts(&self, window: &Window, cx: &App) -> Vec<&'static str> {
        if let Some(e) = self.focused_editor(window, cx) {
            if e.read(cx).is_sql() {
                vec!["SqlConsole", "Editor"]
            } else {
                vec!["Editor"]
            }
        } else if self.db.focus.is_focused(window) {
            vec!["DatabaseView", "ToolWindow"]
        } else if self.focused_grid(window, cx).is_some() {
            vec!["DataGrid"]
        } else if self.tree_focus.is_focused(window) {
            vec!["ProjectView", "ToolWindow"]
        } else if self.commit_focus.is_focused(window) {
            vec!["CommitView", "ToolWindow"]
        } else if self.git.as_ref().is_some_and(|g| g.log_focus.is_focused(window)) {
            vec!["GitLog", "ToolWindow"]
        } else if self.find_window_focused(window) {
            vec!["FindView", "ToolWindow"]
        } else if self.problems_focused(window) {
            vec!["ProblemsView", "ToolWindow"]
        } else if self.active_diff(window, cx).is_some() {
            vec!["DiffView"]
        } else {
            vec![]
        }
    }

    fn active_diff(&self, window: &Window, cx: &App) -> Option<Entity<DiffView>> {
        match self.active.and_then(|i| self.tabs.get(i)) {
            Some(Tab::Diff(d)) if d.read(cx).focus_handle(cx).is_focused(window) => Some(d.clone()),
            _ => None,
        }
    }

    fn on_key_down(&mut self, e: &KeyDownEvent, window: &mut Window, cx: &mut Context<Self>) {
        self.shift_alone = false;
        if self.menu.is_some() {
            if !e.keystroke.modifiers.modified() || e.keystroke.key == "escape" {
                self.menu_key(e, window, cx);
                cx.stop_propagation();
                return;
            }
            self.menu = None;
        }
        if self.db.dialog.is_some() {
            self.db_dialog_key(e, window, cx);
            cx.stop_propagation();
            return;
        }
        if self.find.open {
            self.find_key(e, window, cx);
            cx.stop_propagation();
            return;
        }
        if self.popup.is_some() {
            self.popup_key(e, window, cx);
            cx.stop_propagation();
            return;
        }
        let stroke = keys::convert(&e.keystroke);
        if stroke.is_modifier_only() {
            return;
        }
        if self.tree_focus.is_focused(window) && self.tree_file_key(&e.keystroke, cx) {
            cx.stop_propagation();
            return;
        }
        if self.terminal_focused(window, cx) {
            let t = self.terminal.clone();
            self.terminal_key(t, &["Terminal"], e, stroke, window, cx);
            cx.stop_propagation();
            return;
        }
        if self.run_panel_focused(window, cx) {
            let t = self.runs.panel.clone();
            self.terminal_key(t, &["RunConsole", "Terminal"], e, stroke, window, cx);
            cx.stop_propagation();
            return;
        }
        if self.git_pre_key(e, window, cx) {
            cx.stop_propagation();
            return;
        }
        // The search bar handles Enter, Esc, F3, Tab and its toggles typed in its fields.
        if let Some(ed) = self.active_editor().cloned() {
            if ed.read(cx).find_field_focused(window, cx).is_some() && ed.update(cx, |ed, cx| ed.find_key(&e.keystroke, window, cx)) {
                cx.stop_propagation();
                return;
            }
        }
        // The completion list takes arrows, Enter, Tab and Esc while it is open.
        let m = &e.keystroke.modifiers;
        if !(m.control || m.alt || m.shift || m.platform) {
            if let Some(ed) = self.focused_editor(window, cx).filter(|ed| ed.read(cx).completion_open()) {
                let key = e.keystroke.key.clone();
                if ed.update(cx, |ed, cx| ed.completion_key(&key, cx)) {
                    cx.stop_propagation();
                    return;
                }
            }
        }
        let ctx = self.contexts(window, cx);
        match self.resolver.key_down(stroke, &ctx) {
            Resolution::Action(a) => {
                cx.stop_propagation();
                self.dispatch(a, window, cx);
            }
            Resolution::Pending => cx.stop_propagation(),
            Resolution::Unhandled => {
                let handled = match ctx.first() {
                    Some(&"ProjectView") => self.tree_key(&e.keystroke.key, window, cx),
                    Some(&"CommitView") => self.commit_key(&e.keystroke, window, cx),
                    Some(&"GitLog") => self.log_key(&e.keystroke, window, cx),
                    Some(&"FindView") => self.find_window_key(&e.keystroke, window, cx),
                    Some(&"ProblemsView") => self.problems_key(&e.keystroke, window, cx),
                    Some(&"DatabaseView") => self.db_tree_key(&e.keystroke, window, cx),
                    Some(&"DataGrid") => match self.focused_grid(window, cx) {
                        Some(g) => g.update(cx, |g, cx| g.key(&e.keystroke, cx)),
                        None => false,
                    },
                    _ => false,
                };
                if handled {
                    cx.stop_propagation();
                }
                // Otherwise the platform turns it into text for the focused editor.
                let _ = keys::is_text_input(&e.keystroke);
            }
        }
    }

    /// PyCharm's "Override IDE shortcuts" mode: the shell gets the keys, except a short list of
    /// IDE actions (tool windows, Find Action, run/debug). Esc returns to the editor unless a
    /// full-screen app (vim, less, htop) is running.
    fn terminal_key(&mut self, term: Option<Entity<TerminalPanel>>, ctx: &[&str], e: &KeyDownEvent, stroke: brisk_keymap::Keystroke, window: &mut Window, cx: &mut Context<Self>) {
        use Action as A;
        const IDE_ACTIONS: &[Action] = &[
            A::ToggleProjectWindow, A::ToggleBookmarksWindow, A::ToggleFindWindow, A::ToggleRunWindow,
            A::ToggleDebugWindow, A::ToggleProblemsWindow, A::ToggleStructureWindow, A::ToggleServicesWindow,
            A::ToggleGitWindow, A::ToggleCommitWindow, A::ToggleTerminalWindow, A::HideAllToolWindows,
            A::HideActiveToolWindow, A::FindAction, A::Settings, A::Run, A::Debug, A::RunContext,
            A::DebugContext, A::Stop, A::FindInFiles, A::SaveAll, A::ToggleDatabaseWindow, A::Rerun,
            A::SelectRunConfiguration,
        ];
        let Some(term) = term else { return };
        let k = &e.keystroke;
        let plain_esc = k.key == "escape" && !k.modifiers.shift && !k.modifiers.control && !k.modifiers.alt;
        if plain_esc && !term.read(cx).alt_screen() {
            self.focus_editor(window, cx);
            cx.notify();
            return;
        }
        if let Resolution::Action(a) = self.resolver.key_down(stroke, ctx) {
            if IDE_ACTIONS.contains(&a) {
                self.dispatch(a, window, cx);
                return;
            }
        }
        self.resolver.cancel();
        term.update(cx, |t, cx| t.key(k, cx));
    }

    fn on_modifiers(&mut self, e: &ModifiersChangedEvent, window: &mut Window, cx: &mut Context<Self>) {
        let m = e.modifiers;
        let only_shift = m.shift && !m.control && !m.alt && !m.platform;
        if only_shift {
            self.shift_alone = true;
        } else if !m.shift && !m.control && !m.alt && !m.platform && self.shift_alone {
            self.shift_alone = false;
            if let Resolution::Action(a) = self.resolver.modifier_tapped("shift", Instant::now()) {
                self.dispatch(a, window, cx);
            }
        } else {
            self.shift_alone = false;
        }
    }

    fn dispatch(&mut self, action: Action, window: &mut Window, cx: &mut Context<Self>) {
        use Action as A;
        if self.lsp_action(action, window, cx) {
            cx.notify();
            return;
        }
        if matches!(action, A::SearchEverywhere | A::GotoFile | A::GotoClass | A::GotoSymbol) {
            self.ensure_symbols(cx);
        }
        match action {
            A::SearchEverywhere => self.open_popup(PopupKind::SearchEverywhere(SeTab::All)),
            A::GotoFile => self.open_popup(PopupKind::SearchEverywhere(SeTab::Files)),
            A::GotoClass => self.open_popup(PopupKind::SearchEverywhere(SeTab::Classes)),
            A::GotoSymbol => self.open_popup(PopupKind::SearchEverywhere(SeTab::Symbols)),
            A::OpenProject => self.project_cmd(project_ui::ProjectCmd::Open, window, cx),
            A::NewProject => self.project_cmd(project_ui::ProjectCmd::New, window, cx),
            A::RecentProjects => self.open_popup(PopupKind::Projects),
            A::FindAction => self.open_popup(PopupKind::FindAction),
            A::RecentFiles => self.open_popup(PopupKind::RecentFiles),
            A::GotoLine if self.active.is_some() => self.open_popup(PopupKind::GotoLine),
            A::ToggleProjectWindow => self.toggle_left(LeftTool::Project, window, cx),
            A::ToggleCommitWindow => {
                self.toggle_left(LeftTool::Commit, window, cx);
                if self.left == Some(LeftTool::Commit) {
                    window.focus(&self.commit_focus);
                    self.refresh_git(cx);
                }
            }
            A::Commit | A::Push | A::UpdateProject | A::VcsQuickList | A::Branches | A::ShowDiff | A::RollbackChanges => {
                self.git_action(action, window, cx)
            }
            A::ToggleStructureWindow => self.toggle_left(LeftTool::Structure, window, cx),
            A::ToggleBookmarksWindow => self.toggle_left(LeftTool::Bookmarks, window, cx),
            A::ToggleTerminalWindow => self.toggle_bottom(BottomTool::Terminal, window, cx),
            A::ToggleRunWindow | A::Run | A::RunContext | A::Rerun | A::SelectRunConfiguration | A::EditRunConfigurations => {
                self.run_action(action, window, cx)
            }
            A::ToggleDebugWindow => self.toggle_bottom(BottomTool::Debug, window, cx),
            A::ToggleProblemsWindow => self.toggle_problems(window, cx),
            A::NextError | A::PreviousError => self.goto_problem(action == A::NextError, window, cx),
            A::ShowIntentionActions => self.show_intentions(window, cx),
            A::ToggleGitWindow => {
                self.toggle_bottom(BottomTool::Git, window, cx);
                if self.bottom == Some(BottomTool::Git) {
                    if let Some(g) = &self.git {
                        window.focus(&g.log_focus);
                    }
                    self.refresh_git(cx);
                }
            }
            A::ToggleServicesWindow => {
                self.toggle_bottom(BottomTool::Services, window, cx);
                if self.bottom == Some(BottomTool::Services) {
                    if let Some(g) = &self.db.results {
                        let f = g.read(cx).focus_handle(cx);
                        window.focus(&f);
                    }
                }
            }
            A::ToggleDatabaseWindow | A::NewDataSource | A::DataSourceProperties | A::OpenQueryConsole | A::EditTableData
            | A::RefreshDatabase | A::Disconnect | A::ExecuteQuery => self.db_action(action, window, cx),
            A::Stop if self.db.running.is_some() => {
                if let Some(c) = &self.db.running {
                    c.cancel();
                }
                self.status = "Cancelling the running statement…".into();
            }
            A::Stop => self.run_action(action, window, cx),
            A::Copy if self.focused_grid(window, cx).is_some() => {
                if let Some(g) = self.focused_grid(window, cx) {
                    g.update(cx, |g, cx| g.copy(cx));
                }
            }
            A::ToggleFindWindow => self.toggle_find_window(window, cx),
            A::FindInFiles => self.open_find(false, window, cx),
            A::ReplaceInFiles => self.open_find(true, window, cx),
            A::HideAllToolWindows => {
                if self.left.is_some() || self.bottom.is_some() || self.db.open {
                    self.left = None;
                    self.bottom = None;
                    self.db.open = false;
                } else {
                    self.left = Some(LeftTool::Project);
                }
                self.focus_editor(window, cx);
            }
            A::HideActiveToolWindow => {
                if self.terminal_focused(window, cx) || self.run_panel_focused(window, cx) {
                    self.bottom = None;
                } else if self.db.focus.is_focused(window) {
                    self.db.open = false;
                } else if self.tree_focus.is_focused(window) || self.commit_focus.is_focused(window) || self.git.as_ref().is_some_and(|g| g.message.read(cx).focus_handle(cx).is_focused(window)) {
                    self.left = None;
                } else {
                    self.bottom = None;
                }
                self.focus_editor(window, cx);
            }
            A::FocusEditor => self.focus_editor(window, cx),
            A::SaveAll => {
                self.save_all(cx);
                if self.status.is_empty() {
                    self.status = "All files are up to date".into();
                }
            }
            A::CloseTab => {
                if let Some(i) = self.active {
                    self.close_tab(i, window, cx);
                }
            }
            A::NextTab | A::PreviousTab => {
                if let Some(i) = self.active {
                    let n = self.tabs.len();
                    self.save_all(cx);
                    self.active = Some(if action == A::NextTab { (i + 1) % n } else { (i + n - 1) % n });
                    self.focus_editor(window, cx);
                }
            }
            A::SelectIn => {
                if let Some(p) = self.active_editor().and_then(|e| e.read(cx).path.clone()) {
                    self.left = Some(LeftTool::Project);
                    if let Some(id) = self.tree.reveal(&p) {
                        self.tree_selected = Some(id);
                        if let Some(ix) = self.tree.rows().iter().position(|r| r.id == id) {
                            self.tree_scroll.scroll_to_item(ix, gpui::ScrollStrategy::Center);
                        }
                    }
                    window.focus(&self.tree_focus);
                }
            }
            A::NewElement | A::Settings | A::ProjectStructure => {
                self.status = format!("{} — coming soon", action.info().title);
            }
            _ => {
                let target = self.focused_editor(window, cx).or_else(|| {
                    matches!(action, A::Undo | A::Redo | A::Copy | A::Paste | A::SelectAll | A::CommentLine)
                        .then(|| self.active_editor().cloned())
                        .flatten()
                });
                let handled = match target {
                    Some(ed) => ed.update(cx, |e, cx| e.perform(action, window, cx)),
                    None => false,
                };
                if !handled {
                    let sc = self.resolver.keymap().shortcut_label(action).map(|s| format!(" ({s})")).unwrap_or_default();
                    self.status = format!("{}{sc} — not implemented yet", action.info().title);
                }
            }
        }
        cx.notify();
    }

    // --- Project tree ----------------------------------------------------------

    fn tree_key(&mut self, key: &str, window: &mut Window, cx: &mut Context<Self>) -> bool {
        let rows: Vec<_> = self.tree.rows().to_vec();
        let cur = self.tree_selected.and_then(|id| rows.iter().position(|r| r.id == id)).unwrap_or(0);
        let id = rows.get(cur).map(|r| r.id).unwrap_or(0);
        match key {
            "up" => self.tree_selected = rows.get(cur.saturating_sub(1)).map(|r| r.id),
            "down" => self.tree_selected = rows.get((cur + 1).min(rows.len().saturating_sub(1))).map(|r| r.id),
            "right" => {
                if self.tree.nodes[id].is_dir {
                    if self.tree.nodes[id].expanded {
                        self.tree_selected = rows.get(cur + 1).map(|r| r.id).or(Some(id));
                    } else {
                        self.tree.expand(id);
                    }
                }
            }
            "left" => {
                if self.tree.nodes[id].is_dir && self.tree.nodes[id].expanded {
                    self.tree.collapse(id);
                } else if let Some(p) = self.tree.nodes[id].parent {
                    self.tree_selected = Some(p);
                }
            }
            "enter" | "f4" => {
                if self.tree.nodes[id].is_dir {
                    self.tree.toggle(id);
                } else {
                    let p = self.tree.nodes[id].path.clone();
                    self.open_file(&p, window, cx);
                }
            }
            _ => return false,
        }
        if let Some(sel) = self.tree_selected {
            if let Some(ix) = self.tree.rows().iter().position(|r| r.id == sel) {
                self.tree_scroll.scroll_to_item(ix, gpui::ScrollStrategy::Top);
            }
        }
        cx.notify();
        true
    }

    fn tree_click(&mut self, id: NodeId, e: &ClickEvent, window: &mut Window, cx: &mut Context<Self>) {
        self.tree_selected = Some(id);
        window.focus(&self.tree_focus);
        let double = e.click_count() >= 2;
        if self.tree.nodes[id].is_dir {
            if double || e.position().x < px(0.) {
                self.tree.toggle(id);
            }
        } else if double {
            let p = self.tree.nodes[id].path.clone();
            self.open_file(&p, window, cx);
        }
        cx.notify();
    }

    // --- Popups ----------------------------------------------------------------

    fn open_popup(&mut self, kind: PopupKind) {
        // In an open Search Everywhere another tab's shortcut switches to that tab; the same
        // shortcut again goes to the next tab, like PyCharm.
        if let (Some(p), PopupKind::SearchEverywhere(want)) = (&mut self.popup, kind) {
            if let PopupKind::SearchEverywhere(t) = p.kind {
                p.kind = PopupKind::SearchEverywhere(if want == t { t.next() } else { want });
                p.selected = 0;
                self.refresh_popup();
                return;
            }
        }
        self.popup = Some(Popup { kind, query: String::new(), selected: 0, items: Vec::new() });
        self.refresh_popup();
    }

    /// Clicking a Search Everywhere tab or pressing Tab in it.
    fn set_se_tab(&mut self, tab: SeTab) {
        if let Some(p) = self.popup.as_mut() {
            p.kind = PopupKind::SearchEverywhere(tab);
            p.selected = 0;
            self.refresh_popup();
        }
    }

    fn action_items(&self, q: &str, limit: usize) -> Vec<PopupItem> {
        let ql = q.to_lowercase();
        let mut scored: Vec<(i32, Action)> = ALL_ACTIONS
            .iter()
            .filter_map(|&a| {
                let title = a.info().title.to_lowercase();
                if ql.is_empty() {
                    return Some((0, a));
                }
                if let Some(p) = title.find(&ql) {
                    return Some((1000 - p as i32 * 10 - title.len() as i32, a));
                }
                // Subsequence match on word starts ("gtd" -> Go To Declaration).
                let mut it = ql.chars().peekable();
                for c in title.chars() {
                    if it.peek() == Some(&c) {
                        it.next();
                    }
                }
                it.peek().is_none().then(|| (100 - title.len() as i32, a))
            })
            .collect();
        scored.sort_by(|a, b| b.0.cmp(&a.0));
        scored
            .into_iter()
            .take(limit)
            .map(|(_, a)| PopupItem {
                label: a.info().title.to_string(),
                detail: a.info().group.to_string(),
                hint: self.resolver.keymap().shortcut_label(a).unwrap_or_default(),
                target: Target::Action(a),
            })
            .collect()
    }

    fn file_items(&self, q: &str, limit: usize) -> Vec<PopupItem> {
        let Some(files) = &self.files else { return Vec::new() };
        if q.is_empty() {
            return self.recent.iter().take(limit).map(|p| self.file_item(p)).collect();
        }
        files.search(q, limit).into_iter().map(|(i, _)| self.file_item(&files.abs(i))).collect()
    }

    fn file_item(&self, p: &Path) -> PopupItem {
        let rel = p.strip_prefix(&self.root).unwrap_or(p);
        let dir = rel.parent().map(|d| d.to_string_lossy().replace('\\', "/")).unwrap_or_default();
        PopupItem {
            label: p.file_name().map(|n| n.to_string_lossy().into_owned()).unwrap_or_default(),
            detail: dir,
            hint: String::new(),
            target: Target::File(p.to_path_buf()),
        }
    }

    fn refresh_popup(&mut self) {
        let Some(p) = &self.popup else { return };
        let q = p.query.clone();
        let items = match p.kind {
            PopupKind::SearchEverywhere(SeTab::All) => {
                let mut v = self.symbol_items(&q, true, 5);
                v.extend(self.file_items(&q, 30));
                if !q.is_empty() {
                    v.extend(self.symbol_items(&q, false, 8).into_iter().filter(|i| !matches!(i.target, Target::Symbol(_, _, _, brisk_syntax::OutlineKind::Class))));
                    v.extend(self.action_items(&q, 8));
                }
                v
            }
            PopupKind::SearchEverywhere(SeTab::Classes) => self.symbol_items(&q, true, 50),
            PopupKind::SearchEverywhere(SeTab::Symbols) => self.symbol_items(&q, false, 50),
            PopupKind::SearchEverywhere(SeTab::Files) => self.file_items(&q, 50),
            PopupKind::Projects => self.project_items(&q),
            PopupKind::OpenWhere => self.open_where_items(&q),
            PopupKind::FileName | PopupKind::FileDelete => self.file_op_items(&q),
            PopupKind::SearchEverywhere(SeTab::Actions) | PopupKind::FindAction => self.action_items(&q, 50),
            PopupKind::RecentFiles => {
                let ql = q.to_lowercase();
                self.recent
                    .iter()
                    .skip(if q.is_empty() && self.recent.len() > 1 { 1 } else { 0 }) // Ctrl+E then Enter = previous file
                    .filter(|p| p.to_string_lossy().to_lowercase().contains(&ql))
                    .map(|p| self.file_item(p))
                    .collect()
            }
            PopupKind::GotoLine => {
                let line = q.split(':').next().and_then(|s| s.trim().parse::<usize>().ok());
                line.map(|l| vec![PopupItem { label: format!("Go to line {l}"), detail: String::new(), hint: String::new(), target: Target::Line(l.saturating_sub(1)) }])
                    .unwrap_or_default()
            }
            k @ (PopupKind::Branches | PopupKind::NewBranch | PopupKind::VcsOperations | PopupKind::Confirm) => self.git_popup_items(k, &q),
            k @ (PopupKind::RunConfigs(_) | PopupKind::Interpreters) => self.run_popup_items(k, &q),
            PopupKind::Intentions => self.intention_items(&q),
            PopupKind::Rename => self.rename_items(&q),
        };
        let p = self.popup.as_mut().unwrap();
        p.items = items;
        p.selected = p.selected.min(p.items.len().saturating_sub(1));
    }

    fn popup_key(&mut self, e: &KeyDownEvent, window: &mut Window, cx: &mut Context<Self>) {
        let k = &e.keystroke;
        let Some(p) = self.popup.as_mut() else { return };
        match k.key.as_str() {
            "escape" => {
                self.popup = None;
                self.focus_editor(window, cx);
            }
            "up" => p.selected = p.selected.saturating_sub(1),
            "down" => p.selected = (p.selected + 1).min(p.items.len().saturating_sub(1)),
            "tab" => {
                if let PopupKind::SearchEverywhere(t) = p.kind {
                    self.set_se_tab(t.next());
                }
            }
            "enter" => {
                let target = p.items.get(p.selected).map(|i| i.target.clone());
                self.popup = None;
                match target {
                    Some(Target::File(path)) => self.open_file(&path, window, cx),
                    Some(Target::Action(a)) => {
                        self.focus_editor(window, cx);
                        self.dispatch(a, window, cx);
                    }
                    Some(Target::Line(l)) => {
                        if let Some(ed) = self.active_editor().cloned() {
                            ed.update(cx, |e, cx| e.go_to_line(l, cx));
                        }
                        self.focus_editor(window, cx);
                    }
                    Some(Target::Branch(b)) => {
                        self.focus_editor(window, cx);
                        self.checkout(b, cx);
                    }
                    Some(Target::NewBranch) => self.open_popup(PopupKind::NewBranch),
                    Some(Target::Confirm(p)) => {
                        self.focus_editor(window, cx);
                        self.run_pending(p, cx);
                    }
                    Some(Target::Cancel) => self.focus_editor(window, cx),
                    Some(t @ (Target::RunConfig(..) | Target::EditRunConfigs | Target::Interpreter(_))) => self.run_popup_pick(t, window, cx),
                    Some(Target::Intention(i)) => self.apply_intent(*i, window, cx),
                    Some(Target::Rename(n)) => self.apply_rename(n, window, cx),
                    Some(Target::Project(c)) => {
                        self.focus_editor(window, cx);
                        self.project_cmd(c, window, cx);
                    }
                    Some(Target::Symbol(path, line, col, _)) => self.open_symbol(path, line, col, window, cx),
                    Some(Target::FileDo(t)) => self.run_file_op(t, window, cx),
                    None => self.focus_editor(window, cx),
                }
            }
            "backspace" => {
                if k.modifiers.control {
                    p.query.clear();
                } else {
                    p.query.pop();
                }
                p.selected = 0;
                self.refresh_popup();
            }
            _ => {
                if !(k.modifiers.control || k.modifiers.alt || k.modifiers.platform) {
                    if let Some(ch) = &k.key_char {
                        if !ch.chars().any(|c| c.is_control()) {
                            p.query.push_str(ch);
                            p.selected = 0;
                            self.refresh_popup();
                        }
                    }
                } else if k.modifiers.control && k.key == "e" && p.kind == PopupKind::RecentFiles {
                    p.selected = (p.selected + 1).min(p.items.len().saturating_sub(1));
                } else {
                    // Re-pressing the opening shortcut (e.g. Ctrl+Shift+N) cycles tabs.
                    let stroke = keys::convert(k);
                    if let Resolution::Action(a) = self.resolver.key_down(stroke, &[]) {
                        if matches!(a, Action::GotoFile | Action::GotoClass | Action::GotoSymbol | Action::SearchEverywhere) {
                            self.dispatch(a, window, cx);
                        }
                    }
                }
            }
        }
        cx.notify();
    }

    // --- Rendering -------------------------------------------------------------

    fn stripe_button(&self, id: &'static str, glyph: &'static str, tip: &'static str, active: bool, action: Action, cx: &mut Context<Self>) -> impl IntoElement {
        let shortcut = self.resolver.keymap().shortcut_label(action).unwrap_or_default();
        let tooltip: SharedString = format!("{tip}  {shortcut}").into();
        div()
            .id(id)
            .w(px(30.))
            .h(px(30.))
            .flex()
            .items_center()
            .justify_center()
            .rounded(px(6.))
            .text_size(px(14.))
            .text_color(if active { Theme::text() } else { Theme::text_muted() })
            .when(active, |d| d.bg(Theme::selection_row()))
            .hover(|s| s.bg(Theme::hover_row()))
            .cursor_pointer()
            .on_click(cx.listener(move |this, _, window, cx| this.dispatch(action, window, cx)))
            .tooltip(move |_window, cx| cx.new(|_| Tip(tooltip.clone())).into())
            .child(glyph)
    }

    fn render_toolbar(&self, cx: &mut Context<Self>) -> impl IntoElement {
        // A toolbar widget: text plus a small dimmed chevron, vertically centered.
        let chip = |id: &'static str, text: String| {
            div()
                .id(id)
                .h(px(28.))
                .px(px(8.))
                .flex()
                .items_center()
                .gap(px(5.))
                .rounded(px(5.))
                .hover(|s| s.bg(Theme::hover_row()))
                .cursor_pointer()
                .child(text)
                .child(div().text_size(px(9.)).text_color(Theme::text_muted()).child("▼"))
        };
        div()
            .h(px(40.))
            .flex()
            .items_center()
            .px(px(10.))
            .gap(px(6.))
            .bg(Theme::panel())
            .border_b_1()
            .border_color(Theme::border())
            .text_size(px(13.))
            .text_color(Theme::text())
            .child(
                div().w(px(24.)).h(px(24.)).rounded(px(5.)).bg(Theme::accent()).flex().items_center().justify_center().text_size(px(11.)).font_weight(gpui::FontWeight::BOLD).child(
                    self.project_name().chars().take(2).collect::<String>().to_uppercase(),
                ),
            )
            .child(chip("tb-project", self.project_name()).on_click(cx.listener(|this, _, _, cx| {
                this.open_popup(PopupKind::Projects);
                cx.notify();
            })))
            .child(
                chip("tb-branch", format!("⎇ {}", self.branch_label()))
                    .on_click(cx.listener(|this, _, w, cx| this.dispatch(Action::Branches, w, cx))),
            )
            .child(div().flex_1())
            .child(
                chip("tb-config", self.run_widget_label())
                    .on_click(cx.listener(|this, _, _, cx| {
                        this.reload_runs();
                        this.open_popup(PopupKind::RunConfigs(false));
                        cx.notify();
                    })),
            )
            .child(
                div().id("tb-run").px(px(6.)).text_color(gpui::rgb(0x5fb865)).cursor_pointer().hover(|s| s.bg(Theme::hover_row())).rounded(px(5.)).child("▶")
                    .on_click(cx.listener(|this, _, w, cx| this.dispatch(Action::Run, w, cx))),
            )
            .child(
                div().id("tb-debug").px(px(6.)).text_color(gpui::rgb(0x5fb865)).cursor_pointer().hover(|s| s.bg(Theme::hover_row())).rounded(px(5.)).child("✱")
                    .on_click(cx.listener(|this, _, w, cx| this.dispatch(Action::Debug, w, cx))),
            )
            .child(
                div().id("tb-search").px(px(6.)).cursor_pointer().hover(|s| s.bg(Theme::hover_row())).rounded(px(5.)).child("⌕")
                    .on_click(cx.listener(|this, _, w, cx| this.dispatch(Action::SearchEverywhere, w, cx))),
            )
    }

    fn render_left_stripe(&self, cx: &mut Context<Self>) -> impl IntoElement {
        div()
            .w(px(40.))
            .h_full()
            .flex()
            .flex_col()
            .items_center()
            .py(px(6.))
            .gap(px(4.))
            .bg(Theme::panel())
            .border_r_1()
            .border_color(Theme::border())
            .child(self.stripe_button("st-project", "▤", "Project", self.left == Some(LeftTool::Project), Action::ToggleProjectWindow, cx))
            .child(self.stripe_button("st-commit", "✓", "Commit", self.left == Some(LeftTool::Commit), Action::ToggleCommitWindow, cx))
            .child(self.stripe_button("st-structure", "≡", "Structure", self.left == Some(LeftTool::Structure), Action::ToggleStructureWindow, cx))
            .child(self.stripe_button("st-bookmarks", "☆", "Bookmarks", self.left == Some(LeftTool::Bookmarks), Action::ToggleBookmarksWindow, cx))
            .child(div().flex_1())
            .child(self.stripe_button("st-services", "⚙", "Services", self.bottom == Some(BottomTool::Services), Action::ToggleServicesWindow, cx))
            .child(self.stripe_button("st-run", "▶", "Run", self.bottom == Some(BottomTool::Run), Action::ToggleRunWindow, cx))
            .child(self.stripe_button("st-terminal", "›_", "Terminal", self.bottom == Some(BottomTool::Terminal), Action::ToggleTerminalWindow, cx))
            .child(self.stripe_button("st-problems", "⚠", "Problems", self.bottom == Some(BottomTool::Problems), Action::ToggleProblemsWindow, cx))
            .child(self.stripe_button("st-git", "⎇", "Git", self.bottom == Some(BottomTool::Git), Action::ToggleGitWindow, cx))
    }

    fn render_right_stripe(&self, cx: &mut Context<Self>) -> impl IntoElement {
        div()
            .w(px(40.))
            .h_full()
            .flex()
            .flex_col()
            .items_center()
            .py(px(6.))
            .gap(px(4.))
            .bg(Theme::panel())
            .border_l_1()
            .border_color(Theme::border())
            .child(self.stripe_button("st-database", "⛁", "Database", self.db.open, Action::ToggleDatabaseWindow, cx))
    }

    fn panel_header(title: &str) -> impl IntoElement {
        div()
            .h(px(34.))
            .flex()
            .items_center()
            .px(px(12.))
            .text_size(px(13.))
            .font_weight(gpui::FontWeight::SEMIBOLD)
            .text_color(Theme::text())
            .child(title.to_string())
    }

    fn render_project(&mut self, cx: &mut Context<Self>) -> impl IntoElement {
        let n = self.tree.rows().len();
        div()
            .id("project-panel")
            .key_context("ProjectView")
            .track_focus(&self.tree_focus)
            .flex()
            .flex_col()
            .size_full()
            .child(Self::panel_header("Project"))
            .child(
                uniform_list(
                    "project-tree",
                    n,
                    cx.processor(|this, range: std::ops::Range<usize>, window, cx| {
                        let focused = this.tree_focus.is_focused(window);
                        let rows: Vec<_> = this.tree.rows()[range].to_vec();
                        rows.into_iter()
                            .map(|r| {
                                let node = &this.tree.nodes[r.id];
                                let selected = this.tree_selected == Some(r.id);
                                let chevron = if node.is_dir { if node.expanded { "▾" } else { "▸" } } else { " " };
                                let (icon, icon_color) = file_icon(&node.name, node.is_dir);
                                let open = this.active_editor().is_some_and(|e| e.read(cx).path.as_deref() == Some(node.path.as_path()));
                                let vcs = if node.is_dir { None } else { this.file_status(&node.path).map(git_ui::status_color) };
                                let name_color = if node.excluded {
                                    Theme::excluded()
                                } else if let Some(c) = vcs {
                                    c
                                } else if open {
                                    Theme::modified()
                                } else {
                                    Theme::text()
                                };
                                let id = r.id;
                                let mut label = node.name.clone();
                                if r.depth == 0 {
                                    label = format!("{}  {}", node.name, node.path.display());
                                }
                                div()
                                    .id(("row", r.id))
                                    .h(px(24.))
                                    .flex()
                                    .items_center()
                                    .pl(px(8. + r.depth as f32 * 16.))
                                    .gap(px(4.))
                                    .text_size(px(13.))
                                    .when(selected, |d| d.bg(if focused { Theme::selection_row() } else { Theme::hover_row() }))
                                    .hover(|s| s.bg(Theme::hover_row()))
                                    .on_click(cx.listener(move |this, e: &ClickEvent, window, cx| this.tree_click(id, e, window, cx)))
                                    .on_mouse_down(gpui::MouseButton::Right, cx.listener(move |this, e: &gpui::MouseDownEvent, window, cx| this.tree_menu(id, e.position, window, cx)))
                                    .child(div().w(px(12.)).text_color(Theme::text_muted()).child(chevron))
                                    .child(div().w(px(16.)).text_color(icon_color).child(icon))
                                    .child(div().text_color(name_color).whitespace_nowrap().child(label))
                            })
                            .collect()
                    }),
                )
                .track_scroll(self.tree_scroll.clone())
                .flex_1(),
            )
    }

    fn render_structure(&mut self, cx: &mut Context<Self>) -> impl IntoElement {
        let items = self.active_editor().cloned().map(|e| e.update(cx, |e, _| e.outline())).unwrap_or_default();
        let ed = self.active_editor().cloned();
        div()
            .id("structure")
            .flex()
            .flex_col()
            .size_full()
            .overflow_y_scroll()
            .child(Self::panel_header("Structure"))
            .children(items.into_iter().enumerate().map(move |(i, it)| {
                let (glyph, color) = match it.kind {
                    brisk_syntax::OutlineKind::Class => ("C", gpui::rgb(0x6c9be8)),
                    brisk_syntax::OutlineKind::Function => ("f", gpui::rgb(0xe0a85a)),
                    brisk_syntax::OutlineKind::Method => ("m", gpui::rgb(0xcf8e6d)),
                };
                let ed = ed.clone();
                let line = it.line;
                div()
                    .id(("outline", i))
                    .h(px(24.))
                    .flex()
                    .items_center()
                    .gap(px(6.))
                    .pl(px(12. + it.depth as f32 * 16.))
                    .text_size(px(13.))
                    .hover(|s| s.bg(Theme::hover_row()))
                    .cursor_pointer()
                    .on_click(cx.listener(move |this, _, window, cx| {
                        if let Some(ed) = &ed {
                            ed.update(cx, |e, cx| e.go_to_line(line, cx));
                        }
                        this.focus_editor(window, cx);
                    }))
                    .child(div().w(px(16.)).h(px(16.)).rounded(px(8.)).bg(color).text_color(Theme::bg()).text_size(px(10.)).flex().items_center().justify_center().child(glyph))
                    .child(it.name)
            }))
    }

    fn render_placeholder(title: &str, body: &str) -> gpui::AnyElement {
        div()
            .flex()
            .flex_col()
            .size_full()
            .child(Self::panel_header(title))
            .child(div().px(px(12.)).text_size(px(13.)).text_color(Theme::text_muted()).child(body.to_string()))
            .into_any_element()
    }

    fn render_tabs(&self, cx: &mut Context<Self>) -> impl IntoElement {
        div()
            .id("tabs")
            .h(px(36.))
            .flex()
            .items_end()
            .overflow_x_scroll()
            .bg(Theme::panel_alt())
            .border_b_1()
            .border_color(Theme::divider())
            .children(self.tabs.iter().enumerate().map(|(i, t)| {
                let active = self.active == Some(i);
                let (title, icon, color, name_color, dirty) = match t {
                    Tab::Editor(e) => {
                        let ed = e.read(cx);
                        let title = ed.title();
                        let (icon, color) = file_icon(&title, false);
                        let vcs = ed.path.as_deref().and_then(|p| self.file_status(p)).map(git_ui::status_color);
                        let vcs = if ed.disk_conflict.is_some() { Some(gpui::rgb(0xe0b84a).into()) } else { vcs };
                        (title, icon, color, vcs, ed.buffer.is_dirty())
                    }
                    Tab::Diff(d) => (d.read(cx).title.clone(), "⇆", Theme::file_modified(), None, false),
                    Tab::Grid(g) => (g.read(cx).title.clone(), "▦", gpui::rgb(0x6c9be8).into(), None, false),
                };
                let name_color = name_color.unwrap_or(if active { Theme::text() } else { Theme::text_muted() });
                div()
                    .id(("tab", i))
                    .h(px(36.))
                    .flex()
                    .items_center()
                    .gap(px(6.))
                    .px(px(12.))
                    .text_size(px(13.))
                    .text_color(if active { Theme::text() } else { Theme::text_muted() })
                    .when(active, |d| d.border_b_2().border_color(Theme::accent()).bg(Theme::editor_bg()))
                    .hover(|s| s.bg(Theme::hover_row()))
                    .cursor_pointer()
                    .on_click(cx.listener(move |this, _, window, cx| {
                        this.save_all(cx);
                        this.active = Some(i);
                        this.focus_editor(window, cx);
                        cx.notify();
                    }))
                    .on_mouse_down(MouseButton::Middle, cx.listener(move |this, _, window, cx| this.close_tab(i, window, cx)))
                    .child(div().text_color(color).child(icon))
                    .child(div().text_color(name_color).child(title))
                    .when(dirty, |d| d.child(div().text_color(Theme::text_muted()).child("•")))
                    .child(
                        div().id(("tab-close", i)).text_color(Theme::text_dim()).hover(|s| s.text_color(Theme::text())).child("×")
                            .on_click(cx.listener(move |this, _, window, cx| {
                                cx.stop_propagation();
                                this.close_tab(i, window, cx);
                            })),
                    )
            }))
    }

    fn render_welcome(&self) -> impl IntoElement {
        let km = self.resolver.keymap();
        let row = |label: &str, a: Action| {
            div()
                .flex()
                .gap(px(16.))
                .child(div().w(px(180.)).text_color(Theme::text_muted()).child(label.to_string()))
                .child(div().text_color(Theme::modified()).child(km.shortcut_label(a).unwrap_or_default()))
        };
        div()
            .size_full()
            .flex()
            .flex_col()
            .items_center()
            .justify_center()
            .gap(px(10.))
            .bg(Theme::editor_bg())
            .text_size(px(14.))
            .child(row("Search Everywhere", Action::SearchEverywhere))
            .child(row("Go to File", Action::GotoFile))
            .child(row("Recent Files", Action::RecentFiles))
            .child(row("Find Action", Action::FindAction))
            .child(row("Project View", Action::ToggleProjectWindow))
            .child(row("Terminal", Action::ToggleTerminalWindow))
    }

    fn render_status(&self, cx: &mut Context<Self>) -> impl IntoElement {
        let enc = self.active_editor().map(|e| e.read(cx).encoding_label()).unwrap_or_default();
        let (pos, eol, lang, crumbs) = match self.active_editor() {
            Some(e) => {
                let e = e.read(cx);
                let (l, c) = e.caret_line_col();
                let eol = match e.buffer.line_ending {
                    brisk_text::LineEnding::Lf => "LF",
                    brisk_text::LineEnding::CrLf => "CRLF",
                };
                let crumbs = e
                    .path
                    .as_ref()
                    .and_then(|p| p.strip_prefix(&self.root).ok())
                    .map(|p| format!("{} › {}", self.project_name(), p.to_string_lossy().replace(['\\', '/'], " › ")))
                    .unwrap_or_default();
                (format!("{}:{}", l + 1, c + 1), eol, e.language_name(), crumbs)
            }
            None => (String::new(), "", "", self.project_name()),
        };
        let item = |s: String| div().px(px(8.)).child(s);
        div()
            .h(px(26.))
            .flex()
            .items_center()
            .px(px(8.))
            .bg(Theme::panel())
            .border_t_1()
            .border_color(Theme::border())
            .text_size(px(12.))
            .text_color(Theme::text_muted())
            .child(div().child(crumbs))
            .child(div().flex_1())
            .child(item(self.active_editor().and_then(|e| e.read(cx).caret_problem()).unwrap_or_else(|| self.status.clone())))
            .when(!pos.is_empty(), |d| {
                d.child(item(pos))
                    .child(item(eol.into()))
                    .child(item(enc))
                    .child(item("4 spaces".into()))
                    .child(item(lang.into()))
            })
            .child(
                div()
                    .id("sb-interpreter")
                    .px(px(8.))
                    .rounded(px(4.))
                    .hover(|s| s.bg(Theme::hover_row()))
                    .cursor_pointer()
                    .child(self.interpreter_label())
                    .on_click(cx.listener(|this, _, _, cx| {
                        this.open_popup(PopupKind::Interpreters);
                        cx.notify();
                    })),
            )
    }

    fn render_popup(&self, cx: &mut Context<Self>) -> Option<impl IntoElement> {
        let p = self.popup.as_ref()?;
        let (title, placeholder) = match p.kind {
            PopupKind::SearchEverywhere(_) => ("", "Type / to see commands"),
            PopupKind::FindAction => ("Actions", "Enter action name"),
            PopupKind::RecentFiles => ("Recent Files", "Type to filter"),
            PopupKind::GotoLine => ("Go to Line:Column", "[Line] [:column]"),
            PopupKind::Branches => ("Git Branches", "Search branches"),
            PopupKind::NewBranch => ("Create New Branch", "New branch name"),
            PopupKind::VcsOperations => ("VCS Operations", "Type to filter"),
            PopupKind::Confirm => ("Confirm", ""),
            PopupKind::RunConfigs(true) => ("Run", "Type to filter"),
            PopupKind::RunConfigs(false) => ("Run Configurations", "Type to filter"),
            PopupKind::Interpreters => ("Python Interpreter", "Type to filter"),
            PopupKind::Intentions => ("Context Actions", "Type to filter"),
            PopupKind::Rename => ("Rename", "New name"),
            PopupKind::Projects => ("Projects", "Type to filter"),
            PopupKind::OpenWhere => ("Open Project", "Type to filter"),
            PopupKind::FileName | PopupKind::FileDelete => match &self.file_op {
                Some(op) => (op.title(), op.placeholder()),
                None => ("", ""),
            },
        };
        let tabs = match p.kind {
            PopupKind::SearchEverywhere(t) => Some(t),
            _ => None,
        };
        let selected = p.selected;
        let indexing = match p.kind {
            PopupKind::SearchEverywhere(SeTab::Classes | SeTab::Symbols) => self.symbols_indexing(),
            PopupKind::SearchEverywhere(_) => self.files.is_none(),
            _ => false,
        };
        Some(
            div()
                .absolute()
                .top_0()
                .left_0()
                .size_full()
                .flex()
                .justify_center()
                .pt(px(90.))
                .occlude()
                .child(
                    div()
                        .id("popup")
                        // A click outside the popup closes it, as in PyCharm; the backdrop keeps the
                        // click from reaching what is underneath.
                        .on_mouse_down_out(cx.listener(|this, _, window, cx| {
                            this.popup = None;
                            this.focus_editor(window, cx);
                            cx.notify();
                        }))
                        .w(px(680.))
                        .max_h(px(520.))
                        .flex()
                        .flex_col()
                        .bg(Theme::panel())
                        .border_1()
                        .border_color(Theme::divider())
                        .rounded(px(10.))
                        .shadow_lg()
                        .overflow_hidden()
                        .text_size(px(13.))
                        .text_color(Theme::text())
                        .when_some(tabs, |d, t| {
                            d.child(
                                div()
                                    .flex()
                                    .gap(px(4.))
                                    .px(px(10.))
                                    .pt(px(8.))
                                    .children(SeTab::ORDER.iter().enumerate().map(|(i, &tab)| {
                                        div()
                                            .id(("se-tab", i))
                                            .px(px(10.))
                                            .py(px(4.))
                                            .rounded(px(5.))
                                            .cursor_pointer()
                                            .when(tab == t, |d| d.bg(Theme::selection_row()))
                                            .when(tab != t, |d| d.hover(|s| s.bg(Theme::hover_row())))
                                            .on_click(cx.listener(move |this, _, _, cx| {
                                                this.set_se_tab(tab);
                                                cx.notify();
                                            }))
                                            .child(tab.title())
                                    }))
                                    .child(div().flex_1())
                                    .child(div().text_color(Theme::text_dim()).text_size(px(11.)).child("Tab to switch")),
                            )
                        })
                        .when(!title.is_empty(), |d| d.child(div().px(px(14.)).pt(px(10.)).font_weight(gpui::FontWeight::SEMIBOLD).child(title)))
                        .child(
                            div()
                                .m(px(10.))
                                .px(px(10.))
                                .h(px(32.))
                                .flex()
                                .items_center()
                                .gap(px(8.))
                                .rounded(px(6.))
                                .border_1()
                                .border_color(Theme::accent())
                                .bg(Theme::bg())
                                .child(div().text_color(Theme::text_muted()).child("⌕"))
                                .child(if p.query.is_empty() {
                                    div().text_color(Theme::text_dim()).child(placeholder).into_any_element()
                                } else {
                                    div().flex().child(p.query.clone()).child(div().w(px(1.5)).h(px(16.)).bg(Theme::caret())).into_any_element()
                                }),
                        )
                        .child(
                            div()
                                .id("popup-list")
                                .flex()
                                .flex_col()
                                .overflow_y_scroll()
                                .pb(px(6.))
                                .when(indexing, |d| d.child(div().px(px(14.)).py(px(6.)).text_color(Theme::text_muted()).child("Indexing…")))
                                .when(p.items.is_empty() && !indexing && !p.query.is_empty(), |d| {
                                    d.child(div().px(px(14.)).py(px(6.)).text_color(Theme::text_muted()).child("Nothing found"))
                                })
                                .when(
                                    p.items.is_empty() && !indexing && p.query.is_empty() && matches!(p.kind, PopupKind::SearchEverywhere(SeTab::Classes | SeTab::Symbols)),
                                    |d| d.child(div().px(px(14.)).py(px(6.)).text_color(Theme::text_dim()).child("Type a name: a prefix, or word starts like gcw for get_current_window")),
                                )
                                .children(p.items.iter().enumerate().map(|(i, it)| {
                                    let (icon, color) = match &it.target {
                                        Target::File(_) => file_icon(&it.label, false),
                                        Target::Action(_) => ("⚡", gpui::rgb(0xb3ae60).into()),
                                        Target::Line(_) => ("↧", Theme::text_muted()),
                                        Target::Branch(b) if b.current => ("★", gpui::rgb(0xe0a85a).into()),
                                        Target::Branch(b) if b.remote => ("☁", Theme::text_muted()),
                                        Target::Branch(_) => ("⎇", Theme::text_muted()),
                                        Target::NewBranch => ("+", Theme::text_muted()),
                                        Target::Confirm(_) => ("✓", Theme::file_added()),
                                        Target::Cancel => ("×", Theme::text_muted()),
                                        Target::RunConfig(None, _) => ("▤", Theme::text_muted()),
                                        Target::RunConfig(Some(_), _) => ("▶", gpui::rgb(0x5fb865).into()),
                                        Target::EditRunConfigs => ("✎", Theme::text_muted()),
                                        Target::Interpreter(_) => ("◆", gpui::rgb(0x4b8bbe).into()),
                                        Target::Intention(_) => ("✦", gpui::rgb(0xe0b84a).into()),
                                        Target::Rename(_) => ("✎", gpui::rgb(0x4b8bbe).into()),
                                        Target::Project(project_ui::ProjectCmd::New) => ("+", Theme::text_muted()),
                                        Target::Project(project_ui::ProjectCmd::Open) => ("▰", gpui::rgb(0x8c8f96).into()),
                                        Target::Project(_) => ("▣", gpui::rgb(0x4b8bbe).into()),
                                        Target::Symbol(_, _, _, brisk_syntax::OutlineKind::Class) => ("C", gpui::rgb(0x4b8bbe).into()),
                                        Target::Symbol(_, _, _, brisk_syntax::OutlineKind::Method) => ("m", gpui::rgb(0xe0a85a).into()),
                                        Target::Symbol(..) => ("ƒ", gpui::rgb(0xb07fd5).into()),
                                        Target::FileDo(_) => ("✓", Theme::file_added()),
                                    };
                                    div()
                                        .id(("item", i))
                                        .h(px(28.))
                                        .mx(px(6.))
                                        .px(px(8.))
                                        .flex()
                                        .items_center()
                                        .gap(px(8.))
                                        .rounded(px(5.))
                                        .when(i == selected, |d| d.bg(Theme::selection_row()))
                                        .hover(|s| s.bg(Theme::hover_row()))
                                        .cursor_pointer()
                                        .on_click(cx.listener(move |this, _, window, cx| {
                                            if let Some(p) = this.popup.as_mut() {
                                                p.selected = i;
                                            }
                                            let ev = KeyDownEvent { keystroke: gpui::Keystroke::parse("enter").unwrap(), is_held: false };
                                            this.popup_key(&ev, window, cx);
                                        }))
                                        .child(div().w(px(16.)).text_color(color).child(icon))
                                        .child(it.label.clone())
                                        .child(div().text_color(Theme::text_dim()).text_size(px(12.)).child(it.detail.clone()))
                                        .child(div().flex_1())
                                        .child(div().text_color(Theme::text_muted()).text_size(px(12.)).child(it.hint.clone()))
                                })),
                        ),
                ),
        )
    }
}

/// Text glyph icons (our own, not JetBrains artwork).
fn file_icon(name: &str, is_dir: bool) -> (&'static str, gpui::Hsla) {
    if is_dir {
        return ("▰", gpui::rgb(0x8c8f96).into());
    }
    let ext = name.rsplit('.').next().unwrap_or("");
    let (g, c) = match ext {
        "py" | "pyi" => ("◆", 0x4b8bbe),
        "js" | "jsx" | "mjs" => ("◆", 0xe8d44d),
        "ts" | "tsx" => ("◆", 0x3178c6),
        "html" => ("◆", 0xe34c26),
        "css" | "scss" => ("◆", 0x563d7c),
        "json" => ("{", 0xcbcb41),
        "md" => ("M", 0x8c8f96),
        "toml" | "ini" | "cfg" | "yaml" | "yml" => ("⚙", 0x8c8f96),
        "txt" | "lock" => ("≡", 0x8c8f96),
        _ => ("•", 0x8c8f96),
    };
    (g, gpui::rgb(c).into())
}

struct Tip(SharedString);

impl Render for Tip {
    fn render(&mut self, _: &mut Window, _: &mut Context<Self>) -> impl IntoElement {
        div()
            .px(px(8.))
            .py(px(4.))
            .rounded(px(5.))
            .bg(Theme::panel_alt())
            .border_1()
            .border_color(Theme::divider())
            .text_size(px(12.))
            .text_color(Theme::text())
            .child(self.0.clone())
    }
}

impl Focusable for Workspace {
    fn focus_handle(&self, _: &App) -> FocusHandle {
        self.focus.clone()
    }
}

impl Render for Workspace {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let viewport = window.viewport_size();
        let left: Option<gpui::AnyElement> = match self.left {
            Some(LeftTool::Project) => Some(self.render_project(cx).into_any_element()),
            Some(LeftTool::Structure) => Some(self.render_structure(cx).into_any_element()),
            Some(LeftTool::Commit) => Some(self.render_commit(cx)),
            Some(LeftTool::Bookmarks) => Some(Self::render_placeholder("Bookmarks", "Stage 1: F11 / Ctrl+F11 bookmarks.")),
            None => None,
        };
        let bottom_h = match self.bottom {
            Some(BottomTool::Git) => 320.,
            Some(BottomTool::Services) => 280.,
            _ => 240.,
        };
        let right = self.db.open.then(|| self.render_database(cx));
        let bottom = self.bottom.map(|b| match (b, &self.terminal) {
            (BottomTool::Terminal, Some(t)) => t.clone().into_any_element(),
            (BottomTool::Run, _) if self.runs.panel.is_some() => self.runs.panel.clone().unwrap().into_any_element(),
            (BottomTool::Git, _) => self.render_git_log(cx),
            (BottomTool::Find, _) => self.render_find_window(cx),
            (BottomTool::Problems, _) => self.render_problems(cx),
            (BottomTool::Services, _) => self.render_services(cx),
            _ => Self::render_placeholder(b.title(), b.stage()),
        });
        let center: gpui::AnyElement = match self.active.and_then(|i| self.tabs.get(i)) {
            Some(Tab::Editor(e)) => e.clone().into_any_element(),
            Some(Tab::Diff(d)) => d.clone().into_any_element(),
            Some(Tab::Grid(g)) => g.clone().into_any_element(),
            None => self.render_welcome().into_any_element(),
        };

        div()
            .id("workspace")
            .key_context("Workspace")
            .track_focus(&self.focus)
            .capture_key_down(cx.listener(Self::on_key_down))
            .on_modifiers_changed(cx.listener(Self::on_modifiers))
            .size_full()
            .flex()
            .flex_col()
            .bg(Theme::bg())
            .text_color(Theme::text())
            .font_family(UI_FONT)
            .child(self.render_toolbar(cx))
            .child(
                div()
                    .flex_1()
                    .flex()
                    .min_h_0()
                    .child(self.render_left_stripe(cx))
                    .when_some(left, |d, panel| {
                        d.child(div().w(px(300.)).h_full().flex_shrink_0().bg(Theme::panel()).border_r_1().border_color(Theme::border()).child(panel))
                    })
                    .child(
                        div()
                            .flex_1()
                            .min_w_0()
                            .flex()
                            .flex_col()
                            .when(!self.tabs.is_empty(), |d| d.child(self.render_tabs(cx)))
                            .child(div().flex_1().min_h_0().font_family(MONO_FONT).child(center))
                            .when_some(bottom, |d, panel| {
                                d.child(div().h(px(bottom_h)).flex_shrink_0().bg(Theme::panel()).border_t_1().border_color(Theme::divider()).child(panel))
                            }),
                    )
                    .when_some(right, |d, panel| {
                        d.child(div().w(px(320.)).h_full().flex_shrink_0().bg(Theme::panel()).border_l_1().border_color(Theme::border()).child(panel))
                    })
                    .child(self.render_right_stripe(cx)),
            )
            .child(self.render_status(cx))
            .children(self.render_toast())
            .children(self.render_popup(cx))
            .children(self.render_find_popup(cx))
            .children(self.render_ds_dialog(cx))
            .children(self.render_menu(viewport, cx))
    }
}
