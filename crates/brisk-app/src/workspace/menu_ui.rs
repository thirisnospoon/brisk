//! Context menus (right click): one mechanism for the editor and the project tree.
//! Keyboard: ↑/↓, Enter, Esc. A click outside closes the menu without reaching what is under it.

use super::Workspace;
use crate::editor::Editor;
use crate::theme::Theme;
use brisk_keymap::Action;
use gpui::prelude::*;
use gpui::{div, px, ClipboardItem, Context, Entity, KeyDownEvent, Pixels, Point, Size, Window};
use std::rc::Rc;

pub(super) type MenuFn = Rc<dyn Fn(&mut Workspace, &mut Window, &mut Context<Workspace>)>;

pub(super) enum Entry {
    Item { label: String, shortcut: String, enabled: bool, run: MenuFn },
    Separator,
}

pub(super) struct ContextMenu {
    pub pos: Point<Pixels>,
    pub entries: Vec<Entry>,
    pub selected: Option<usize>,
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
        self.menu = Some(ContextMenu { pos, entries, selected: None });
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
        }
        if let Some(p) = path {
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
            run: Rc::new(move |this, _, cx| {
                cx.write_to_clipboard(ClipboardItem::new_string(p.to_string_lossy().into_owned()));
                this.status = format!("Copied {}", p.display());
            }),
        }
    }

    fn run_menu_entry(&mut self, i: usize, window: &mut Window, cx: &mut Context<Self>) {
        let Some(m) = self.menu.take() else { return };
        if let Some(Entry::Item { enabled: true, run, .. }) = m.entries.get(i) {
            let run = run.clone();
            run(self, window, cx);
        }
        cx.notify();
    }

    /// Keys while a menu is open. Always consumes the key.
    pub(super) fn menu_key(&mut self, e: &KeyDownEvent, window: &mut Window, cx: &mut Context<Self>) {
        let Some(m) = self.menu.as_mut() else { return };
        let selectable: Vec<usize> = m.entries.iter().enumerate().filter(|(_, e)| matches!(e, Entry::Item { enabled: true, .. })).map(|(i, _)| i).collect();
        let at = m.selected.and_then(|s| selectable.iter().position(|&i| i == s));
        match e.keystroke.key.as_str() {
            "escape" => self.menu = None,
            "down" => m.selected = selectable.get(at.map_or(0, |a| (a + 1) % selectable.len().max(1))).copied(),
            "up" => m.selected = selectable.get(at.map_or(selectable.len().saturating_sub(1), |a| (a + selectable.len() - 1) % selectable.len().max(1))).copied(),
            "enter" => {
                if let Some(i) = m.selected {
                    return self.run_menu_entry(i, window, cx);
                }
            }
            _ => {}
        }
        cx.notify();
    }

    pub(super) fn render_menu(&self, viewport: Size<Pixels>, cx: &mut Context<Self>) -> Option<gpui::AnyElement> {
        let m = self.menu.as_ref()?;
        let h: f32 = m.entries.iter().map(|e| if matches!(e, Entry::Separator) { 9.0 } else { ITEM_H }).sum::<f32>() + 10.0;
        // Keep the menu inside the window: flip up / left near the edges.
        let (vw, vh) = (f32::from(viewport.width), f32::from(viewport.height));
        let (mut x, mut y) = (f32::from(m.pos.x), f32::from(m.pos.y));
        if x + MENU_W > vw {
            x = (x - MENU_W).max(0.0);
        }
        if y + h > vh {
            y = (y - h).max(0.0);
        }
        let selected = m.selected;
        let rows = m.entries.iter().enumerate().map(|(i, e)| match e {
            Entry::Separator => div().h(px(9.)).flex().items_center().child(div().h(px(1.)).w_full().bg(Theme::divider())).into_any_element(),
            Entry::Item { label, shortcut, enabled, .. } => {
                let enabled = *enabled;
                div()
                    .id(("menu-item", i))
                    .h(px(ITEM_H))
                    .px(px(10.))
                    .flex()
                    .items_center()
                    .gap(px(16.))
                    .rounded(px(4.))
                    .text_color(if enabled { Theme::text() } else { Theme::text_dim() })
                    .when(selected == Some(i), |d| d.bg(Theme::selection_row()))
                    // The mouse moves the selection, so hover and keyboard never show two rows.
                    .when(enabled, |d| {
                        d.cursor_pointer().on_mouse_move(cx.listener(move |this, _, _, cx| {
                            if let Some(m) = this.menu.as_mut() {
                                if m.selected != Some(i) {
                                    m.selected = Some(i);
                                    cx.notify();
                                }
                            }
                        }))
                    })
                    .on_click(cx.listener(move |this, _, window, cx| this.run_menu_entry(i, window, cx)))
                    .child(label.clone())
                    .child(div().flex_1())
                    .child(div().text_size(px(12.)).text_color(Theme::text_muted()).child(shortcut.clone()))
                    .into_any_element()
            }
        });
        Some(
            div()
                .absolute()
                .top_0()
                .left_0()
                .size_full()
                .occlude()
                .child(
                    div()
                        .id("context-menu")
                        .absolute()
                        .left(px(x))
                        .top(px(y))
                        .w(px(MENU_W))
                        .p(px(5.))
                        .flex()
                        .flex_col()
                        .bg(Theme::panel())
                        .border_1()
                        .border_color(Theme::divider())
                        .rounded(px(8.))
                        .shadow_lg()
                        .text_size(px(13.))
                        .on_mouse_down_out(cx.listener(|this, _, _, cx| {
                            this.menu = None;
                            cx.notify();
                        }))
                        .children(rows),
                )
                .into_any_element(),
        )
    }
}
