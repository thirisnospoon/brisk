//! TODO tool window: TODO / FIXME comments of the project grouped by file; a click (or
//! Enter) opens the line. Rescanned when the window opens, on Refresh, and for each file
//! saved while the window exists.

use super::{BottomTool, Workspace};
use crate::theme::ui_px;
use crate::theme::{mono_font, Theme};
#[cfg(feature = "smoke")]
use brisk_keymap::Action;
use brisk_search::todo::{self, Item};
use gpui::prelude::*;
use gpui::{div, px, uniform_list, AnyElement, Context, Entity, EventEmitter, FocusHandle, KeyDownEvent, ScrollStrategy, UniformListScrollHandle, Window};
use std::collections::HashSet;
use std::path::{Path, PathBuf};
use std::sync::Arc;

/// The TODO tool window as `Workspace` holds it: its focus and its model, which is created,
/// and the project scanned, the first time the window opens.
pub struct Todo {
    pub focus: FocusHandle,
    model: Option<Entity<TodoModel>>,
}

impl Todo {
    pub fn new(cx: &mut Context<Workspace>) -> Self {
        Self { focus: cx.focus_handle(), model: None }
    }

    /// Whether the scan finished and found something (the scroll budget waits for it).
    #[cfg(feature = "smoke")]
    pub fn scanned(&self, cx: &gpui::App) -> bool {
        self.model.as_ref().is_some_and(|m| {
            let m = m.read(cx);
            !m.scanning && !m.groups.is_empty()
        })
    }
}

/// What the TODO model asks of the workspace.
pub enum TodoEvent {
    /// Open the item's file and select its keyword.
    Open(Item),
    /// Esc: back to the editor.
    FocusEditor,
}

/// What a key in the TODO window did.
pub enum TodoKey {
    /// Not a key of the window.
    NotMine,
    Done,
    /// Done, and the workspace has to do this now (in the same turn, so the caller sees it).
    Then(TodoEvent),
}

/// The TODO / FIXME comments of the project, grouped by file, and the window that lists them.
pub struct TodoModel {
    root: PathBuf,
    focus: FocusHandle,
    groups: Vec<(PathBuf, Vec<Item>)>,
    collapsed: HashSet<PathBuf>,
    /// Row index in `rows()`.
    selected: usize,
    scanning: bool,
    generation: u64,
    scroll: UniformListScrollHandle,
    /// `rows()`, built once per change of `groups` or `collapsed` (see `touch`).
    rows_cache: std::cell::RefCell<Option<std::rc::Rc<Vec<Row>>>>,
}

#[derive(Clone, Debug, PartialEq)]
enum Row {
    File(PathBuf, usize),
    Item(Item),
}

impl EventEmitter<TodoEvent> for TodoModel {}

impl TodoModel {
    /// An empty list for the project at `root`; `focus` is the tool window's.
    pub fn new(root: PathBuf, focus: FocusHandle) -> Self {
        Self {
            root,
            focus,
            groups: Vec::new(),
            collapsed: HashSet::new(),
            selected: 0,
            scanning: false,
            generation: 0,
            scroll: UniformListScrollHandle::new(),
            rows_cache: Default::default(),
        }
    }

    /// The visible rows; cached, so a frame or a key costs no copy of the items.
    fn rows(&self) -> std::rc::Rc<Vec<Row>> {
        if let Some(r) = &*self.rows_cache.borrow() {
            return r.clone();
        }
        let mut out = Vec::new();
        for (p, items) in &self.groups {
            out.push(Row::File(p.clone(), items.len()));
            if !self.collapsed.contains(p) {
                out.extend(items.iter().cloned().map(Row::Item));
            }
        }
        let r = std::rc::Rc::new(out);
        *self.rows_cache.borrow_mut() = Some(r.clone());
        r
    }

    /// `groups` or `collapsed` changed: rebuild the rows on next use.
    fn touch(&mut self) {
        self.rows_cache.get_mut().take();
    }

    fn count(&self) -> usize {
        self.groups.iter().map(|g| g.1.len()).sum()
    }

    /// Scans the whole project again, in the background.
    pub fn refresh(&mut self, cx: &mut Context<Self>) {
        let root = self.root.clone();
        self.scanning = true;
        self.generation += 1;
        let gen = self.generation;
        let task = cx.background_executor().spawn(async move { todo::scan_project(&root) });
        cx.spawn(async move |this, cx| {
            let groups = task.await;
            let _ = this.update(cx, |t, cx| {
                if t.generation != gen {
                    return;
                }
                t.groups = groups;
                t.touch();
                t.scanning = false;
                t.selected = t.selected.min(t.rows().len().saturating_sub(1));
                cx.notify();
            });
        })
        .detach();
    }

    /// Saved files: rescan just them (the rest of the list stays). `files` is the project's
    /// file list, if it has been read.
    pub fn files_saved(&mut self, paths: Vec<PathBuf>, files: Option<Arc<brisk_project::FileList>>, cx: &mut Context<Self>) {
        if paths.is_empty() {
            return;
        }
        let root = self.root.clone();
        let task = cx.background_executor().spawn(async move {
            // Ignored files stay out, as in the full scan: only files in the project's list
            // (sorted) count. A file not in it yet (just created) takes a fresh list.
            let listed =
                |list: &brisk_project::FileList, p: &PathBuf| brisk_project::rel_path(&list.root, p).is_some_and(|rel| list.files.binary_search(&rel).is_ok());
            let list = match files {
                Some(l) if paths.iter().all(|p| listed(&l, p)) => l,
                _ => Arc::new(brisk_project::FileList::scan(&root)),
            };
            paths.into_iter().filter(|p| listed(&list, p)).map(|p| (p.clone(), todo::scan_file(&p))).collect::<Vec<_>>()
        });
        cx.spawn(async move |this, cx| {
            let fresh = task.await;
            let _ = this.update(cx, |t, cx| {
                for (p, items) in fresh {
                    t.groups.retain(|g| g.0 != p);
                    if !items.is_empty() {
                        let at = t.groups.partition_point(|g| g.0 < p);
                        t.groups.insert(at, (p, items));
                    }
                }
                t.touch();
                t.selected = t.selected.min(t.rows().len().saturating_sub(1));
                cx.notify();
            });
        })
        .detach();
    }

    /// Selects `row` and folds / unfolds it if it is a file; an item is returned for the
    /// workspace to open.
    fn activate(&mut self, row: usize, cx: &mut Context<Self>) -> Option<TodoEvent> {
        let r = self.rows().get(row).cloned()?;
        self.selected = row;
        cx.notify();
        match r {
            Row::File(p, _) => {
                if !self.collapsed.remove(&p) {
                    self.collapsed.insert(p);
                }
                self.touch();
                None
            }
            Row::Item(i) => Some(TodoEvent::Open(i)),
        }
    }

    /// A key pressed with the window focused and no modifier held.
    pub fn key(&mut self, key: &str, cx: &mut Context<Self>) -> TodoKey {
        let n = self.rows().len();
        let sel = self.selected;
        match key {
            "up" | "down" if n > 0 => {
                self.selected = if key == "up" { sel.saturating_sub(1) } else { (sel + 1).min(n - 1) };
                self.scroll.scroll_to_item(self.selected, ScrollStrategy::Top);
            }
            "enter" => {
                if let Some(ev) = self.activate(sel, cx) {
                    return TodoKey::Then(ev);
                }
            }
            "left" | "right" => {
                if let Some(Row::File(p, _)) = self.rows().get(sel) {
                    if key == "left" {
                        self.collapsed.insert(p.clone());
                    } else {
                        self.collapsed.remove(p);
                    }
                    self.touch();
                }
            }
            "escape" => return TodoKey::Then(TodoEvent::FocusEditor),
            _ => return TodoKey::NotMine,
        }
        cx.notify();
        TodoKey::Done
    }

    /// One row of the TODO list (only the rows in view are built).
    fn row(&self, i: usize, r: &Row, cx: &mut Context<Self>) -> AnyElement {
        let rel = |p: &Path| p.strip_prefix(&self.root).unwrap_or(p).to_string_lossy().replace('\\', "/");
        let sel = i == self.selected;
        let row = div()
            .id(("todo-row", i))
            .w_full()
            .h(ui_px(22.))
            .flex()
            .items_center()
            .gap(px(6.))
            .whitespace_nowrap()
            .overflow_hidden()
            .cursor_pointer()
            .when(sel, |d| d.bg(Theme::selection_row()))
            .when(!sel, |d| d.hover(|s| s.bg(Theme::hover_row())))
            .on_mouse_down(
                gpui::MouseButton::Left,
                cx.listener(move |this, _, w, cx| {
                    w.focus(&this.focus);
                    // A click opens an item, or folds / unfolds a file.
                    if let Some(ev) = this.activate(i, cx) {
                        cx.emit(ev);
                    }
                }),
            );
        let el = match r {
            Row::File(p, n) => {
                let open = !self.collapsed.contains(p);
                let name = p.file_name().map(|n| n.to_string_lossy().to_string()).unwrap_or_default();
                let dir = rel(p.parent().unwrap_or(p));
                row.pl(px(8.))
                    .child(crate::icons::tree_arrow(true, open, Theme::text_dim()))
                    .child(div().text_color(Theme::text()).child(name))
                    .child(div().text_size(ui_px(12.)).text_color(Theme::text_dim()).child(dir))
                    .child(div().text_size(ui_px(12.)).text_color(Theme::text_muted()).child(format!("{n} item{}", if *n == 1 { "" } else { "s" })))
            }
            Row::Item(it) => {
                let fixme = it.text.to_lowercase().starts_with("fixme");
                row.pl(px(34.))
                    .child(div().text_size(ui_px(12.)).text_color(Theme::text_dim()).font_family(mono_font()).child(format!(
                        "({}, {})",
                        it.line + 1,
                        it.col + 1
                    )))
                    .child(div().text_color(if fixme { Theme::warning() } else { Theme::syntax(brisk_syntax::Highlight::Comment) }).child(it.text.clone()))
            }
        };
        el.into_any_element()
    }
}

impl Render for TodoModel {
    fn render(&mut self, _window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let rows = self.rows();
        let summary = if self.scanning && self.groups.is_empty() {
            "Scanning…".to_string()
        } else {
            format!("Found {} TODO items in {} files", self.count(), self.groups.len())
        };
        let toolbar = div()
            .flex()
            .flex_shrink_0()
            .items_center()
            .gap(px(6.))
            .pl(px(12.))
            .pr(px(6.))
            .h(px(crate::ui_kit::header_h()))
            .border_b_1()
            .border_color(Theme::divider())
            .child(crate::ui_kit::tool_title("TODO"))
            .child(
                div()
                    .id("todo-refresh")
                    .size(ui_px(24.))
                    .flex()
                    .items_center()
                    .justify_center()
                    .rounded(px(5.))
                    .cursor_pointer()
                    .text_color(Theme::accent())
                    .hover(|s| s.bg(Theme::hover_row()))
                    .active(|s| s.bg(Theme::press()))
                    .child("↻")
                    .tooltip(|_, cx| cx.new(|_| super::Tip("Rescan the project".into())).into())
                    .on_click(cx.listener(|this, _, _, cx| this.refresh(cx))),
            )
            .child(div().text_size(ui_px(12.5)).text_color(Theme::text_muted()).child(summary));
        let empty = rows.is_empty() && !self.scanning;
        let list = div()
            .id("todo-list")
            .track_focus(&self.focus)
            .flex_1()
            .min_h_0()
            .flex()
            .flex_col()
            .text_size(ui_px(13.))
            .when(empty, |d| d.child(div().p(px(10.)).text_color(Theme::text_dim()).child("No TODO or FIXME comments in the project")))
            .when(!empty, |d| {
                d.child(
                    uniform_list(
                        "todo-rows",
                        rows.len(),
                        cx.processor(|this, range: std::ops::Range<usize>, _window, cx| {
                            let rows = this.rows();
                            range.filter_map(|i| rows.get(i).map(|r| this.row(i, r, cx))).collect()
                        }),
                    )
                    .flex_1()
                    .track_scroll(self.scroll.clone()),
                )
            });
        div().id("todo-window").size_full().flex().flex_col().child(toolbar).child(list)
    }
}

impl Workspace {
    /// The TODO model, created on first use.
    fn todo_model(&mut self, window: &mut Window, cx: &mut Context<Self>) -> Entity<TodoModel> {
        if let Some(m) = &self.todo.model {
            return m.clone();
        }
        let m = cx.new(|_| TodoModel::new(self.root.clone(), self.todo.focus.clone()));
        cx.subscribe_in(&m, window, |this, _, ev, window, cx| this.on_todo_event(ev, window, cx)).detach();
        self.todo.model = Some(m.clone());
        m
    }

    fn on_todo_event(&mut self, ev: &TodoEvent, window: &mut Window, cx: &mut Context<Self>) {
        match ev {
            TodoEvent::Open(i) => {
                self.open_file(&i.path, window, cx);
                if let Some(ed) = self.active_editor().cloned() {
                    let len = i.text.split([':', ' ', '(']).next().map(|w| w.chars().count()).unwrap_or(0);
                    ed.update(cx, |e, cx| e.select_on_line(i.line, i.col..i.col + len, cx));
                }
            }
            TodoEvent::FocusEditor => self.focus_editor(window, cx),
        }
        cx.notify();
    }

    pub(super) fn toggle_todo_window(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let m = self.todo_model(window, cx);
        self.toggle_bottom(BottomTool::Todo, window, cx);
        if self.bottom == Some(BottomTool::Todo) {
            m.update(cx, |m, cx| m.refresh(cx));
            window.focus(&self.todo.focus);
        }
        cx.notify();
    }

    /// Saved files: rescan just them, if the window was ever opened.
    pub(super) fn todo_files_saved(&mut self, paths: Vec<PathBuf>, cx: &mut Context<Self>) {
        let Some(m) = self.todo.model.clone() else { return };
        let files = self.files.clone();
        m.update(cx, |m, cx| m.files_saved(paths, files, cx));
    }

    pub(super) fn todo_key(&mut self, e: &KeyDownEvent, window: &mut Window, cx: &mut Context<Self>) -> bool {
        if !self.todo.focus.is_focused(window) {
            return false;
        }
        let Some(m) = self.todo.model.clone() else { return false };
        let mods = &e.keystroke.modifiers;
        if mods.control || mods.alt || mods.platform {
            return false;
        }
        match m.update(cx, |m, cx| m.key(&e.keystroke.key, cx)) {
            TodoKey::NotMine => false,
            TodoKey::Done => true,
            TodoKey::Then(ev) => {
                self.on_todo_event(&ev, window, cx);
                true
            }
        }
    }

    pub(super) fn render_todo(&self) -> AnyElement {
        match &self.todo.model {
            Some(m) => m.clone().into_any_element(),
            None => div().into_any_element(),
        }
    }

    // --- Smoke --------------------------------------------------------------------------

    /// BRISK_SMOKE_TODO=1|hold on a scratch folder: writes files with TODO / FIXME (and an
    /// ignored one), opens the window, adds a TODO through the editor and saves, opens an item.
    #[cfg(feature = "smoke")]
    pub(super) fn todo_smoke(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let root = self.root.clone();
        let _ = std::fs::create_dir_all(root.join("pkg"));
        let _ = std::fs::create_dir_all(root.join("build"));
        let _ = std::fs::write(root.join(".gitignore"), "build/\n");
        let _ = std::fs::write(
            root.join("app.py"),
            "import os\n\n\ndef main():\n    # TODO: read the config from env\n    path = os.getcwd()  # FIXME handle a missing folder\n    return path\n",
        );
        let _ = std::fs::write(root.join("pkg/util.py"), "def todo_list():\n    return []  # todo_list is a name, not an item\n");
        let _ = std::fs::write(root.join("pkg/web.js"), "// TODO debounce the handler\nfunction f() {}\n/* FIXME: leaks */\n");
        let _ = std::fs::write(root.join("notes.md"), "# Notes\n\n- [ ] TODO write the README\n");
        let _ = std::fs::write(root.join("build/gen.py"), "# TODO generated, ignored\n");
        cx.spawn_in(window, async move |this, cx| {
            let bg = cx.background_executor().clone();
            let pause = |ms| bg.timer(std::time::Duration::from_millis(ms));
            pause(500).await;
            let _ = this.update_in(cx, |this, window, cx| this.dispatch(Action::ToggleTodoWindow, window, cx));
            pause(1200).await;
            let dump = |this: &Workspace, cx: &gpui::App| {
                let Some(m) = &this.todo.model else { return vec!["no TODO model".to_string()] };
                m.read(cx)
                    .groups
                    .iter()
                    .map(|(p, items)| {
                        format!(
                            "{}: {:?}",
                            p.strip_prefix(&this.root).unwrap_or(p).display(),
                            items.iter().map(|i| format!("{}:{} {}", i.line + 1, i.col + 1, i.text)).collect::<Vec<_>>()
                        )
                    })
                    .collect::<Vec<_>>()
            };
            let _ = this.update_in(cx, |this, window, cx| {
                eprintln!("smoke-todo: scan -> {:#?}", dump(this, cx));
                let util = this.root.join("pkg/util.py");
                this.open_file(&util, window, cx);
                if let Some(ed) = this.active_editor().cloned() {
                    ed.update(cx, |e, cx| e.go_to_line(1, cx));
                }
                this.smoke_press("end enter", window, cx);
                this.smoke_type("# TODO: cache the list", window, cx);
                this.dispatch(Action::SaveAll, window, cx);
            });
            pause(1200).await;
            let _ = this.update_in(cx, |this, window, cx| {
                eprintln!("smoke-todo: after save -> {:#?}", dump(this, cx));
                let Some(m) = this.todo.model.clone() else { return cx.quit() };
                // Select the FIXME of app.py and open it with Enter.
                m.update(cx, |t, _| {
                    t.selected = t.rows().iter().position(|r| matches!(r, Row::Item(i) if i.text.starts_with("FIXME handle"))).unwrap_or(0);
                });
                window.focus(&this.todo.focus);
                this.smoke_press("enter", window, cx);
                let at = this.active_editor().map(|e| (e.read(cx).title(), e.read(cx).caret_line_col()));
                eprintln!("smoke-todo: enter on FIXME -> {at:?}");
                // Fold app.py.
                m.update(cx, |t, _| t.selected = 0);
                window.focus(&this.todo.focus);
                this.smoke_press("left", window, cx);
                eprintln!("smoke-todo: rows after folding the first file -> {}", m.read(cx).rows().len());
                this.smoke_press("right", window, cx);
                if std::env::var("BRISK_SMOKE_TODO").as_deref() != Ok("hold") {
                    cx.quit();
                }
            });
        })
        .detach();
    }
}
