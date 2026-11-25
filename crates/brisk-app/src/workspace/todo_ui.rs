//! TODO tool window: TODO / FIXME comments of the project grouped by file; a click (or
//! Enter) opens the line. Rescanned when the window opens, on Refresh, and for each file
//! saved while the window exists.

use super::{BottomTool, Workspace};
use crate::theme::ui_px;
use crate::theme::{mono_font, Theme};
use crate::todo_scan::{self, Item};
#[cfg(feature = "smoke")]
use brisk_keymap::Action;
use gpui::prelude::*;
use gpui::{div, px, uniform_list, Context, FocusHandle, KeyDownEvent, ScrollStrategy, UniformListScrollHandle, Window};
use std::collections::HashSet;
use std::path::{Path, PathBuf};

pub struct Todo {
    pub focus: FocusHandle,
    pub groups: Vec<(PathBuf, Vec<Item>)>,
    pub collapsed: HashSet<PathBuf>,
    /// Row index in `rows()`.
    pub selected: usize,
    pub scanning: bool,
    generation: u64,
    scroll: UniformListScrollHandle,
    /// `rows()`, built once per change of `groups` or `collapsed` (see `touch`).
    rows_cache: std::cell::RefCell<Option<std::rc::Rc<Vec<Row>>>>,
}

#[derive(Clone, Debug, PartialEq)]
pub enum Row {
    File(PathBuf, usize),
    Item(Item),
}

impl Todo {
    /// The visible rows; cached, so a frame or a key costs no copy of the items.
    pub fn rows(&self) -> std::rc::Rc<Vec<Row>> {
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
    pub fn touch(&mut self) {
        self.rows_cache.get_mut().take();
    }

    pub fn count(&self) -> usize {
        self.groups.iter().map(|g| g.1.len()).sum()
    }
}

impl Workspace {
    pub(super) fn toggle_todo_window(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        if self.todo.is_none() {
            self.todo = Some(Todo {
                focus: cx.focus_handle(),
                groups: Vec::new(),
                collapsed: HashSet::new(),
                selected: 0,
                scanning: false,
                generation: 0,
                scroll: UniformListScrollHandle::new(),
                rows_cache: Default::default(),
            });
        }
        self.toggle_bottom(BottomTool::Todo, window, cx);
        if self.bottom == Some(BottomTool::Todo) {
            self.refresh_todo(cx);
            if let Some(t) = &self.todo {
                window.focus(&t.focus);
            }
        }
        cx.notify();
    }

    pub(super) fn refresh_todo(&mut self, cx: &mut Context<Self>) {
        let root = self.root.clone();
        let Some(t) = self.todo.as_mut() else { return };
        t.scanning = true;
        t.generation += 1;
        let gen = t.generation;
        let task = cx.background_executor().spawn(async move { todo_scan::scan_project(&root) });
        cx.spawn(async move |this, cx| {
            let groups = task.await;
            let _ = this.update(cx, |this, cx| {
                let Some(t) = this.todo.as_mut().filter(|t| t.generation == gen) else { return };
                t.groups = groups;
                t.touch();
                t.scanning = false;
                t.selected = t.selected.min(t.rows().len().saturating_sub(1));
                cx.notify();
            });
        })
        .detach();
    }

    /// Saved files: rescan just them (the rest of the list stays).
    pub(super) fn todo_files_saved(&mut self, paths: Vec<PathBuf>, cx: &mut Context<Self>) {
        if self.todo.is_none() || paths.is_empty() {
            return;
        }
        let root = self.root.clone();
        let files = self.files.clone();
        let task = cx.background_executor().spawn(async move {
            // Ignored files stay out, as in the full scan: only files in the project's list
            // (sorted) count. A file not in it yet (just created) takes a fresh list.
            let listed =
                |list: &brisk_project::FileList, p: &PathBuf| brisk_project::rel_path(&list.root, p).is_some_and(|rel| list.files.binary_search(&rel).is_ok());
            let list = match files {
                Some(l) if paths.iter().all(|p| listed(&l, p)) => l,
                _ => std::sync::Arc::new(brisk_project::FileList::scan(&root)),
            };
            paths.into_iter().filter(|p| listed(&list, p)).map(|p| (p.clone(), todo_scan::scan_file(&p))).collect::<Vec<_>>()
        });
        cx.spawn(async move |this, cx| {
            let fresh = task.await;
            let _ = this.update(cx, |this, cx| {
                let Some(t) = this.todo.as_mut() else { return };
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

    fn todo_open(&mut self, row: usize, window: &mut Window, cx: &mut Context<Self>) {
        let Some(t) = self.todo.as_mut() else { return };
        let Some(r) = t.rows().get(row).cloned() else { return };
        t.selected = row;
        match r {
            Row::File(p, _) => {
                if !t.collapsed.remove(&p) {
                    t.collapsed.insert(p);
                }
                t.touch();
            }
            Row::Item(i) => {
                self.open_file(&i.path, window, cx);
                if let Some(ed) = self.active_editor().cloned() {
                    let len = i.text.split([':', ' ', '(']).next().map(|w| w.chars().count()).unwrap_or(0);
                    ed.update(cx, |e, cx| e.select_on_line(i.line, i.col..i.col + len, cx));
                }
            }
        }
        cx.notify();
    }

    pub(super) fn todo_key(&mut self, e: &KeyDownEvent, window: &mut Window, cx: &mut Context<Self>) -> bool {
        let Some(t) = &self.todo else { return false };
        if !t.focus.is_focused(window) {
            return false;
        }
        let m = &e.keystroke.modifiers;
        if m.control || m.alt || m.platform {
            return false;
        }
        let n = t.rows().len();
        let sel = t.selected;
        match e.keystroke.key.as_str() {
            "up" | "down" if n > 0 => {
                let t = self.todo.as_mut().unwrap();
                t.selected = if e.keystroke.key == "up" { sel.saturating_sub(1) } else { (sel + 1).min(n - 1) };
                t.scroll.scroll_to_item(t.selected, ScrollStrategy::Top);
            }
            "enter" => self.todo_open(sel, window, cx),
            "left" | "right" => {
                let t = self.todo.as_mut().unwrap();
                if let Some(Row::File(p, _)) = t.rows().get(sel) {
                    if e.keystroke.key == "left" {
                        t.collapsed.insert(p.clone());
                    } else {
                        t.collapsed.remove(p);
                    }
                    t.touch();
                }
            }
            "escape" => self.focus_editor(window, cx),
            _ => return false,
        }
        cx.notify();
        true
    }

    pub(super) fn render_todo(&mut self, cx: &mut Context<Self>) -> gpui::AnyElement {
        let Some(t) = &self.todo else { return div().into_any_element() };
        let rows = t.rows();
        let summary =
            if t.scanning && t.groups.is_empty() { "Scanning…".to_string() } else { format!("Found {} TODO items in {} files", t.count(), t.groups.len()) };
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
                    .on_click(cx.listener(|this, _, _, cx| this.refresh_todo(cx))),
            )
            .child(div().text_size(ui_px(12.5)).text_color(Theme::text_muted()).child(summary));
        let empty = rows.is_empty() && !t.scanning;
        let list = div()
            .id("todo-list")
            .track_focus(&t.focus)
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
                            let Some(t) = &this.todo else { return Vec::new() };
                            let rows = t.rows();
                            range.filter_map(|i| rows.get(i).map(|r| this.todo_row(i, r, cx))).collect()
                        }),
                    )
                    .flex_1()
                    .track_scroll(t.scroll.clone()),
                )
            });
        div().id("todo-window").size_full().flex().flex_col().child(toolbar).child(list).into_any_element()
    }

    /// One row of the TODO list (only the rows in view are built).
    fn todo_row(&self, i: usize, r: &Row, cx: &mut Context<Self>) -> gpui::AnyElement {
        let Some(t) = &self.todo else { return div().into_any_element() };
        let rel = |p: &Path| p.strip_prefix(&self.root).unwrap_or(p).to_string_lossy().replace('\\', "/");
        {
            {
                let sel = i == t.selected;
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
                            if let Some(t) = this.todo.as_mut() {
                                t.selected = i;
                                w.focus(&t.focus.clone());
                            }
                            // A click opens an item, or folds / unfolds a file.
                            this.todo_open(i, w, cx);
                        }),
                    );
                let el = match r {
                    Row::File(p, n) => {
                        let open = !t.collapsed.contains(p);
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
                            .child(
                                div().text_color(if fixme { Theme::warning() } else { Theme::syntax(brisk_syntax::Highlight::Comment) }).child(it.text.clone()),
                            )
                    }
                };
                el.into_any_element()
            }
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
            let dump = |this: &Workspace| {
                let t = this.todo.as_ref().unwrap();
                t.groups
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
                eprintln!("smoke-todo: scan -> {:#?}", dump(this));
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
                eprintln!("smoke-todo: after save -> {:#?}", dump(this));
                // Select the FIXME of app.py and open it with Enter.
                let t = this.todo.as_mut().unwrap();
                let row = t.rows().iter().position(|r| matches!(r, Row::Item(i) if i.text.starts_with("FIXME handle"))).unwrap_or(0);
                t.selected = row;
                let f = t.focus.clone();
                window.focus(&f);
                this.smoke_press("enter", window, cx);
                let at = this.active_editor().map(|e| (e.read(cx).title(), e.read(cx).caret_line_col()));
                eprintln!("smoke-todo: enter on FIXME -> {at:?}");
                // Fold app.py.
                let t = this.todo.as_mut().unwrap();
                t.selected = 0;
                window.focus(&t.focus.clone());
                this.smoke_press("left", window, cx);
                eprintln!("smoke-todo: rows after folding the first file -> {}", this.todo.as_ref().unwrap().rows().len());
                this.smoke_press("right", window, cx);
                if std::env::var("BRISK_SMOKE_TODO").as_deref() != Ok("hold") {
                    cx.quit();
                }
            });
        })
        .detach();
    }
}
