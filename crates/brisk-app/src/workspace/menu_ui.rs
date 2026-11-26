//! Context menus (right click) and the main menu (☰): one mechanism for the editor, the project
//! tree and the title bar. A `Submenu` row opens a second menu next to it (hover, click, → or
//! Enter). Keyboard: ↑/↓, Enter, →/← into and out of submenus, Esc. A click outside closes the
//! menu without reaching what is under it.

use super::Workspace;
use crate::editor::Editor;
use crate::theme::ui_px;
use crate::theme::Theme;
use brisk_keymap::Action;
use gpui::prelude::*;
use gpui::{div, point, px, ClipboardItem, Context, Entity, KeyDownEvent, Pixels, Point, Size, Window};
use std::cell::Cell;
use std::rc::Rc;

pub(super) type MenuFn = Rc<dyn Fn(&mut Workspace, &mut Window, &mut Context<Workspace>)>;

#[derive(Clone)]
pub(super) enum Entry {
    Item { label: String, shortcut: String, enabled: bool, run: MenuFn },
    Separator,
    Submenu { label: String, entries: Vec<Entry> },
}

impl Entry {
    /// Labels of the items, a submenu's own items in brackets (smoke tests).
    #[cfg(feature = "smoke")]
    pub(super) fn labels(&self) -> Vec<String> {
        match self {
            Entry::Item { label, .. } => vec![label.clone()],
            Entry::Separator => Vec::new(),
            Entry::Submenu { label, entries } => std::iter::once(format!("{label}:")).chain(entries.iter().flat_map(Entry::labels)).collect(),
        }
    }

    fn selectable(&self) -> bool {
        matches!(self, Entry::Item { enabled: true, .. } | Entry::Submenu { .. })
    }

    /// The painted height (rows are `ui_px`), so placement and submenus line up at any IDE zoom.
    fn height(&self) -> f32 {
        let h = if matches!(self, Entry::Separator) { 9.0 } else { ITEM_H };
        h * crate::theme::ui_scale()
    }
}

pub(super) struct ContextMenu {
    pub pos: Point<Pixels>,
    pub entries: Vec<Entry>,
    pub selected: Option<usize>,
    /// The open submenu: the row it belongs to and the menu itself.
    child: Option<(usize, Box<ContextMenu>)>,
    /// A row the mouse moved to while a submenu was open: it takes over after a pause.
    pending: Option<usize>,
    /// Where the menu was last painted (after flipping at the window edges).
    placed: Cell<(f32, f32)>,
}

impl ContextMenu {
    fn new(pos: Point<Pixels>, entries: Vec<Entry>) -> Self {
        let placed = Cell::new((f32::from(pos.x), f32::from(pos.y)));
        ContextMenu { pos, entries, selected: None, child: None, pending: None, placed }
    }

    /// The mouse picked row `i`: select it and open its submenu, or close another one.
    fn select_row(&mut self, i: usize) {
        self.selected = Some(i);
        if matches!(self.entries.get(i), Some(Entry::Submenu { .. })) {
            self.open_child(i, false);
        } else {
            self.child = None;
        }
    }

    /// The menu `depth` levels down (0 = this one).
    fn level(&self, depth: usize) -> Option<&ContextMenu> {
        if depth == 0 {
            Some(self)
        } else {
            self.child.as_ref()?.1.level(depth - 1)
        }
    }

    fn level_mut(&mut self, depth: usize) -> Option<&mut ContextMenu> {
        if depth == 0 {
            Some(self)
        } else {
            self.child.as_mut()?.1.level_mut(depth - 1)
        }
    }

    /// The deepest open level.
    fn depth(&self) -> usize {
        self.child.as_ref().map_or(0, |(_, c)| 1 + c.depth())
    }

    fn height(&self) -> f32 {
        self.entries.iter().map(Entry::height).sum::<f32>() + 10.0
    }

    /// Opens the submenu of row `i` (or closes the open one when `i` is not a submenu).
    fn open_child(&mut self, i: usize, select_first: bool) {
        if self.child.as_ref().is_some_and(|(j, _)| *j == i) && !select_first {
            return;
        }
        self.child = None;
        let Some(Entry::Submenu { entries, .. }) = self.entries.get(i) else { return };
        let (x, y) = self.placed.get();
        let top: f32 = self.entries[..i].iter().map(Entry::height).sum();
        let mut sub = ContextMenu::new(point(px(x + MENU_W - 6.0), px(y + top)), entries.clone());
        if select_first {
            sub.selected = sub.entries.iter().position(Entry::selectable);
        }
        self.child = Some((i, Box::new(sub)));
    }
}

const ITEM_H: f32 = 26.0;
const MENU_W: f32 = 280.0;

impl Workspace {
    /// A menu entry that runs a keymap action (its shortcut shown on the right). `editor`:
    /// give the editor the focus first, so editor actions apply to it.
    pub(super) fn action_entry(&self, label: &str, a: Action, enabled: bool, editor: bool) -> Entry {
        Entry::Item {
            label: label.into(),
            shortcut: self.resolver.keymap().shortcut_label(a).unwrap_or_default(),
            enabled,
            run: Rc::new(move |this, window, cx| {
                if editor {
                    this.focus_editor(window, cx);
                }
                this.dispatch(a, window, cx);
            }),
        }
    }

    pub(super) fn open_menu(&mut self, pos: Point<Pixels>, entries: Vec<Entry>, cx: &mut Context<Self>) {
        self.popup = None;
        self.menu = Some(ContextMenu::new(pos, entries));
        cx.notify();
    }

    /// Right click in the editor.
    pub(super) fn open_editor_menu(&mut self, ed: Entity<Editor>, pos: Point<Pixels>, cx: &mut Context<Self>) {
        let (python, path) = {
            let e = ed.read(cx);
            (e.is_python(), e.path.clone())
        };
        let name = path.as_ref().and_then(|p| p.file_name()).map(|n| n.to_string_lossy().into_owned()).unwrap_or_default();
        let mut v = vec![
            self.action_entry("Show Context Actions", Action::ShowIntentionActions, python, true),
            Entry::Separator,
            self.action_entry("Go to Declaration or Usages", Action::GotoDeclaration, python, true),
            self.action_entry("Find Usages", Action::FindUsages, python, true),
            self.action_entry("Rename…", Action::Rename, python, true),
            Entry::Separator,
            self.action_entry("Cut", Action::Cut, true, true),
            self.action_entry("Copy", Action::Copy, true, true),
            self.action_entry("Paste", Action::Paste, true, true),
            Entry::Separator,
            self.action_entry("Find…", Action::Find, true, true),
            self.action_entry("Replace…", Action::Replace, true, true),
            self.action_entry("Comment with Line Comment", Action::CommentLine, true, true),
            Entry::Separator,
        ];
        if python {
            v.push(self.action_entry(&format!("Run '{name}'"), Action::RunContext, true, true));
            if self.debug.read(cx).paused_thread().is_some() {
                v.push(self.action_entry("Jump to Cursor (Set Next Statement)", Action::SetNextStatement, true, true));
            }
        }
        if let Some(p) = path {
            v.extend(self.git_menu_entries(&p, Some(&ed), cx));
            v.push(self.path_entries(p));
            v.push(Entry::Item {
                label: "Show in Explorer".into(),
                shortcut: String::new(),
                enabled: true,
                run: Rc::new({
                    let p = ed.read(cx).path.clone();
                    move |_, _, cx| {
                        if let Some(p) = &p {
                            cx.reveal_path(p);
                        }
                    }
                }),
            });
        }
        self.open_menu(pos, v, cx);
    }

    /// "Copy Path": the absolute path to the clipboard.
    pub(super) fn path_entries(&self, p: std::path::PathBuf) -> Entry {
        Entry::Item {
            label: "Copy Path".into(),
            shortcut: self.resolver.keymap().shortcut_label(Action::CopyPath).unwrap_or_default(),
            enabled: true,
            run: Rc::new(move |this, _, cx| this.copy_path(&p, cx)),
        }
    }

    fn copy_path(&mut self, p: &std::path::Path, cx: &mut Context<Self>) {
        cx.write_to_clipboard(ClipboardItem::new_string(p.to_string_lossy().into_owned()));
        self.status = format!("Copied {}", p.display());
    }

    /// Copy Path (Ctrl+Shift+C): the selected row's path in the Project view, else the
    /// active file's.
    pub(super) fn copy_current_path(&mut self, window: &Window, cx: &mut Context<Self>) {
        let path = if self.tree_focus.is_focused(window) {
            self.selected_path().map(|(_, p, _)| p)
        } else {
            self.active_editor().and_then(|e| e.read(cx).path.clone())
        };
        match path {
            Some(p) => self.copy_path(&p, cx),
            None => self.status = "No file to copy the path of".into(),
        }
    }

    /// Row `i` of the menu at `depth`: runs an item, opens a submenu.
    fn run_menu_entry(&mut self, depth: usize, i: usize, window: &mut Window, cx: &mut Context<Self>) {
        let Some(m) = self.menu.as_mut().and_then(|m| m.level_mut(depth)) else { return };
        match m.entries.get(i) {
            Some(Entry::Submenu { .. }) => {
                m.selected = Some(i);
                m.open_child(i, true);
            }
            Some(Entry::Item { enabled: true, run, .. }) => {
                let run = run.clone();
                self.menu = None;
                run(self, window, cx);
            }
            _ => {}
        }
        cx.notify();
    }

    /// Keys while a menu is open. Always consumes the key. They act on the deepest open level.
    pub(super) fn menu_key(&mut self, e: &KeyDownEvent, window: &mut Window, cx: &mut Context<Self>) {
        let Some(root) = self.menu.as_mut() else { return };
        let depth = root.depth();
        let key = e.keystroke.key.as_str();
        if key == "escape" || key == "left" {
            if depth > 0 {
                if let Some(parent) = root.level_mut(depth - 1) {
                    parent.child = None;
                }
            } else if key == "escape" {
                self.menu = None;
            }
            cx.notify();
            return;
        }
        let Some(m) = root.level_mut(depth) else { return };
        let selectable: Vec<usize> = m.entries.iter().enumerate().filter(|(_, e)| e.selectable()).map(|(i, _)| i).collect();
        let at = m.selected.and_then(|s| selectable.iter().position(|&i| i == s));
        match key {
            "down" => m.selected = selectable.get(at.map_or(0, |a| (a + 1) % selectable.len().max(1))).copied(),
            "up" => {
                m.selected = selectable.get(at.map_or(selectable.len().saturating_sub(1), |a| (a + selectable.len() - 1) % selectable.len().max(1))).copied()
            }
            "right" => {
                if let Some(i) = m.selected.filter(|&i| matches!(m.entries.get(i), Some(Entry::Submenu { .. }))) {
                    m.open_child(i, true);
                }
            }
            "enter" => {
                if let Some(i) = m.selected {
                    return self.run_menu_entry(depth, i, window, cx);
                }
            }
            _ => {}
        }
        cx.notify();
    }

    pub(super) fn render_menu(&self, viewport: Size<Pixels>, cx: &mut Context<Self>) -> Option<gpui::AnyElement> {
        let root = self.menu.as_ref()?;
        let (vw, vh) = (f32::from(viewport.width), f32::from(viewport.height));
        let mut panels = Vec::new();
        let mut parent_x: Option<f32> = None;
        for depth in 0..=root.depth() {
            let Some(m) = root.level(depth) else { break };
            let h = m.height().min(vh - 8.0);
            // Keep the menu inside the window: flip up / left near the edges (a submenu flips to
            // the parent's left side).
            let (mut x, mut y) = (f32::from(m.pos.x), f32::from(m.pos.y));
            if x + MENU_W > vw {
                x = match parent_x {
                    Some(px_) => (px_ - MENU_W + 6.0).max(0.0),
                    None => (x - MENU_W).max(0.0),
                };
            }
            if y + h > vh {
                y = if depth == 0 { (y - h).max(0.0) } else { (vh - h - 4.0).max(0.0) };
            }
            m.placed.set((x, y));
            parent_x = Some(x);
            panels.push(self.render_menu_panel(m, depth, x, y, h, cx));
        }
        Some(
            div()
                .absolute()
                .top_0()
                .left_0()
                .size_full()
                .occlude()
                .on_any_mouse_down(cx.listener(|this, _, _, cx| {
                    this.menu = None;
                    cx.notify();
                }))
                .children(panels)
                .into_any_element(),
        )
    }

    fn render_menu_panel(&self, m: &ContextMenu, depth: usize, x: f32, y: f32, h: f32, cx: &mut Context<Self>) -> gpui::AnyElement {
        let selected = m.selected;
        let open = m.child.as_ref().map(|(i, _)| *i);
        let rows = m.entries.iter().enumerate().map(|(i, e)| match e {
            Entry::Separator => div().h(ui_px(9.)).flex().items_center().child(div().h(ui_px(1.)).w_full().bg(Theme::divider())).into_any_element(),
            Entry::Item { .. } | Entry::Submenu { .. } => {
                let (label, enabled, right, sub) = match e {
                    Entry::Item { label, shortcut, enabled, .. } => (label.clone(), *enabled, shortcut.clone(), false),
                    Entry::Submenu { label, .. } => (label.clone(), true, "›".to_string(), true),
                    Entry::Separator => unreachable!(),
                };
                div()
                    .id(("menu-item", depth * 1000 + i))
                    .h(ui_px(ITEM_H))
                    .flex_shrink_0()
                    .px(px(10.))
                    .flex()
                    .items_center()
                    .gap(px(16.))
                    .rounded(px(4.))
                    .text_color(if enabled { Theme::text() } else { Theme::text_dim() })
                    .when(selected == Some(i) || open == Some(i), |d| d.bg(Theme::selection_row()))
                    // The mouse moves the selection, so hover and keyboard never show two rows;
                    // it also opens the row's submenu, or closes another one.
                    .when(enabled, |d| {
                        d.cursor_pointer().on_mouse_move(cx.listener(move |this, _, _, cx| {
                            let Some(root) = this.menu.as_mut() else { return };
                            // Moving inside a submenu cancels a pending switch of its parent.
                            if depth > 0 {
                                if let Some(parent) = root.level_mut(depth - 1) {
                                    parent.pending = None;
                                }
                            }
                            let Some(m) = root.level_mut(depth) else { return };
                            let open = m.child.as_ref().map(|(j, _)| *j);
                            if m.selected == Some(i) && open == sub.then_some(i) {
                                m.pending = None;
                                return;
                            }
                            // With a submenu open, another row takes over only after a pause, so a
                            // diagonal move toward the submenu does not switch it on the way.
                            if open.is_some_and(|j| j != i) {
                                if m.pending == Some(i) {
                                    return;
                                }
                                m.pending = Some(i);
                                let timer = cx.background_executor().timer(std::time::Duration::from_millis(250));
                                cx.spawn(async move |this, cx| {
                                    timer.await;
                                    let _ = this.update(cx, |this, cx| {
                                        let Some(m) = this.menu.as_mut().and_then(|m| m.level_mut(depth)) else { return };
                                        if m.pending == Some(i) {
                                            m.pending = None;
                                            m.select_row(i);
                                            cx.notify();
                                        }
                                    });
                                })
                                .detach();
                                return;
                            }
                            m.select_row(i);
                            cx.notify();
                        }))
                    })
                    .on_click(cx.listener(move |this, _, window, cx| this.run_menu_entry(depth, i, window, cx)))
                    .child(label)
                    .child(div().flex_1())
                    .child(div().text_size(px(if sub { 14. } else { 12. })).text_color(Theme::text_muted()).child(right))
                    .into_any_element()
            }
        });
        div()
            .id(("context-menu", depth))
            .absolute()
            .left(px(x))
            .top(px(y))
            .w(px(MENU_W))
            .max_h(px(h))
            .overflow_y_scroll()
            .p(px(5.))
            .flex()
            .flex_col()
            .bg(Theme::panel())
            .border_1()
            .border_color(Theme::divider())
            .rounded(px(8.))
            .map(crate::ui_kit::floating)
            .text_size(ui_px(13.))
            // A press inside a menu panel is not a click outside.
            .on_any_mouse_down(|_, _, cx| cx.stop_propagation())
            .children(rows)
            .into_any_element()
    }
}
