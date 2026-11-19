//! Main window: tool window stripes, project tree, editor tabs, popups, status bar.
//! All shortcuts go through brisk-keymap's PyCharm keymap.

mod bookmarks_ui;
mod console_ui;
mod packages_ui;
mod packages_smoke;
mod todo_ui;
mod db_ui;
mod services_ui;
mod pin_smoke;
mod dataview_smoke;
mod multi_smoke;
mod pycon_smoke;
mod run_tpl_smoke;
mod run_term_smoke;
mod test_view_smoke;
mod ansi_smoke;
mod latency_smoke;
mod flood_smoke;
mod test_runners_smoke;
mod debug_ui;
mod disk_ui;
mod edit2_smoke;
mod edit_smoke;
mod files_ui;
mod find_ui;
mod git_annotate_ui;
mod git_branch_ui;
mod git_push_ui;
mod git_compare_ui;
mod git_history_ui;
mod git_log_filter;
mod git_merge_ui;
mod git_stash_ui;
mod git_smoke;
mod vcs_smoke;
mod git_ui;
pub(crate) use git_ui::fmt_time;
mod history_ui;
mod intel_smoke;
mod complete_smoke;
mod big_smoke;
mod local_refs;
mod mouse_smoke;
mod bars_smoke;
mod scope_smoke;
mod w7_smoke;
mod lsp_ui;
mod main_menu;
mod menu_ui;
mod problems_ui;
pub(crate) mod project_ui;
mod run_config_ui;
mod run_marks_ui;
mod run_ui;
mod settings_ui;
mod status_ui;
mod tabs_ui;
mod venv_ui;
mod run_anything_ui;
mod terminal_env_ui;
mod coverage_ui;
mod profile_ui;
mod before_launch_ui;
mod web_smoke;
mod console_find_smoke;
mod membudget_smoke;
mod hang_smoke;
mod scroll_smoke;
mod templates_ui;
mod win_input;

use crate::theme::ui_px;
use crate::diff_view::DiffView;
use crate::editor::Editor;
use crate::grid_view::GridView;
use crate::keys;
use crate::terminal_view::TerminalPanel;
use crate::theme::{Theme, mono_font, ui_font};
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
    PythonConsole,
    Packages,
    Todo,
    Coverage,
    Profiler,
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
            BottomTool::PythonConsole => "Python Console",
            BottomTool::Packages => "Python Packages",
            BottomTool::Todo => "TODO",
            BottomTool::Coverage => "Coverage",
            BottomTool::Profiler => "Profiler",
        }
    }
    fn stage(self) -> &'static str {
        match self {
            BottomTool::Terminal => "",
            BottomTool::Run => "Nothing is running. Run a file with Ctrl+Shift+F10, the selected configuration with Shift+F10.",
            BottomTool::Debug => "Shift+F9 debugs the selected configuration, Ctrl+Shift+F9 the current file.",
            BottomTool::Problems => "Stage 2: ruff + ty diagnostics.",
            BottomTool::Git => "",
            BottomTool::Services => "Stage 5: Docker and databases.",
            BottomTool::Find => "",
            BottomTool::PythonConsole | BottomTool::Packages | BottomTool::Todo | BottomTool::Coverage | BottomTool::Profiler => "",
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
    /// Add Interpreter: the base Python for a new project venv.
    AddInterpreter,
    /// Run Anything (double Ctrl).
    RunAnything,
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
    /// Ctrl+F12.
    FileStructure,
    /// Shift+F11.
    Bookmarks,
    /// Ctrl+F11: pick a digit or letter.
    BookmarkMnemonic,
    /// Ctrl+Shift+V.
    PasteHistory,
    /// Show Local History.
    LocalHistory,
    /// Local History → Put Label: the label's name.
    PutLabel,
    /// A breakpoint's condition.
    BreakpointCondition,
    /// A directory breadcrumb: the folder's contents (`Workspace::crumb_dir`).
    FolderCrumb,
    /// The status bar's background tasks.
    BackgroundTasks,
    /// The ▾ at the end of the tab strip: every tab of the group, hidden ones marked.
    TabList,
}

#[derive(Clone)]
pub enum Tab {
    Editor(Entity<Editor>),
    Diff(Entity<DiffView>),
    Grid(Entity<GridView>),
    /// View as DataFrame / Array of the paused program.
    Data(Entity<crate::data_view::DataView>),
}

impl Tab {
    fn entity_id(&self) -> gpui::EntityId {
        match self {
            Tab::Editor(e) => e.entity_id(),
            Tab::Diff(d) => d.entity_id(),
            Tab::Grid(g) => g.entity_id(),
            Tab::Data(v) => v.entity_id(),
        }
    }

    fn focus(&self, cx: &App) -> FocusHandle {
        match self {
            Tab::Editor(e) => e.read(cx).focus_handle(cx),
            // A live diff types into its editor.
            Tab::Diff(d) => match d.read(cx).live_editor() {
                Some(e) => e.read(cx).focus_handle(cx),
                None => d.read(cx).focus_handle(cx),
            },
            Tab::Grid(g) => g.read(cx).focus_handle(cx),
            Tab::Data(v) => v.read(cx).focus_handle(cx),
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
    /// A folder in the folder-crumb popup: Enter or → steps into it.
    Dir(PathBuf),
    Action(Action),
    Line(usize),
    Branch(brisk_git::Branch),
    NewBranch,
    Confirm(git_ui::Pending),
    Cancel,
    /// A row of the background tasks popup (nothing to do on Enter).
    Task,
    /// A tab of the focused editor group (the ▾ list).
    Tab(usize),
    RunConfig(Option<String>, bool),
    /// The configuration is still running: stop it and run it again.
    StopAndRerun(Box<brisk_run::RunConfig>),
    EditRunConfigs,
    /// Make a temporary run configuration permanent.
    SaveRunConfig(String),
    Interpreter(PathBuf),
    /// "Add Interpreter…" in the interpreter popup.
    AddInterpreter,
    /// A new project venv from this base interpreter.
    CreateVenv(PathBuf),
    /// A Run Anything command line.
    RunAnything(String),
    Intention(Box<problems_ui::Intent>),
    Rename(String),
    Project(project_ui::ProjectCmd),
    /// A class or function from the symbol index: file, line, column.
    Symbol(PathBuf, usize, usize, brisk_syntax::OutlineKind),
    /// Run the pending file operation with this target.
    FileDo(PathBuf),
    /// A bookmark: file and line.
    Bookmark(PathBuf, usize),
    /// Ctrl+F11: set a bookmark with this mnemonic.
    Mnemonic(char),
    /// Paste from History: entry index.
    Clip(usize),
    /// A Local History revision: file and time (ms).
    History(PathBuf, u64),
    /// Put a Local History label with this name.
    PutLabel(String),
    /// Set the edited breakpoint's condition (empty: none).
    BpCondition(String),
    BpRemove,
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

/// Structure tool window state: its scroll and the item it last followed the caret to.
#[derive(Default)]
struct StructureView {
    scroll: gpui::ScrollHandle,
    bar: crate::scrollbar::BarState,
    follow: Option<usize>,
    /// Caret line of the active editor at the last repaint.
    caret: Option<usize>,
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
    popup_scroll: UniformListScrollHandle,
    tree_bar: crate::scrollbar::BarState,
    structure_view: StructureView,
    files: Option<Arc<FileList>>,
    tabs: Vec<Tab>,
    active: Option<usize>,
    /// The tab strips of the two editor groups (index: `second`), and the (active tab, tab
    /// count) each last scrolled to, so a newly active tab is brought into view once.
    tab_scroll: [gpui::ScrollHandle; 2],
    tab_seen: [std::cell::Cell<(Option<usize>, usize)>; 2],
    git: Option<git_ui::Git>,
    git_init_running: bool,
    /// Commit window state exists even without a repo (it offers "Create Git Repository").
    commit_focus: FocusHandle,
    toast: Option<(String, bool)>,
    toast_gen: u64,
    find: find_ui::Find,
    db: db_ui::Db,
    services: services_ui::Services,
    debug: debug_ui::Debug,
    /// Created on first open of the Python Console.
    pyc: Option<console_ui::PyConsole>,
    /// A project venv is being created (Add Interpreter).
    venv_busy: bool,
    /// The offer to install requirements into a new venv.
    venv_offer: Option<venv_ui::VenvOffer>,
    /// Run Anything history, newest first.
    ra_history: Vec<String>,
    /// The last coverage report (Run with Coverage).
    coverage: Option<coverage_ui::CoverageView>,
    /// A coverage run whose report comes when its tab finishes.
    cov_pending: Option<coverage_ui::Pending>,
    /// Offer to install coverage.py for this configuration.
    cov_offer: Option<brisk_run::RunConfig>,
    /// The last profile (Run › Profile).
    profile: Option<profile_ui::ProfileView>,
    /// A profiled run whose stats come when its tab finishes.
    prof_pending: Option<profile_ui::Pending>,
    /// Before launch steps in progress.
    before: Option<before_launch_ui::Chain>,
    /// The configuration whose Before launch just passed (its launch skips the check once).
    before_passed: Option<String>,
    /// The last `http(s)://` link opened from the Run console.
    last_url: Option<String>,
    /// pytest results per file for the run icons in the gutter.
    test_marks: run_marks_ui::TestMarks,
    /// Created on first open of Python Packages.
    packages: Option<packages_ui::Packages>,
    /// Created on first open of the TODO window; then kept current on save.
    todo: Option<todo_ui::Todo>,
    /// The breakpoint whose condition the popup edits.
    debug_edit: Option<(Entity<Editor>, usize)>,
    settings_dialog: Option<settings_ui::SettingsDialog>,
    recent: Vec<PathBuf>,
    /// Bookmarks of files that are not open.
    bookmarks: Vec<bookmarks_ui::Stored>,
    /// Items of the File Structure / Bookmarks popup, taken when it opens.
    structure: Vec<PopupItem>,
    /// The folder the folder-crumb popup lists.
    crumb_dir: (PathBuf, String),
    /// The second editor group (split editor).
    split: Option<tabs_ui::Split>,
    /// Recently closed files (Reopen Closed Tab).
    closed: Vec<PathBuf>,
    /// Pinned tabs (both groups): kept first and spared by Close Other / All / Left / Right.
    pinned: std::collections::HashSet<gpui::EntityId>,
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
    /// Double Ctrl ("Ctrl, Ctrl+Up"): when Ctrl was last tapped alone, whether it is held now
    /// alone, and whether this hold is the second tap (arrows clone the caret).
    ctrl_tap: Option<Instant>,
    ctrl_alone: bool,
    ctrl_clone: bool,
    status: String,
    started: Instant,
}

/// An icon button of the title bar: 28 px square, hover background with radius 5.
fn title_button(id: &'static str, icon: &'static str, color: gpui::Hsla, tip: &'static str) -> gpui::Stateful<gpui::Div> {
    div()
        .id(id)
        .w(px(28.))
        .h(ui_px(28.))
        .flex()
        .items_center()
        .justify_center()
        .rounded(px(5.))
        .cursor_pointer()
        .hover(|s| s.bg(Theme::hover_row()))
        .active(|s| s.bg(Theme::press()))
        .tooltip(move |_, cx| cx.new(|_| Tip(tip.into())).into())
        .child(crate::icons::ui_icon(icon, 16., color))
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
        if std::env::var_os("BRISK_SMOKE_TEMPLATES").is_some() {
            cx.defer_in(window, move |this: &mut Self, window, cx| this.templates_smoke(window, cx));
        }
        if std::env::var_os("BRISK_SMOKE_BARS").is_some() {
            cx.defer_in(window, move |this: &mut Self, window, cx| this.bars_smoke(window, cx));
        }
        if std::env::var_os("BRISK_SMOKE_MOUSE").is_some() {
            cx.defer_in(window, move |this: &mut Self, window, cx| this.mouse_smoke(window, cx));
        }
        if std::env::var_os("BRISK_SMOKE_W7").is_some() {
            cx.defer_in(window, move |this: &mut Self, window, cx| this.w7_smoke(window, cx));
        }
        if std::env::var_os("BRISK_SMOKE_SCOPE").is_some() {
            cx.defer_in(window, move |this: &mut Self, window, cx| this.scope_smoke(window, cx));
        }
        if let Some(f) = std::env::var_os("BRISK_SMOKE_BIG") {
            let f = PathBuf::from(f);
            cx.defer_in(window, move |this: &mut Self, window, cx| this.big_smoke(f, window, cx));
        }
        if std::env::var_os("BRISK_SMOKE_COMPLETE").is_some() {
            cx.defer_in(window, move |this: &mut Self, window, cx| this.complete_smoke(window, cx));
        }
        if std::env::var_os("BRISK_SMOKE_INTEL").is_some() {
            cx.defer_in(window, move |this: &mut Self, window, cx| this.intel_smoke(window, cx));
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
        // Screenshot helpers: open files (a `;`-separated list) and/or the Settings dialog at start.
        if let Some(f) = std::env::var_os("BRISK_OPEN_FILE") {
            let files: Vec<PathBuf> = std::env::split_paths(&f).collect();
            cx.defer_in(window, move |this: &mut Self, window, cx| {
                for f in &files {
                    this.open_file(f, window, cx);
                }
            });
        }
        // Screenshot helper: the Structure window, with the caret on line N (1-based) of the file.
        if let Some(line) = std::env::var("BRISK_OPEN_STRUCTURE").ok().and_then(|l| l.parse::<usize>().ok()) {
            cx.defer_in(window, move |this: &mut Self, window, cx| {
                this.toggle_left(LeftTool::Structure, window, cx);
                if let Some(ed) = this.active_editor().cloned() {
                    ed.update(cx, |e, cx| e.go_to_line(line.saturating_sub(1), cx));
                }
            });
        }
        if std::env::var_os("BRISK_OPEN_SETTINGS").is_some() {
            cx.defer_in(window, |this: &mut Self, window, cx| {
                this.open_settings(window);
                cx.notify();
            });
        }
        if let Some(f) = std::env::var_os("BRISK_SMOKE_DATAVIEW") {
            let f = PathBuf::from(f);
            cx.defer_in(window, move |this: &mut Self, window, cx| this.dataview_smoke(f, window, cx));
        }
        if let Some(f) = std::env::var_os("BRISK_SMOKE_DEBUG") {
            let f = PathBuf::from(f);
            cx.defer_in(window, move |this: &mut Self, window, cx| this.debug_smoke(f, window, cx));
        }
        if std::env::var_os("BRISK_SMOKE_SETTINGS").is_some() {
            cx.defer_in(window, |this: &mut Self, window, cx| this.settings_smoke(window, cx));
        }
        if std::env::var_os("BRISK_SMOKE_DISK").is_some() {
            cx.defer_in(window, |this: &mut Self, window, cx| this.disk_smoke(window, cx));
        }
        if std::env::var_os("BRISK_SMOKE_UI").is_some() {
            cx.defer_in(window, |this: &mut Self, window, cx| this.ui_smoke(window, cx));
        }
        if std::env::var_os("BRISK_SMOKE_RUNCFG").is_some() {
            cx.defer_in(window, |this: &mut Self, window, cx| this.run_dialog_smoke(window, cx));
        }
        if std::env::var_os("BRISK_SMOKE_RUNMARKS").is_some() {
            cx.defer_in(window, |this: &mut Self, window, cx| this.run_marks_smoke(window, cx));
        }
        if std::env::var_os("BRISK_SMOKE_RUNTPL").is_some() {
            cx.defer_in(window, |this: &mut Self, window, cx| this.run_tpl_smoke(window, cx));
        }
        if std::env::var_os("BRISK_SMOKE_TESTRUNNERS").is_some() {
            cx.defer_in(window, |this: &mut Self, window, cx| this.test_runners_smoke(window, cx));
        }
        if std::env::var_os("BRISK_SMOKE_FLOOD").is_some() {
            cx.defer_in(window, |this: &mut Self, window, cx| this.flood_smoke(window, cx));
        }
        if std::env::var_os("BRISK_SMOKE_LATENCY").is_some() {
            cx.defer_in(window, |this: &mut Self, window, cx| this.latency_smoke(window, cx));
        }
        if std::env::var_os("BRISK_SMOKE_ANSI").is_some() {
            cx.defer_in(window, |this: &mut Self, window, cx| this.ansi_smoke(window, cx));
        }
        if std::env::var_os("BRISK_SMOKE_TESTVIEW").is_some() {
            cx.defer_in(window, |this: &mut Self, window, cx| this.test_view_smoke(window, cx));
        }
        if std::env::var_os("BRISK_SMOKE_RUNTERM").is_some() {
            cx.defer_in(window, |this: &mut Self, window, cx| this.run_term_smoke(window, cx));
        }
        if std::env::var_os("BRISK_SMOKE_PYCON").is_some() {
            cx.defer_in(window, |this: &mut Self, window, cx| this.pycon_smoke(window, cx));
        }
        if std::env::var_os("BRISK_SMOKE_MULTI").is_some() {
            cx.defer_in(window, |this: &mut Self, window, cx| this.multi_smoke(window, cx));
        }
        if std::env::var_os("BRISK_SMOKE_PIN").is_some() {
            cx.defer_in(window, |this: &mut Self, window, cx| this.pin_smoke(window, cx));
        }
        if std::env::var_os("BRISK_SMOKE_SERVICES").is_some() {
            cx.defer_in(window, |this: &mut Self, window, cx| this.services_smoke(window, cx));
        }
        if std::env::var_os("BRISK_SMOKE_CONSOLEFIND").is_some() {
            cx.defer_in(window, |this: &mut Self, window, cx| this.console_find_smoke(window, cx));
        }
        if std::env::var_os("BRISK_SMOKE_WEB").is_some() {
            cx.defer_in(window, |this: &mut Self, window, cx| this.web_smoke(window, cx));
        }
        if std::env::var_os("BRISK_SMOKE_BEFORE").is_some() {
            cx.defer_in(window, |this: &mut Self, window, cx| this.before_smoke(window, cx));
        }
        if std::env::var_os("BRISK_SMOKE_PROFILE").is_some() {
            cx.defer_in(window, |this: &mut Self, window, cx| this.profile_smoke(window, cx));
        }
        if std::env::var_os("BRISK_SMOKE_COVERAGE").is_some() {
            cx.defer_in(window, |this: &mut Self, window, cx| this.coverage_smoke(window, cx));
        }
        if std::env::var_os("BRISK_SMOKE_COMPOUND").is_some() {
            cx.defer_in(window, |this: &mut Self, window, cx| this.compound_smoke(window, cx));
        }
        if std::env::var_os("BRISK_SMOKE_ENVFILE").is_some() {
            cx.defer_in(window, |this: &mut Self, window, cx| this.envfile_smoke(window, cx));
        }
        if std::env::var_os("BRISK_SMOKE_TERMVENV").is_some() {
            cx.defer_in(window, |this: &mut Self, window, cx| this.terminal_env_smoke(window, cx));
        }
        if std::env::var_os("BRISK_SMOKE_RUNANYTHING").is_some() {
            cx.defer_in(window, |this: &mut Self, window, cx| this.run_anything_smoke(window, cx));
        }
        if std::env::var_os("BRISK_SMOKE_VENV").is_some() {
            cx.defer_in(window, |this: &mut Self, window, cx| this.venv_smoke(window, cx));
        }
        if std::env::var_os("BRISK_SMOKE_PYCONSOLE").is_some() {
            cx.defer_in(window, |this: &mut Self, window, cx| this.python_console_smoke(window, cx));
        }
        if std::env::var_os("BRISK_SMOKE_PKGQUEUE").is_some() {
            cx.defer_in(window, |this: &mut Self, window, cx| this.python_packages_queue_smoke(window, cx));
        }
        if std::env::var_os("BRISK_SMOKE_PACKAGES").is_some() {
            cx.defer_in(window, |this: &mut Self, window, cx| this.python_packages_smoke(window, cx));
        }
        if std::env::var_os("BRISK_SMOKE_TODO").is_some() {
            cx.defer_in(window, |this: &mut Self, window, cx| this.todo_smoke(window, cx));
        }
        if crate::stall::frames_enabled() {
            cx.defer_in(window, |this: &mut Self, window, cx| this.list_scroll_smoke(window, cx));
        }
        if std::env::var_os("BRISK_SMOKE_GITX").is_some() {
            cx.defer_in(window, |this: &mut Self, window, cx| this.gitx_smoke(window, cx));
        }
        if std::env::var_os("BRISK_SMOKE_VCS").is_some() {
            cx.defer_in(window, |this: &mut Self, window, cx| this.vcs_smoke(window, cx));
        }
        if std::env::var_os("BRISK_SMOKE_RUNWIN").is_some() {
            cx.defer_in(window, |this: &mut Self, window, cx| this.run_window_smoke(window, cx));
        }
        if std::env::var_os("BRISK_SMOKE_HANG").is_some() {
            cx.defer_in(window, |this: &mut Self, window, cx| this.hang_smoke(window, cx));
        }
        if std::env::var_os("BRISK_SMOKE_MEMBUDGET").is_some() {
            cx.defer_in(window, |this: &mut Self, window, cx| this.membudget_smoke(window, cx));
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
        let saved_bookmarks = bookmarks_ui::load(&root);
        let debug = debug_ui::Debug::new(&root, cx);
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
            popup_scroll: UniformListScrollHandle::new(),
            tree_bar: Default::default(),
            structure_view: Default::default(),
            files: None,
            tabs: Vec::new(),
            tab_scroll: [gpui::ScrollHandle::new(), gpui::ScrollHandle::new()],
            tab_seen: Default::default(),
            active: None,
            git,
            git_init_running: false,
            commit_focus: cx.focus_handle(),
            toast: None,
            toast_gen: 0,
            find: find_ui::Find::new(),
            db,
            services: services_ui::Services { selected: None, focus: cx.focus_handle() },
            debug,
            pyc: None,
            venv_busy: false,
            venv_offer: None,
            ra_history: Vec::new(),
            coverage: None,
            cov_pending: None,
            cov_offer: None,
            profile: None,
            prof_pending: None,
            before: None,
            before_passed: None,
            last_url: None,
            test_marks: Default::default(),
            packages: None,
            todo: None,
            debug_edit: None,
            recent: Vec::new(),
            bookmarks: saved_bookmarks,
            structure: Vec::new(),
            crumb_dir: (PathBuf::new(), String::new()),
            split: None,
            closed: Vec::new(),
            pinned: Default::default(),
            dir_stamps: Default::default(),
            left: Some(LeftTool::Project),
            bottom: None,
            popup: None,
            terminal: None,
            resolver: KeyResolver::new(pycharm_keymap()),
            focus,
            tree_focus,
            shift_alone: false,
            ctrl_tap: None,
            ctrl_alone: false,
            ctrl_clone: false,
            settings_dialog: None,
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
        self.all_tabs().filter_map(|t| match t {
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
        // The editable side of a live diff.
        if let Some(Tab::Diff(d)) = self.active.and_then(|i| self.tabs.get(i)) {
            if let Some(e) = d.read(cx).live_editor() {
                if e.read(cx).focus_handle(cx).is_focused(window) {
                    return Some(e);
                }
                if let Some(f) = e.read(cx).find_field_focused(window, cx) {
                    return Some(f);
                }
            }
        }
        let git_fields = self.git.as_ref().map(|g| [g.message.clone(), g.filter.field.clone()]).into_iter().flatten();
        let console = self.pyc.as_ref().map(|p| p.input.clone());
        let packages = self.packages.as_ref().map(|p| p.fields()).into_iter().flatten();
        git_fields.chain(console).chain(packages).find(|m| m.read(cx).focus_handle(cx).is_focused(window))
    }

    fn save_all(&mut self, cx: &mut Context<Self>) {
        self.actions_on_save(cx);
        let mut saved = 0;
        let mut saved_paths = Vec::new();
        let mut problem = None;
        let mut eds: Vec<_> = self.editors().cloned().collect();
        // Editors that live diffs opened for files without a tab.
        eds.extend(self.all_tabs().filter_map(|t| if let Tab::Diff(d) = t { d.read(cx).owned_editor() } else { None }));
        for t in eds {
            t.update(cx, |e, cx| {
                match e.save() {
                    Ok(true) => {
                        saved += 1;
                        saved_paths.extend(e.path.clone());
                    }
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
        self.todo_files_saved(saved_paths, cx);
        if saved > 0 {
            self.persist_bookmarks(cx);
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
        } else if self.activate_in_other_pane(&path, window, cx) {
        } else {
            let ed = cx.new(|cx| Editor::open(&path, cx));
            self.load_vcs_base(ed.clone(), cx);
            self.watch_editor(&ed, window, cx);
            self.adopt_bookmarks(&ed, cx);
            self.adopt_breakpoints(&ed, cx);
            ed.update(cx, |e, _| e.collapse_imports());
            if ed.read(cx).is_big() {
                let msg = format!("Large file ({} lines): inlay hints are off; fold markers and completion words update lazily", ed.read(cx).buffer.len_lines());
                self.status = msg.clone();
                self.show_toast(msg, false, cx);
            }
            self.add_tab(Tab::Editor(ed));
            self.enforce_tab_limit(window, cx);
        }
        if let Some(e) = self.active_editor() {
            let f = e.read(cx).focus_handle(cx);
            window.focus(&f);
        }
        window.set_window_title(&format!("{} – {}", self.project_name(), path.file_name().map(|n| n.to_string_lossy()).unwrap_or_default()));
        cx.notify();
    }

    fn add_tab(&mut self, tab: Tab) {
        let insert_at = self.active.map(|i| i + 1).unwrap_or(self.tabs.len()).min(self.tabs.len()).max(self.pinned_count());
        self.tabs.insert(insert_at, tab);
        self.active = Some(insert_at);
    }

    fn close_tab(&mut self, i: usize, window: &mut Window, cx: &mut Context<Self>) {
        if i >= self.tabs.len() {
            return;
        }
        // A live diff's own editor is written when the diff closes, like a tab's.
        if let Tab::Diff(d) = self.tabs[i].clone() {
            if let Some(e) = d.read(cx).owned_editor() {
                if let Err(err) = e.update(cx, |e, _| e.save()) {
                    let msg = format!("Cannot save {}: {err}", e.read(cx).title());
                    self.show_toast(msg, true, cx);
                }
            }
        }
        if let Tab::Editor(e) = self.tabs[i].clone() {
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
                self.remember_closed(p);
            }
            let e = e.clone();
            self.stash_bookmarks(&e, cx);
            self.stash_breakpoints(&e, cx);
        }
        self.tabs.remove(i);
        if self.tabs.is_empty() && self.split.is_some() {
            // The group is empty: the other one takes the whole area.
            self.collapse_empty_pane();
            self.focus_editor(window, cx);
            cx.notify();
            return;
        }
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
    pub(super) fn toggle_terminal(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        if self.bottom == Some(BottomTool::Terminal) && self.terminal_focused(window, cx) {
            self.bottom = None;
            self.focus_editor(window, cx);
            return;
        }
        let root = self.root.clone();
        let term = match &self.terminal {
            Some(t) if !t.read(cx).is_empty() => t.clone(),
            _ => {
                let activation = self.terminal_activation();
                let t = cx.new(|cx| TerminalPanel::new(root, activation, cx));
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

    /// The active diff tab while focus is anywhere in it (its live editor too): F7 / F4.
    pub(super) fn diff_in_focus(&self, window: &Window, cx: &App) -> Option<Entity<DiffView>> {
        match self.active.and_then(|i| self.tabs.get(i)) {
            Some(Tab::Diff(d)) if d.read(cx).focus_handle(cx).contains_focused(window, cx) => Some(d.clone()),
            _ => None,
        }
    }

    fn on_key_down(&mut self, e: &KeyDownEvent, window: &mut Window, cx: &mut Context<Self>) {
        self.shift_alone = false;
        self.ctrl_alone = false;
        self.ctrl_tap = None;
        if self.ctrl_clone && self.popup.is_none() && matches!(e.keystroke.key.as_str(), "up" | "down") {
            let m = e.keystroke.modifiers;
            if m.control && !m.shift && !m.alt && !m.platform {
                if let Some(ed) = self.focused_editor(window, cx) {
                    let a = if e.keystroke.key == "up" { Action::CloneCaretAbove } else { Action::CloneCaretBelow };
                    ed.update(cx, |ed, cx| ed.perform(a, window, cx));
                    cx.stop_propagation();
                    return;
                }
            }
        }
        if self.bottom == Some(BottomTool::Services) && self.services_key(e, window, cx) {
            cx.stop_propagation();
            return;
        }
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
        if self.git_dialog_open() {
            self.git_dialog_key(e, window, cx);
            cx.stop_propagation();
            return;
        }
        if self.runs.dialog.is_some() {
            self.run_dialog_key(e, window, cx);
            cx.stop_propagation();
            return;
        }
        if self.debug.chooser.is_some() {
            self.chooser_key(e, window, cx);
            cx.stop_propagation();
            return;
        }
        if self.debug.bp_dialog.is_some() {
            self.bp_dialog_key(e, window, cx);
            cx.stop_propagation();
            return;
        }
        if self.settings_dialog.is_some() {
            self.settings_key(e, window, cx);
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
        if self.popup.is_none() && self.debug_key(e, window, cx) {
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
        if self.popup.is_none() && self.bookmark_jump_key(&e.keystroke, window, cx) {
            cx.stop_propagation();
            cx.notify();
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
        if self.python_console_key(e, window, cx) || self.packages_key(e, window, cx) || self.todo_key(e, window, cx) {
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
            A::SelectRunConfiguration, A::TogglePythonConsole, A::TogglePythonPackages, A::ToggleTodoWindow,
        ];
        let Some(term) = term else { return };
        let k = &e.keystroke;
        let plain_esc = k.key == "escape" && !k.modifiers.shift && !k.modifiers.control && !k.modifiers.alt;
        if plain_esc && !term.read(cx).alt_screen() && !term.read(cx).search_open() {
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
        if self.track_ctrl(m) {
            // Ctrl tapped twice with nothing in between: Run Anything, like PyCharm.
            self.dispatch(Action::RunAnything, window, cx);
            return;
        }
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
        if crate::stall::enabled() {
            crate::stall::note(format!("action {action:?}"));
        }
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
            A::OpenFile => self.open_file_dialog(window, cx),
            A::MainMenu => {
                let entries = self.main_menu_entries();
                self.open_menu(gpui::point(px(8.), px(40.)), entries, cx);
            }
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
            A::Commit | A::Push | A::UpdateProject | A::VcsQuickList | A::Branches | A::ShowDiff | A::RollbackChanges | A::Annotate | A::ResolveConflicts | A::StashChanges | A::UnstashChanges | A::ShowFileHistory | A::CompareWithBranch | A::Fetch => {
                self.git_action(action, window, cx)
            }
            A::ToggleStructureWindow => self.toggle_left(LeftTool::Structure, window, cx),
            A::ToggleBookmarksWindow => self.toggle_left(LeftTool::Bookmarks, window, cx),
            A::RunAnything => self.open_run_anything(),
            A::ToggleTerminalWindow => self.toggle_bottom(BottomTool::Terminal, window, cx),
            A::ToggleRunWindow | A::Run | A::RunContext | A::Rerun | A::SelectRunConfiguration | A::EditRunConfigurations => {
                self.run_action(action, window, cx)
            }
            A::ToggleDebugWindow => self.toggle_bottom(BottomTool::Debug, window, cx),
            A::ToggleProblemsWindow => self.toggle_problems(window, cx),
            A::NextError | A::PreviousError => self.goto_problem(action == A::NextError, window, cx),
            A::ShowIntentionActions => self.show_intentions(window, cx),
            A::SurroundWith | A::InsertLiveTemplate => self.template_action(action, window, cx),
            A::Unwrap => self.unwrap_action(window, cx),
            A::ToggleGitWindow => {
                self.toggle_bottom(BottomTool::Git, window, cx);
                if self.bottom == Some(BottomTool::Git) {
                    if let Some(g) = &self.git {
                        window.focus(&g.log_focus);
                    }
                    self.refresh_git(cx);
                }
            }
            A::TogglePythonConsole => self.toggle_python_console(window, cx),
            A::TogglePythonPackages => self.toggle_python_packages(window, cx),
            A::ToggleTodoWindow => self.toggle_todo_window(window, cx),
            A::ToggleCoverageWindow => self.toggle_coverage_window(window, cx),
            A::RunWithCoverage => self.run_with_coverage(window, cx),
            A::Profile => self.profile_run(window, cx),
            A::ToggleProfilerWindow => self.toggle_profiler_window(window, cx),
            A::RefreshPythonPackages | A::InstallPythonPackage | A::UninstallPythonPackage => self.python_packages_action(action, window, cx),
            A::RerunPythonConsole | A::StopPythonConsole | A::ClearPythonConsole => self.python_console_action(action, window, cx),
            A::ToggleServicesWindow => self.toggle_services_window(window, cx),
            A::ToggleDatabaseWindow | A::NewDataSource | A::DataSourceProperties | A::OpenQueryConsole | A::EditTableData
            | A::RefreshDatabase | A::Disconnect | A::ExecuteQuery => self.db_action(action, window, cx),
            A::Stop if self.db.running.is_some() => {
                if let Some(c) = &self.db.running {
                    c.cancel();
                }
                self.status = "Cancelling the running statement…".into();
            }
            A::Stop if self.debug.is_live() => {
                self.debug_action(action, window, cx);
            }
            A::Stop => self.run_action(action, window, cx),
            A::Copy if self.focused_grid(window, cx).is_some() => {
                if let Some(g) = self.focused_grid(window, cx) {
                    g.update(cx, |g, cx| g.copy(cx));
                }
            }
            A::ToggleFindWindow => self.toggle_find_window(window, cx),
            A::PasteFromHistory if self.focused_editor(window, cx).is_some() => self.open_popup(PopupKind::PasteHistory),
            A::ShowLocalHistory => self.show_local_history(cx),
            A::PutLabel => self.start_put_label(),
            A::ToggleBreakpoint | A::ViewBreakpoints | A::Debug | A::DebugContext | A::Resume | A::StepOver | A::StepInto | A::StepOut | A::RunToCursor | A::EvaluateExpression
            | A::PauseProgram | A::RerunDebug | A::AddToWatches | A::MuteBreakpoints | A::SmartStepInto | A::AttachToProcess | A::SetNextStatement => {
                self.debug_action(action, window, cx);
            }
            A::ToggleBookmark | A::ToggleBookmarkWithMnemonic | A::ShowBookmarks | A::FileStructurePopup => {
                self.bookmark_action(action, window, cx);
            }
            A::CloseOtherTabs | A::CloseAllTabs | A::CloseTabsToRight | A::CloseTabsToLeft | A::ReopenClosedTab | A::PinTab | A::SplitVertically
            | A::SplitHorizontally | A::Unsplit | A::MoveToOppositeGroup | A::NextSplitter => {
                self.tab_action(action, window, cx);
            }
            A::ReformatCode | A::OptimizeImports | A::CodeCleanup => self.format_action(action, window, cx),
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
            A::Settings | A::IncreaseFontSize | A::DecreaseFontSize | A::ResetFontSize | A::QuickSwitchTheme | A::ZoomInIde | A::ZoomOutIde | A::ResetIdeZoom => {
                self.settings_action(action, window, cx);
            }
            A::NewElement | A::ProjectStructure => {
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

    /// A press on a tree row: select at once; a double press opens or toggles; a press on the
    /// chevron toggles.
    fn tree_press(&mut self, id: NodeId, clicks: usize, chevron: bool, window: &mut Window, cx: &mut Context<Self>) {
        self.tree_selected = Some(id);
        window.focus(&self.tree_focus);
        let double = clicks >= 2;
        if self.tree.nodes[id].is_dir {
            if double || chevron {
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
            PopupKind::FileStructure | PopupKind::Bookmarks | PopupKind::LocalHistory => self.structure_items(&q),
            PopupKind::BookmarkMnemonic => self.mnemonic_items(&q),
            PopupKind::PasteHistory => clip_items(&q),
            PopupKind::BreakpointCondition => self.condition_items(&q),
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
            PopupKind::AddInterpreter => self.add_interpreter_items(&q),
            PopupKind::RunAnything => self.run_anything_items(&q),
            PopupKind::Intentions => self.intention_items(&q),
            PopupKind::Rename => self.rename_items(&q),
            PopupKind::FolderCrumb => self.folder_items(&q),
            PopupKind::PutLabel => self.put_label_items(&q),
            PopupKind::BackgroundTasks => self.bg_task_items(&q),
            PopupKind::TabList => self.structure_items(&q),
        };
        let p = self.popup.as_mut().unwrap();
        p.items = items;
        p.selected = p.selected.min(p.items.len().saturating_sub(1));
    }

    pub(super) fn popup_key(&mut self, e: &KeyDownEvent, window: &mut Window, cx: &mut Context<Self>) {
        let k = &e.keystroke;
        let Some(p) = self.popup.as_mut() else { return };
        match k.key.as_str() {
            "escape" => {
                self.popup = None;
                self.focus_editor(window, cx);
            }
            "up" => {
                p.selected = p.selected.saturating_sub(1);
                self.popup_scroll.scroll_to_item(p.selected, gpui::ScrollStrategy::Top);
            }
            "down" => {
                p.selected = (p.selected + 1).min(p.items.len().saturating_sub(1));
                self.popup_scroll.scroll_to_item(p.selected, gpui::ScrollStrategy::Top);
            }
            // Branches popup: → shows the actions of the selected branch.
            "right" if p.kind == PopupKind::Branches => {
                if let Some(Target::Branch(b)) = p.items.get(p.selected).map(|i| i.target.clone()) {
                    self.open_branch_actions(&b);
                }
            }
            "right" | "left" if p.kind == PopupKind::FolderCrumb => {
                let right = k.key == "right";
                self.folder_crumb_step(right);
                cx.notify();
            }
            "tab" => {
                if let PopupKind::SearchEverywhere(t) = p.kind {
                    self.set_se_tab(t.next());
                }
            }
            "enter" => {
                let target = p.items.get(p.selected).map(|i| i.target.clone());
                if let (true, Some(Target::History(path, at))) = (k.modifiers.control, target.clone()) {
                    self.popup = None;
                    self.history_revert(path, at, window, cx);
                    cx.notify();
                    return;
                }
                if let Some(Target::Dir(_)) = target {
                    self.folder_crumb_step(true);
                    cx.notify();
                    return;
                }
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
                    Some(Target::StopAndRerun(c)) => self.stop_and_rerun(*c, window, cx),
                    Some(t @ (Target::RunConfig(..) | Target::EditRunConfigs | Target::SaveRunConfig(_) | Target::Interpreter(_))) => self.run_popup_pick(t, window, cx),
                    Some(Target::AddInterpreter) => {
                        self.open_popup(PopupKind::AddInterpreter);
                        cx.notify();
                    }
                    Some(Target::RunAnything(line)) => {
                        self.popup = None;
                        self.run_anything(line, window, cx);
                    }
                    Some(Target::CreateVenv(base)) => {
                        self.popup = None;
                        self.create_venv(base, cx);
                        self.focus_editor(window, cx);
                    }
                    Some(Target::Intention(i)) => self.apply_intent(*i, window, cx),
                    Some(Target::Rename(n)) => self.apply_rename(n, window, cx),
                    Some(Target::Project(c)) => {
                        self.focus_editor(window, cx);
                        self.project_cmd(c, window, cx);
                    }
                    Some(Target::Symbol(path, line, col, _)) => self.open_symbol(path, line, col, window, cx),
                    Some(Target::FileDo(t)) => self.run_file_op(t, window, cx),
                    Some(Target::Bookmark(path, line)) => self.open_symbol(path, line, 0, window, cx),
                    Some(Target::Mnemonic(c)) => self.pick_mnemonic(c, window, cx),
                    Some(Target::History(path, at)) => self.history_diff(path, at, window, cx),
                    Some(Target::PutLabel(name)) => {
                        self.put_label(name, cx);
                        self.focus_editor(window, cx);
                    }
                    Some(t @ (Target::BpCondition(_) | Target::BpRemove)) => self.pick_condition(t, window, cx),
                    Some(Target::Clip(i)) => {
                        self.focus_editor(window, cx);
                        if let (Some(t), Some(ed)) = (crate::editor::clipboard_history().get(i).cloned(), self.active_editor().cloned()) {
                            ed.update(cx, |e, cx| e.paste_text(&t, cx));
                        }
                    }
                    Some(Target::Tab(i)) => {
                        if i < self.tabs.len() {
                            self.save_all(cx);
                            self.active = Some(i);
                        }
                        self.focus_editor(window, cx);
                    }
                    Some(Target::Dir(_) | Target::Task) | None => self.focus_editor(window, cx),
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

    fn stripe_button(&self, id: &'static str, icon: &'static str, tip: &'static str, active: bool, action: Action, cx: &mut Context<Self>) -> impl IntoElement {
        let shortcut = self.resolver.keymap().shortcut_label(action).unwrap_or_default();
        let tooltip: SharedString = format!("{tip}  {shortcut}").into();
        div()
            .id(id)
            .w(px(30.))
            .h(ui_px(30.))
            .flex()
            .items_center()
            .justify_center()
            .rounded(px(5.))
            .when(active && !crate::theme::islands(), |d| d.bg(Theme::selection_row()))
            .when(crate::theme::islands(), |d| d.size(ui_px(32.)).rounded(px(8.)))
            .when(active && crate::theme::islands(), |d| d.bg(Theme::text().opacity(0.14)))
            .when(!active, |d| d.hover(|s| s.bg(Theme::hover_row())))
            .active(|s| s.bg(Theme::press()))
            .cursor_pointer()
            .on_click(cx.listener(move |this, _, window, cx| {
                if active {
                    this.hide_tool_window(ToolSide::of(action), window, cx);
                } else {
                    this.dispatch(action, window, cx);
                }
            }))
            .tooltip(move |_window, cx| cx.new(|_| Tip(tooltip.clone())).into())
            .child(crate::icons::ui_icon(icon, 16., if active { Theme::text() } else { Theme::text_muted() }))
            .map(|d| crate::ui_kit::pill_in(d, gpui::ElementId::Name(format!("pill-{id}").into()), active, Theme::text().opacity(0.14)))
    }

    /// Hides the tool window on one side and gives the editor the focus.
    fn hide_tool_window(&mut self, side: ToolSide, window: &mut Window, cx: &mut Context<Self>) {
        match side {
            ToolSide::Left => self.left = None,
            ToolSide::Bottom => self.bottom = None,
            ToolSide::Right => self.db.open = false,
        }
        self.focus_editor(window, cx);
        cx.notify();
    }

    /// A narrow column on a tool window's edge with its "Hide" button (—), as in PyCharm's
    /// tool window header.
    fn hide_column(&self, side: ToolSide, cx: &mut Context<Self>) -> impl IntoElement {
        let id = match side {
            ToolSide::Left => "hide-left",
            ToolSide::Bottom => "hide-bottom",
            ToolSide::Right => "hide-right",
        };
        div().w(px(28.)).h_full().flex_shrink_0().flex().flex_col().items_center().pt(px(6.)).child(
            div()
                .id(id)
                .w(px(22.))
                .h(ui_px(22.))
                .flex()
                .items_center()
                .justify_center()
                .rounded(px(4.))
                .text_size(ui_px(13.))
                .text_color(Theme::text_muted())
                .cursor_pointer()
                .hover(|s| s.bg(Theme::hover_row()).text_color(Theme::text()))
                .active(|s| s.bg(Theme::press()))
                .tooltip(|_, cx| cx.new(|_| Tip("Hide  Shift+Esc".into())).into())
                .child("—")
                .on_mouse_down(MouseButton::Left, |_, _, cx| cx.stop_propagation())
                .on_click(cx.listener(move |this, _, window, cx| this.hide_tool_window(side, window, cx))),
        )
    }

    fn render_toolbar(&self, cx: &mut Context<Self>) -> impl IntoElement {
        // A toolbar widget: text plus a small dimmed chevron, vertically centered.
        let chip = |id: &'static str, lead: Option<&'static str>, text: String| {
            div()
                .id(id)
                .h(ui_px(28.))
                .px(px(8.))
                .flex()
                .items_center()
                .gap(px(5.))
                .rounded(px(5.))
                .hover(|s| s.bg(Theme::hover_row()))
                .active(|s| s.bg(Theme::press()))
                .cursor_pointer()
                .when_some(lead, |d, n| d.child(crate::icons::ui_icon(n, 14., Theme::text_muted())))
                .child(text)
                .child(crate::icons::ui_icon("chevron-down", 12., Theme::text_muted()))
        };
        div()
            .h(ui_px(40.))
            .flex()
            .items_center()
            .px(px(10.))
            .gap(px(6.))
            .bg(if crate::theme::islands() { Theme::frame() } else { Theme::panel() })
            .when(!crate::theme::islands(), |d| d.border_b_1().border_color(Theme::border()))
            .text_size(ui_px(13.))
            .text_color(Theme::text())
            // The main menu, as in PyCharm's new UI.
            .child(
                div()
                    .id("tb-main-menu")
                    .w(px(28.))
                    .h(ui_px(28.))
                    .flex()
                    .items_center()
                    .justify_center()
                    .rounded(px(5.))
                    .cursor_pointer()
                    .hover(|s| s.bg(Theme::hover_row()))
                    .active(|s| s.bg(Theme::press()))
                    .tooltip(|_, cx| cx.new(|_| Tip("Main Menu".into())).into())
                    .child(crate::icons::ui_icon("menu", 16., Theme::text_muted()))
                    .on_mouse_down(MouseButton::Left, cx.listener(|this, _, w, cx| {
                        cx.stop_propagation();
                        this.dispatch(Action::MainMenu, w, cx);
                    })),
            )
            .child(
                div().w(px(24.)).h(ui_px(24.)).rounded(px(5.)).bg(Theme::accent()).flex().items_center().justify_center().text_size(ui_px(11.)).font_weight(gpui::FontWeight::BOLD).child(
                    self.project_name().chars().take(2).collect::<String>().to_uppercase(),
                ),
            )
            .child(chip("tb-project", None, self.project_name()).on_click(cx.listener(|this, _, _, cx| {
                this.open_popup(PopupKind::Projects);
                cx.notify();
            })))
            .child(
                chip("tb-branch", Some("git-branch"), self.branch_label())
                    .on_click(cx.listener(|this, _, w, cx| this.dispatch(Action::Branches, w, cx))),
            )
            .child(div().flex_1())
            .child(
                chip("tb-config", None, self.run_widget_label())
                    .on_click(cx.listener(|this, _, _, cx| {
                        this.reload_runs();
                        this.open_popup(PopupKind::RunConfigs(false));
                        cx.notify();
                    })),
            )
            .child(
                title_button("tb-run", "play", Theme::run_icon(), "Run")
                    .on_click(cx.listener(|this, _, w, cx| this.dispatch(Action::Run, w, cx))),
            )
            .child(
                title_button("tb-debug", "bug", Theme::run_icon(), "Debug")
                    .on_click(cx.listener(|this, _, w, cx| this.dispatch(Action::Debug, w, cx))),
            )
            .child(
                title_button("tb-search", "search", Theme::text_muted(), "Search Everywhere  Double Shift")
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
            .bg(if crate::theme::islands() { Theme::frame() } else { Theme::panel() })
            .when(!crate::theme::islands(), |d| d.border_r_1().border_color(Theme::border()))
            .child(self.stripe_button("st-project", "folder", "Project", self.left == Some(LeftTool::Project), Action::ToggleProjectWindow, cx))
            .child(self.stripe_button("st-commit", "git-commit", "Commit", self.left == Some(LeftTool::Commit), Action::ToggleCommitWindow, cx))
            .child(self.stripe_button("st-structure", "list-tree", "Structure", self.left == Some(LeftTool::Structure), Action::ToggleStructureWindow, cx))
            .child(self.stripe_button("st-bookmarks", "bookmark", "Bookmarks", self.left == Some(LeftTool::Bookmarks), Action::ToggleBookmarksWindow, cx))
            .child(div().flex_1())
            .child(self.stripe_button("st-pyconsole", "terminal", "Python Console", self.bottom == Some(BottomTool::PythonConsole), Action::TogglePythonConsole, cx))
            .child(self.stripe_button("st-packages", "package", "Python Packages", self.bottom == Some(BottomTool::Packages), Action::TogglePythonPackages, cx))
            .child(self.stripe_button("st-services", "server", "Services", self.bottom == Some(BottomTool::Services), Action::ToggleServicesWindow, cx))
            .child(self.stripe_button("st-run", "play", "Run", self.bottom == Some(BottomTool::Run), Action::ToggleRunWindow, cx))
            .child(self.stripe_button("st-terminal", "square-terminal", "Terminal", self.bottom == Some(BottomTool::Terminal), Action::ToggleTerminalWindow, cx))
            .child(self.stripe_button("st-todo", "list-todo", "TODO", self.bottom == Some(BottomTool::Todo), Action::ToggleTodoWindow, cx))
            .when(self.coverage.is_some(), |d| d.child(self.stripe_button("st-coverage", "chart-pie", "Coverage", self.bottom == Some(BottomTool::Coverage), Action::ToggleCoverageWindow, cx)))
            .when(self.profile.is_some(), |d| d.child(self.stripe_button("st-profiler", "timer", "Profiler", self.bottom == Some(BottomTool::Profiler), Action::ToggleProfilerWindow, cx)))
            .child(self.stripe_button("st-problems", "triangle-alert", "Problems", self.bottom == Some(BottomTool::Problems), Action::ToggleProblemsWindow, cx))
            .child(self.stripe_button("st-git", "git-branch", "Git", self.bottom == Some(BottomTool::Git), Action::ToggleGitWindow, cx))
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
            .bg(if crate::theme::islands() { Theme::frame() } else { Theme::panel() })
            .when(!crate::theme::islands(), |d| d.border_l_1().border_color(Theme::border()))
            .child(self.stripe_button("st-database", "database", "Database", self.db.open, Action::ToggleDatabaseWindow, cx))
    }

    fn panel_header(title: &str) -> impl IntoElement {
        div()
            .h(px(crate::ui_kit::header_h()))
            .flex()
            .items_center()
            .px(px(12.))
            .text_size(ui_px(13.))
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
            .child(div().relative().flex_1().min_h_0().flex().flex_col().child(
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
                                let chevron = crate::icons::tree_arrow(node.is_dir, node.expanded, Theme::text_muted());
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
                                    .h(ui_px(24.))
                                    .flex()
                                    .items_center()
                                    .pl(px(8. + r.depth as f32 * 16.))
                                    .gap(px(4.))
                                    .text_size(ui_px(13.))
                                    .w_full()
                                    .when(selected, |d| d.bg(if focused { Theme::selection_row() } else { Theme::hover_row() }))
                                    .when(!selected, |d| d.hover(|s| s.bg(Theme::hover_row())))
                                    .on_mouse_down(gpui::MouseButton::Left, cx.listener(move |this, e: &gpui::MouseDownEvent, window, cx| this.tree_press(id, e.click_count, false, window, cx)))
                                    .on_mouse_down(gpui::MouseButton::Right, cx.listener(move |this, e: &gpui::MouseDownEvent, window, cx| this.tree_menu(id, e.position, window, cx)))
                                    .child(
                                        div()
                                            .w(px(12.))
                                            .text_color(Theme::text_muted())
                                            .hover(|s| s.text_color(Theme::text()))
                                            .on_mouse_down(gpui::MouseButton::Left, cx.listener(move |this, e: &gpui::MouseDownEvent, window, cx| {
                                                cx.stop_propagation();
                                                this.tree_press(id, e.click_count, true, window, cx);
                                            }))
                                            .child(chevron),
                                    )
                                    .child(div().w(px(16.)).flex().items_center().child(crate::icons::glyph(icon, 14., icon_color)))
                                    .child(div().text_color(name_color).whitespace_nowrap().child(label))
                            })
                            .collect()
                    }),
                )
                .track_scroll(self.tree_scroll.clone())
                .flex_1(),
            ).child(crate::scrollbar::scrollbar(crate::scrollbar::Source::list(&self.tree_scroll), self.tree_bar.clone())))
    }

    /// The active editor notified: repaint Structure when its caret moved to another line, so
    /// the highlighted item follows it.
    pub(super) fn structure_follow_caret(&mut self, ed: &Entity<Editor>, cx: &mut Context<Self>) {
        if self.left != Some(LeftTool::Structure) || self.active_editor() != Some(ed) {
            return;
        }
        let line = ed.read(cx).caret_line_col().0;
        if self.structure_view.caret != Some(line) {
            self.structure_view.caret = Some(line);
            cx.notify();
        }
    }

    /// Structure (Alt+7): the file's classes and functions. The item around the caret is
    /// highlighted and scrolled into view as the caret moves (PyCharm's Autoscroll from Source).
    fn render_structure(&mut self, cx: &mut Context<Self>) -> impl IntoElement {
        let ed = self.active_editor().cloned();
        let (items, chain) = ed.as_ref().map(|e| e.update(cx, |e, _| e.caret_scope())).unwrap_or_default();
        let current = chain.last().copied();
        if current != self.structure_view.follow {
            if let Some(i) = current {
                self.structure_view.scroll.scroll_to_item(i);
            }
            // Before the list's first layout GPUI cannot scroll it yet: try again next frame.
            if self.structure_view.scroll.bounds().size.height > px(0.) {
                self.structure_view.follow = current;
            } else {
                cx.notify();
            }
        }
        let rows = items.iter().cloned().enumerate().map(move |(i, it)| {
            let (glyph, color) = match it.kind {
                brisk_syntax::OutlineKind::Class => ("C", gpui::rgb(0x6c9be8)),
                brisk_syntax::OutlineKind::Function => ("f", gpui::rgb(0xe0a85a)),
                brisk_syntax::OutlineKind::Method => ("m", gpui::rgb(0xcf8e6d)),
                brisk_syntax::OutlineKind::Variable => ("v", gpui::rgb(0x9876aa)),
            };
            let ed = ed.clone();
            let line = it.line;
            div()
                .id(("outline", i))
                .h(ui_px(24.))
                .flex_none()
                .flex()
                .items_center()
                .gap(px(6.))
                .pl(px(12. + it.depth as f32 * 16.))
                .text_size(ui_px(13.))
                .when(current == Some(i), |d| d.bg(Theme::selection_row()))
                .hover(|s| s.bg(Theme::hover_row()))
                .cursor_pointer()
                .on_click(cx.listener(move |this, _, window, cx| {
                    if let Some(ed) = &ed {
                        ed.update(cx, |e, cx| e.go_to_line(line, cx));
                    }
                    this.focus_editor(window, cx);
                }))
                .child(div().w(px(16.)).h(ui_px(16.)).rounded(px(8.)).bg(color).text_color(Theme::bg()).text_size(ui_px(10.)).flex().items_center().justify_center().child(glyph))
                .child(it.name)
        });
        div()
            .id("structure")
            .flex()
            .flex_col()
            .size_full()
            .child(Self::panel_header("Structure"))
            .child(
                div()
                    .relative()
                    .flex_1()
                    .min_h_0()
                    .child(div().id("structure-list").size_full().flex().flex_col().overflow_y_scroll().track_scroll(&self.structure_view.scroll).children(rows))
                    .child(crate::scrollbar::scrollbar(crate::scrollbar::Source::Handle(self.structure_view.scroll.clone()), self.structure_view.bar.clone())),
            )
    }

    fn render_placeholder(title: &str, body: &str) -> gpui::AnyElement {
        div()
            .flex()
            .flex_col()
            .size_full()
            .child(Self::panel_header(title))
            .map(|d| if crate::theme::islands() {
                d.child(crate::ui_kit::empty_state("list-todo", body.to_string()))
            } else {
                d.child(div().px(px(12.)).text_size(ui_px(13.)).text_color(Theme::text_muted()).child(body.to_string()))
            })
            .into_any_element()
    }

    fn render_tabs(&self, tabs: &[Tab], current: Option<usize>, focused: bool, second: bool, cx: &mut Context<Self>) -> impl IntoElement {
        let scroll = self.tab_scroll[second as usize].clone();
        // A tab that became active (opened, picked by Ctrl+E, Ctrl+Tab...) scrolls into view;
        // otherwise the strip stays where the user scrolled it.
        let seen = (current, tabs.len());
        if self.tab_seen[second as usize].get() != seen {
            if let Some(i) = current {
                scroll.scroll_to_item(i);
            }
            // Before the strip's first layout GPUI cannot scroll it yet: try again next frame.
            if scroll.bounds().size.width > px(0.) {
                self.tab_seen[second as usize].set(seen);
            } else {
                cx.notify();
            }
        }
        let wheel = scroll.clone();
        let islands = crate::theme::islands();
        let rows = crate::settings::tab_rows();
        // Some tabs do not fit (as of the last layout): the ▾ list at the strip's end.
        let overflow = !rows && scroll.max_offset().width > px(0.5);
        let strip = div()
            .id(("tabs", second as usize))
            .flex_1()
            .min_w_0()
            .flex()
            .map(|d| if rows {
                d.flex_wrap().min_h(ui_px(36.)).when(islands, |d| d.py(px(4.)).gap_y(px(2.)))
            } else {
                d.h(ui_px(36.)).overflow_x_scroll().track_scroll(&scroll)
            })
            .items_end()
            // A plain mouse wheel scrolls the strip sideways, as in PyCharm.
            .on_scroll_wheel(move |e, window, cx| {
                let d = e.delta.pixel_delta(px(20.));
                if d.x == px(0.) && d.y != px(0.) {
                    let max = wheel.max_offset().width;
                    let mut o = wheel.offset();
                    o.x = (o.x + d.y).clamp(-max, px(0.));
                    wheel.set_offset(o);
                    window.refresh();
                    cx.stop_propagation();
                }
            })
            // Islands: pills on the editor's own background.
            .when(islands, |d| d.items_center().px(px(6.)).gap_x(px(2.)))
            .children(tabs.iter().enumerate().map(|(i, t)| {
                let active = current == Some(i);
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
                    Tab::Data(v) => (v.read(cx).title.clone(), "▦", gpui::rgb(0xd0a35c).into(), None, false),
                };
                let name_color = name_color.unwrap_or(if active { Theme::text() } else { Theme::text_muted() });
                let pinned = self.pinned.contains(&t.entity_id());
                let dragged = tabs_ui::DraggedTab { ix: i, second, title: title.clone().into() };
                let tab = div()
                    .id(("tab", i))
                    .h(px(if islands { 26. } else { 36. }))
                    .when(islands, |d| d.rounded(px(6.)))
                    .flex_shrink_0()
                    // Long names are cut with an ellipsis, as in PyCharm, so one tab cannot
                    // take the whole strip.
                    .max_w(px(260.))
                    .whitespace_nowrap()
                    .flex()
                    .items_center()
                    .gap(px(6.))
                    .px(px(12.))
                    .text_size(ui_px(13.))
                    .text_color(if active { Theme::text() } else { Theme::text_muted() })
                    .when(active && !islands, |d| d.border_b_2().border_color(if focused { Theme::accent() } else { Theme::text_dim() }).bg(Theme::editor_bg()))
                    .when(active && islands, |d| d.bg(if focused { Theme::selection_row() } else { Theme::hover_row() }))
                    .when(!active, |d| d.hover(|s| s.bg(Theme::hover_row())))
                    .cursor_pointer()
                    .on_mouse_down(MouseButton::Left, cx.listener(move |this, _, window, cx| {
                        this.focus_pane(second, window, cx);
                        if this.active != Some(i) {
                            this.save_all(cx);
                            this.active = Some(i);
                        }
                        this.focus_editor(window, cx);
                        cx.notify();
                    }))
                    .on_mouse_down(MouseButton::Middle, cx.listener(move |this, _, window, cx| {
                        this.focus_pane(second, window, cx);
                        this.close_tab(i, window, cx);
                    }))
                    .on_mouse_down(MouseButton::Right, cx.listener(move |this, e: &gpui::MouseDownEvent, window, cx| {
                        cx.stop_propagation();
                        this.tab_menu(second, i, e.position, window, cx);
                    }))
                    .child(div().flex_shrink_0().flex().items_center().child(crate::icons::glyph(icon, 14., color)))
                    .child(div().min_w_0().overflow_hidden().text_ellipsis().text_color(name_color).child(title))
                    .when(dirty, |d| d.child(div().flex_shrink_0().text_color(Theme::text_muted()).child("•")))
                    .when(pinned, |d| {
                        d.child(
                            div()
                                .id(("tab-pin", i))
                                .flex_shrink_0()
                                .w(px(16.))
                                .h(ui_px(16.))
                                .flex()
                                .items_center()
                                .justify_center()
                                .rounded(px(4.))
                                .text_size(ui_px(11.))
                                .text_color(Theme::accent())
                                .hover(|s| s.bg(Theme::press()))
                                .tooltip(|_, cx| cx.new(|_| Tip("Unpin Tab".into())).into())
                                .child("⚲")
                                .on_mouse_down(MouseButton::Left, |_, _, cx| cx.stop_propagation())
                                .on_click(cx.listener(move |this, _, window, cx| {
                                    cx.stop_propagation();
                                    this.focus_pane(second, window, cx);
                                    this.active = Some(i);
                                    this.tab_action(Action::PinTab, window, cx);
                                })),
                        )
                    })
                    .when(!pinned, |d| d.child(
                        div()
                            .id(("tab-close", i))
                            .flex_shrink_0()
                            .w(px(16.))
                            .h(ui_px(16.))
                            .flex()
                            .items_center()
                            .justify_center()
                            .rounded(px(4.))
                            .text_color(Theme::text_dim())
                            .hover(|s| s.bg(Theme::press()))
                            .group("close-x")
                            .child(crate::icons::close_x("close-x", 12.))
                            // The press must not also activate the tab underneath.
                            .on_mouse_down(MouseButton::Left, |_, _, cx| cx.stop_propagation())
                            .on_click(cx.listener(move |this, _, window, cx| {
                                cx.stop_propagation();
                                this.focus_pane(second, window, cx);
                                this.close_tab(i, window, cx);
                            })),
                    ))
                    .map(|d| crate::ui_kit::pill_in(d, ("tab-pill", i + 1000 * second as usize), active, if focused { Theme::selection_row() } else { Theme::hover_row() }));
                // Drag to reorder. The drag lives on a wrapper: the pill's fade-in wraps an
                // active tab in an animation element, which changes the tab's element id path
                // on the press that activates it (GPUI would forget the pending drag).
                div()
                    .id(("tab-dnd", i))
                    .flex_shrink_0()
                    .rounded(px(6.))
                    .on_drag(dragged, |d, _, _, cx| cx.new(|_| d.clone()))
                    .drag_over::<tabs_ui::DraggedTab>(|s, _, _, _| s.bg(Theme::hover_row()))
                    .on_drop(cx.listener(move |this, d: &tabs_ui::DraggedTab, window, cx| this.move_tab(d, i, second, window, cx)))
                    .child(tab)
            }));
        div()
            .flex_shrink_0()
            .flex()
            .items_center()
            .when(!islands, |d| d.bg(Theme::panel_alt()).border_b_1().border_color(Theme::divider()))
            .when(islands, |d| d.bg(Theme::editor_bg()))
            .child(strip)
            .when(overflow, |d| {
                d.child(
                    div()
                        .id(("tabs-more", second as usize))
                        .flex_shrink_0()
                        .mx(px(4.))
                        .size(ui_px(24.))
                        .flex()
                        .items_center()
                        .justify_center()
                        .rounded(px(5.))
                        .cursor_pointer()
                        .hover(|s| s.bg(Theme::hover_row()))
                        .tooltip(|_, cx| cx.new(|_| Tip("Show Hidden Tabs".into())).into())
                        .child(crate::icons::ui_icon("chevron-down", 14., Theme::text_muted()))
                        .on_mouse_down(MouseButton::Left, |_, _, cx| cx.stop_propagation())
                        .on_click(cx.listener(move |this, _, window, cx| {
                            this.focus_pane(second, window, cx);
                            this.structure = this.tab_list_items(cx);
                            this.open_popup(PopupKind::TabList);
                            if let (Some(a), Some(p)) = (this.active, this.popup.as_mut()) {
                                p.selected = p.items.iter().position(|it| matches!(it.target, Target::Tab(t) if t == a)).unwrap_or(0);
                            }
                            cx.notify();
                        })),
                )
            })
    }

    fn render_welcome(&self) -> impl IntoElement {
        // A two-column grid: labels right-aligned, shortcuts left-aligned, equal rows.
        let km = self.resolver.keymap();
        let items = [
            ("Search Everywhere", Action::SearchEverywhere),
            ("Go to File", Action::GotoFile),
            ("Recent Files", Action::RecentFiles),
            ("Find Action", Action::FindAction),
            ("Project View", Action::ToggleProjectWindow),
            ("Terminal", Action::ToggleTerminalWindow),
        ];
        let cell = || div().h(ui_px(30.)).flex().items_center();
        div()
            .size_full()
            .flex()
            .items_center()
            .justify_center()
            .bg(Theme::editor_bg())
            .font_family(ui_font())
            .text_size(ui_px(13.))
            .when(crate::theme::islands(), |d| d.flex_col().gap(px(20.)).child(crate::icons::ui_icon("file-text", 40., Theme::text_dim().opacity(0.6))))
            .child(
                div()
                    .flex()
                    .gap(px(18.))
                    .child(div().flex().flex_col().items_end().children(items.iter().map(|(label, _)| cell().text_color(Theme::text_muted()).child(*label))))
                    .child(div().flex().flex_col().items_start().children(items.iter().map(|(_, a)| {
                        cell().child(
                            div()
                                .px(px(6.))
                                .h(ui_px(20.))
                                .flex()
                                .items_center()
                                .rounded(px(4.))
                                .bg(Theme::hover_row())
                                .text_color(Theme::accent())
                                .text_size(ui_px(12.))
                                .child(km.shortcut_label(*a).unwrap_or_default()),
                        )
                    }))),
            )
    }

    fn render_status(&self, cx: &mut Context<Self>) -> gpui::AnyElement {
        if crate::theme::islands() {
            return self.render_status_islands(cx);
        }
        let enc = self.active_editor().map(|e| e.read(cx).encoding_label()).unwrap_or_default();
        let indent = self.active_editor().map(|e| e.read(cx).indent_label()).unwrap_or_default();
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
                // PyCharm: "3 carets" with several, and the column selection mode.
                let n = e.buffer.selections.len();
                let carets = if n > 1 { format!("{n} carets  ") } else { String::new() };
                let pos = format!("{carets}{}:{}{}", l + 1, c + 1, if e.column_mode() { "  Column" } else { "" });
                (pos, eol, e.language_name(), crumbs)
            }
            None => (String::new(), "", "", self.project_name()),
        };
        let item = |s: String| div().px(px(8.)).child(s);
        div()
            .h(ui_px(26.))
            .flex()
            .items_center()
            .px(px(8.))
            .bg(if crate::theme::islands() { Theme::frame() } else { Theme::panel() })
            .when(!crate::theme::islands(), |d| d.border_t_1().border_color(Theme::border()))
            .text_size(ui_px(12.))
            .text_color(Theme::text_muted())
            .when(!crate::theme::islands(), |d| d.child(div().child(crumbs)))
            .child(div().flex_1())
            .child(item(self.active_editor().and_then(|e| e.read(cx).caret_problem()).unwrap_or_else(|| self.status.clone())))
            .when(!pos.is_empty(), |d| {
                d.child(item(pos))
                    .child(item(eol.into()))
                    .child(item(enc))
                    .child(item(indent))
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
            .into_any_element()
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
            PopupKind::AddInterpreter => ("Add Interpreter", "A new virtual environment in the project from this Python"),
            PopupKind::RunAnything => ("Run Anything", "python x.py, pytest …, pip install …, a configuration or any command"),
            PopupKind::Intentions => (self.problems.intents_title.unwrap_or("Context Actions"), "Type to filter"),
            PopupKind::Rename => ("Rename", "New name"),
            PopupKind::Projects => ("Projects", "Type to filter"),
            PopupKind::OpenWhere => ("Open Project", "Type to filter"),
            PopupKind::FileStructure => ("File Structure", "Type to filter"),
            PopupKind::BackgroundTasks => ("Background Tasks", "Type to filter"),
            PopupKind::TabList => ("Open Tabs", "Type to filter · hidden tabs are marked"),
            PopupKind::FolderCrumb => (self.crumb_dir.1.as_str(), "Type to filter · → opens a folder · ← goes up"),
            PopupKind::Bookmarks => ("Bookmarks", "Type to filter"),
            PopupKind::BookmarkMnemonic => ("Add Bookmark with Mnemonic", "Type a digit or letter, then Enter"),
            PopupKind::PasteHistory => ("Paste from History", "Type to filter"),
            PopupKind::LocalHistory => ("Local History", "Enter: show diff · Ctrl+Enter: revert or restore"),
            PopupKind::PutLabel => ("Put Label", "Label name"),
            PopupKind::BreakpointCondition => ("Breakpoint Condition", "Python expression, e.g. i == 3"),
            PopupKind::FileName | PopupKind::FileDelete => match &self.file_op {
                Some(op) => (op.title(), op.placeholder()),
                None => ("", ""),
            },
        };
        let tabs = match p.kind {
            PopupKind::SearchEverywhere(t) => Some(t),
            _ => None,
        };
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
                // Without this the row stretches the popup to its max height.
                .items_start()
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
                        .max_h(ui_px(520.))
                        .flex()
                        .flex_col()
                        .bg(Theme::panel())
                        .border_1()
                        .border_color(Theme::divider())
                        .rounded(px(10.))
                        .map(crate::ui_kit::floating)
                        .overflow_hidden()
                        .text_size(ui_px(13.))
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
                                    .child(div().text_color(Theme::text_dim()).text_size(ui_px(11.)).child("Tab to switch")),
                            )
                        })
                        .when(!title.is_empty(), |d| d.child(div().px(px(14.)).pt(px(10.)).font_weight(gpui::FontWeight::SEMIBOLD).child(title.to_string())))
                        .child(
                            div()
                                .m(px(10.))
                                .px(px(10.))
                                .h(ui_px(32.))
                                .flex()
                                .items_center()
                                .gap(px(8.))
                                .rounded(px(6.))
                                .border_1()
                                .border_color(Theme::accent())
                                .bg(Theme::bg())
                                .child(crate::icons::ui_icon("search", 14., Theme::text_muted()))
                                .child(if p.query.is_empty() {
                                    div().text_color(Theme::text_dim()).child(placeholder).into_any_element()
                                } else {
                                    div().flex().child(p.query.clone()).child(div().w(px(1.5)).h(ui_px(16.)).bg(Theme::caret())).into_any_element()
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
                                .child(
                                    uniform_list(
                                        "popup-items",
                                        p.items.len(),
                                        cx.processor(|this, range: std::ops::Range<usize>, _window, cx| {
                                            let Some(p) = this.popup.as_ref() else { return Vec::new() };
                                            let selected = p.selected;
                                            let items: Vec<(usize, PopupItem)> = range.filter_map(|i| p.items.get(i).map(|it| (i, it.clone()))).collect();
                                            items.into_iter().map(|(i, it)| Self::popup_row(i, &it, selected, cx)).collect()
                                        }),
                                    )
                                    .track_scroll(self.popup_scroll.clone())
                                    // Rows are drawn only where visible: hundreds of branches or
                                    // packages no longer rebuild every row on each frame.
                                    .h(ui_px(POPUP_ROW_H * p.items.len().min(POPUP_ROWS_SHOWN) as f32)),
                                ),
                        ),
                ),
        )
    }

    fn popup_row(i: usize, it: &PopupItem, selected: usize, cx: &mut Context<Self>) -> gpui::Stateful<gpui::Div> {
        let (icon, color) = match &it.target {
            Target::File(_) => file_icon(&it.label, false),
            Target::Dir(_) => file_icon(&it.label, true),
            Target::Action(_) => ("⚡", gpui::rgb(0xb3ae60).into()),
            Target::Line(_) => ("↧", Theme::text_muted()),
            Target::Branch(b) if b.current => ("★", gpui::rgb(0xe0a85a).into()),
            Target::Branch(b) if b.remote => ("☁", Theme::text_muted()),
            Target::Branch(_) => ("⎇", Theme::text_muted()),
            Target::NewBranch => ("+", Theme::text_muted()),
            Target::Confirm(_) => ("✓", Theme::file_added()),
            Target::Cancel => ("×", Theme::text_muted()),
            Target::Task => ("◷", Theme::accent()),
            Target::Tab(_) => file_icon(&it.label, false),
            Target::RunConfig(None, _) => ("▤", Theme::text_muted()),
            Target::RunConfig(Some(_), _) | Target::StopAndRerun(_) => ("▶", gpui::rgb(0x5fb865).into()),
            Target::EditRunConfigs => ("✎", Theme::text_muted()),
            Target::SaveRunConfig(_) => ("⎙", Theme::text_muted()),
            Target::Interpreter(_) => ("◆", gpui::rgb(0x4b8bbe).into()),
            Target::AddInterpreter | Target::CreateVenv(_) => ("+", gpui::rgb(0x4b8bbe).into()),
            Target::RunAnything(_) => ("▶", gpui::rgb(0x5fb865).into()),
            Target::Intention(_) => ("✦", gpui::rgb(0xe0b84a).into()),
            Target::Rename(_) => ("✎", gpui::rgb(0x4b8bbe).into()),
            Target::Project(project_ui::ProjectCmd::New) => ("+", Theme::text_muted()),
            Target::Project(project_ui::ProjectCmd::Open) => ("▰", gpui::rgb(0x8c8f96).into()),
            Target::Project(_) => ("▣", gpui::rgb(0x4b8bbe).into()),
            Target::Symbol(_, _, _, brisk_syntax::OutlineKind::Class) => ("C", gpui::rgb(0x4b8bbe).into()),
            Target::Symbol(_, _, _, brisk_syntax::OutlineKind::Method) => ("m", gpui::rgb(0xe0a85a).into()),
            Target::Symbol(_, _, _, brisk_syntax::OutlineKind::Variable) => ("v", gpui::rgb(0x9876aa).into()),
            Target::Symbol(..) => ("ƒ", gpui::rgb(0xb07fd5).into()),
            Target::FileDo(_) => ("✓", Theme::file_added()),
            Target::Bookmark(..) | Target::Mnemonic(_) => ("▮", Theme::bookmark()),
            Target::Clip(_) => ("¶", Theme::text_muted()),
            Target::History(..) => ("◷", Theme::text_muted()),
            Target::PutLabel(_) => ("▮", Theme::bookmark()),
            Target::BpCondition(_) | Target::BpRemove => ("●", Theme::breakpoint()),
        };
        let branch = matches!(it.target, Target::Branch(_));
        // A branch row has no room to spare: "tracks …" is shown only
        // when it fits whole next to the name (otherwise in the tooltip).
        let room = 680. - 12. - 16. - 16. - 84. - 3. * 8.;
        let show_detail = !branch || text_width(cx, &it.label, 13.) + text_width(cx, &it.detail, 12.) <= room;
        let label = if branch { fit_text(cx, &it.label, 13., room) } else { it.label.to_string() };
        let tip: Option<SharedString> = branch.then(|| if it.detail.is_empty() { it.label.to_string() } else { format!("{}
{}", it.label, it.detail) }.into());
        // uniform_list sizes an item to its content: the row spans the list's width through a
        // full-width wrapper (its padding is the row's side margin).
        let row = div()
            .id(("item", i))
            .w_full()
            .h(ui_px(28.))
            .flex_shrink_0()
            .px(px(8.))
            .flex()
            .items_center()
            .gap(px(8.))
            .whitespace_nowrap()
            .overflow_hidden()
            .rounded(px(5.))
            .when_some(tip, |d, t| d.tooltip(move |_, cx| cx.new(|_| Tip(t.clone())).into()))
            .when(i == selected, |d| d.bg(Theme::selection_row()))
            .when(!(i == selected), |d| d.hover(|s| s.bg(Theme::hover_row())))
            .cursor_pointer()
            .on_mouse_move(cx.listener(move |this, _, _, cx| {
                if let Some(p) = this.popup.as_mut() {
                    if p.selected != i {
                        p.selected = i;
                        cx.notify();
                    }
                }
            }))
            .on_click(cx.listener(move |this, _, window, cx| {
                if let Some(p) = this.popup.as_mut() {
                    p.selected = i;
                }
                let ev = KeyDownEvent { keystroke: gpui::Keystroke::parse("enter").unwrap(), is_held: false };
                this.popup_key(&ev, window, cx);
            }))
            // Fixed slots: the icon and the hint keep their size, the label
            // shrinks with an ellipsis, and the detail only takes what is
            // left (it goes first when the label is long).
            .child(div().w(px(16.)).flex_shrink_0().flex().items_center().justify_center().child(crate::icons::glyph(icon, 14., color)))
            .child(div().min_w_0().flex_shrink().overflow_hidden().text_ellipsis().child(label))
            .child(div().flex_1().min_w_0().overflow_hidden().text_ellipsis().text_color(Theme::text_dim()).text_size(ui_px(12.)).when(show_detail, |d| d.child(it.detail.clone())))
            .child(
                div()
                    .flex_shrink_0()
                    .flex()
                    .justify_end()
                    .text_color(Theme::text_muted())
                    .text_size(ui_px(12.))
                    .when(branch, |d| d.w(px(84.)).font_family(crate::theme::mono_font()))
                    .child(it.hint.clone()),
            );
        div().id(("item-wrap", i)).w_full().px(px(6.)).child(row)
    }
}

/// The width of `text` in the UI font at `size` px (for layouts that must not clip).
fn text_width(cx: &App, text: &str, size: f32) -> f32 {
    let ts = cx.text_system();
    let id = ts.resolve_font(&gpui::font(ui_font()));
    text.chars().map(|c| ts.advance(id, px(size), c).map(|a| f32::from(a.width)).unwrap_or(size * 0.55)).sum()
}

/// `text` cut with "…" to fit `room` px (GPUI clips a shrunk flex item without an ellipsis).
fn fit_text(cx: &App, text: &str, size: f32, room: f32) -> String {
    if text_width(cx, text, size) <= room {
        return text.to_string();
    }
    let budget = room - text_width(cx, "…", size);
    let mut w = 0.;
    let mut out = String::new();
    for c in text.chars() {
        w += text_width(cx, c.encode_utf8(&mut [0; 4]), size);
        if w > budget {
            break;
        }
        out.push(c);
    }
    out.push('…');
    out
}

const POPUP_ROW_H: f32 = 28.;
/// Rows the popup shows before it scrolls.
const POPUP_ROWS_SHOWN: usize = 14;

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

pub(crate) struct Tip(pub SharedString);

impl Render for Tip {
    fn render(&mut self, _: &mut Window, _: &mut Context<Self>) -> impl IntoElement {
        div()
            .px(px(8.))
            .py(px(4.))
            .rounded(px(5.))
            .bg(Theme::panel_alt())
            .border_1()
            .border_color(Theme::divider())
            .text_size(ui_px(12.))
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
        crate::stall::note("Workspace render, layout or paint");
        // Text without a size of its own follows the UI scale too.
        window.set_rem_size(gpui::px(16. * crate::theme::ui_scale()));
        crate::stall::frame_begin();
        let viewport = window.viewport_size();
        let left: Option<gpui::AnyElement> = match self.left {
            Some(LeftTool::Project) => Some(self.render_project(cx).into_any_element()),
            Some(LeftTool::Structure) => Some(self.render_structure(cx).into_any_element()),
            Some(LeftTool::Commit) => Some(self.render_commit(cx)),
            Some(LeftTool::Bookmarks) => Some(self.render_bookmarks(cx)),
            None => None,
        };
        let bottom_h = match self.bottom {
            Some(BottomTool::Git) => 320.,
            Some(BottomTool::Services) => 280.,
            Some(BottomTool::Debug) | Some(BottomTool::PythonConsole) => 300.,
            _ => 240.,
        };
        let right = self.db.open.then(|| self.render_database(cx));
        let bottom = self.bottom.map(|b| match (b, &self.terminal) {
            (BottomTool::Terminal, Some(t)) => t.clone().into_any_element(),
            (BottomTool::Run, _) if self.runs.panel.is_some() => self.runs.panel.clone().unwrap().into_any_element(),
            (BottomTool::Git, _) => self.render_git_log(cx),
            (BottomTool::Find, _) => self.render_find_window(cx),
            (BottomTool::Problems, _) => self.render_problems(cx),
            (BottomTool::Debug, _) => self.render_debug(window, cx),
            (BottomTool::Services, _) => self.render_services(cx),
            (BottomTool::PythonConsole, _) => self.render_python_console(window, cx),
            (BottomTool::Packages, _) => self.render_python_packages(window, cx),
            (BottomTool::Todo, _) => self.render_todo(cx),
            (BottomTool::Coverage, _) => self.render_coverage(cx),
            (BottomTool::Profiler, _) => self.render_profiler(cx),
            _ => Self::render_placeholder(b.title(), b.stage()),
        });
        let center = self.render_editor_area(cx);
        let islands = crate::theme::islands();

        div()
            .id("workspace")
            .key_context("Workspace")
            .track_focus(&self.focus)
            .capture_key_down(cx.listener(Self::on_key_down))
            .on_modifiers_changed(cx.listener(Self::on_modifiers))
            .size_full()
            .flex()
            .flex_col()
            .bg(if islands { Theme::frame() } else { Theme::bg() })
            .text_color(Theme::text())
            .font_family(ui_font())
            .child(self.render_toolbar(cx))
            .child(
                div()
                    .flex_1()
                    .flex()
                    .min_h_0()
                    .when(islands, |d| d.gap(px(crate::ui_kit::ISLAND_GAP)).pb(px(crate::ui_kit::ISLAND_GAP)))
                    .child(self.render_left_stripe(cx))
                    .when_some(left, |d, panel| {
                        let p = div().w(px(300.)).h_full().flex_shrink_0().flex().bg(Theme::panel())
                            .when(!islands, |d| d.border_r_1().border_color(Theme::border()))
                            .child(div().flex_1().min_w_0().h_full().child(panel_in("panel-left-in", panel)))
                            .child(self.hide_column(ToolSide::Left, cx));
                        d.child(if islands { crate::ui_kit::island(p) } else { p })
                    })
                    .child(
                        div()
                            .flex_1()
                            .min_w_0()
                            .flex()
                            .flex_col()
                            .when(islands, |d| d.gap(px(crate::ui_kit::ISLAND_GAP)))
                            .child(center)
                            .when_some(bottom, |d, panel| {
                                let p = div().h(px(bottom_h)).flex_shrink_0().flex().bg(Theme::panel())
                                    .when(!islands, |d| d.border_t_1().border_color(Theme::divider()))
                                    .child(div().flex_1().min_w_0().h_full().child(panel_in("panel-bottom-in", panel)))
                                    .child(self.hide_column(ToolSide::Bottom, cx));
                                d.child(if islands { crate::ui_kit::island(p) } else { p })
                            }),
                    )
                    .when_some(right, |d, panel| {
                        let p = div().w(px(320.)).h_full().flex_shrink_0().flex().bg(Theme::panel())
                            .when(!islands, |d| d.border_l_1().border_color(Theme::border()))
                            .child(div().flex_1().min_w_0().h_full().child(panel_in("panel-right-in", panel)))
                            .child(self.hide_column(ToolSide::Right, cx));
                        d.child(if islands { crate::ui_kit::island(p) } else { p })
                    })
                    .child(self.render_right_stripe(cx)),
            )
            .child(self.render_status(cx))
            .children(self.render_toast().map(|e| fade_in("toast-in", e, 160)))
            .children(self.render_venv_offer(cx).map(|e| fade_in("venv-offer-in", e, 160)))
            .children(self.render_cov_offer(cx).map(|e| fade_in("cov-offer-in", e, 160)))
            .children(self.render_popup(cx).map(|e| fade_in("popup-in", e, 110)))
            .children(self.render_find_popup(cx).map(|e| fade_in("find-in", e, 110)))
            .children(self.render_ds_dialog(cx).map(|e| fade_in("ds-in", e, 110)))
            .children(self.render_settings(cx).map(|e| fade_in("settings-in", e, 110)))
            .children(self.render_bp_dialog(cx).map(|e| fade_in("bp-dialog-in", e, 110)))
            .children(self.render_chooser(cx))
            .children(self.render_run_dialog(cx).map(|e| fade_in("run-dialog-in", e, 110)))
            .children(self.render_git_dialog(cx).map(|e| fade_in("git-dialog-in", e, 110)))
            .children(self.render_menu(viewport, cx).map(|e| fade_in("menu-in", e, 90)))
            .children(crate::stall::frame_probe())
    }
}

/// Which edge a tool window sits on.
#[derive(Clone, Copy, PartialEq, Eq)]
enum ToolSide {
    Left,
    Bottom,
    Right,
}

impl ToolSide {
    fn of(toggle: Action) -> ToolSide {
        match toggle {
            Action::ToggleProjectWindow | Action::ToggleCommitWindow | Action::ToggleStructureWindow | Action::ToggleBookmarksWindow => ToolSide::Left,
            Action::ToggleDatabaseWindow => ToolSide::Right,
            _ => ToolSide::Bottom,
        }
    }
}

/// Overlays (popups, menus, dialogs, toasts) fade in over `ms` instead of popping up, and
/// settle the last few pixels upward: short enough never to hold up the next keystroke.
fn fade_in(id: &'static str, e: impl IntoElement, ms: u64) -> impl IntoElement {
    use gpui::{Animation, AnimationExt};
    // Islands: popups, menus and dialogs take 140 ms (toasts keep 160), still ease-out.
    let ms = if crate::theme::islands() { ms.clamp(140, 160) } else { ms };
    // The surface's shadow fades in with it: GPUI scales shadow alpha by the element opacity
    // (from 0 to the `ui_kit::floating` target over the same 140 ms), so no shadow animation of
    // its own is needed.
    // Toasts and balloons rise a little more than popups and menus.
    let rise = if ms >= 160 { 8. } else if ms >= 110 { 4. } else { 2. };
    div()
        .absolute()
        .left_0()
        .size_full()
        .child(e)
        .with_animation(id, Animation::new(std::time::Duration::from_millis(ms)).with_easing(gpui::ease_out_quint()), move |d, t| {
            d.opacity(t).top(px((1. - t) * rise))
        })
}

/// A tool window fades in when it opens (no slide: the editor next to it must not move twice).
fn panel_in(id: &'static str, e: impl IntoElement) -> impl IntoElement {
    use gpui::{Animation, AnimationExt};
    div().size_full().child(e).with_animation(id, Animation::new(std::time::Duration::from_millis(120)).with_easing(gpui::ease_out_quint()), |d, t| {
        d.opacity(0.35 + 0.65 * t)
    })
}

impl Workspace {
    /// "Ctrl, Ctrl+Up/Down": the second Ctrl press within 400 ms of a lone Ctrl tap,
    /// held, turns Up/Down into Clone Caret.
    /// Track lone Ctrl taps; true on the release of a second quick tap with no key between.
    fn track_ctrl(&mut self, m: gpui::Modifiers) -> bool {
        let only_ctrl = m.control && !m.shift && !m.alt && !m.platform;
        if only_ctrl {
            if !self.ctrl_alone {
                self.ctrl_clone = self.ctrl_tap.is_some_and(|t| t.elapsed() < std::time::Duration::from_millis(400));
                self.ctrl_alone = true;
            }
        } else if !m.modified() && self.ctrl_alone {
            // Released without another key: a tap (unless it was the clone hold).
            let double = self.ctrl_clone;
            self.ctrl_tap = (!self.ctrl_clone).then(Instant::now);
            self.ctrl_alone = false;
            self.ctrl_clone = false;
            return double;
        } else {
            self.ctrl_alone = false;
            self.ctrl_tap = None;
            if !m.control {
                self.ctrl_clone = false;
            }
        }
        false
    }
}

fn clip_items(q: &str) -> Vec<PopupItem> {
    let ql = q.trim().to_lowercase();
    crate::editor::clipboard_history()
        .into_iter()
        .enumerate()
        .filter(|(_, t)| ql.is_empty() || t.to_lowercase().contains(&ql))
        .map(|(i, t)| {
            let first = t.lines().find(|l| !l.trim().is_empty()).unwrap_or("").trim();
            let n = t.trim_end_matches('\n').lines().count();
            PopupItem {
                label: first.chars().take(90).collect(),
                detail: if n > 1 { format!("{n} lines") } else { String::new() },
                hint: String::new(),
                target: Target::Clip(i),
            }
        })
        .collect()
}

