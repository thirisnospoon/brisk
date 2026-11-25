//! Bookmarks (F11, Ctrl+F11 with a mnemonic, Ctrl+<digit> to jump, Shift+F11 list, Alt+2
//! window) and the File Structure popup (Ctrl+F12), as in PyCharm. Bookmarks of open files
//! live in their editors (and follow edits); the rest are kept here. They are saved per
//! project in the config folder.

use super::{PopupItem, PopupKind, Target, Workspace};
use crate::editor::Editor;
use crate::theme::ui_px;
use crate::theme::Theme;
use brisk_keymap::Action;
use gpui::prelude::*;
use gpui::{div, px, Context, Entity, Keystroke, Window};
use std::path::{Path, PathBuf};

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Stored {
    pub path: PathBuf,
    pub line: usize,
    pub mnemonic: Option<char>,
}

fn store_path() -> PathBuf {
    brisk_db::config_dir().join("bookmarks.json")
}

fn root_key(root: &Path) -> String {
    root.to_string_lossy().to_lowercase()
}

/// Each project's bookmarks as last read from or written to the file: a save that leaves them
/// as they are does not rewrite it.
static WRITTEN: std::sync::Mutex<Option<std::collections::HashMap<String, Vec<serde_json::Value>>>> = std::sync::Mutex::new(None);

fn written(key: &str, list: &[serde_json::Value]) -> bool {
    WRITTEN.lock().unwrap_or_else(|e| e.into_inner()).as_ref().and_then(|m| m.get(key)).is_some_and(|l| l.as_slice() == list)
}

fn set_written(key: String, list: Vec<serde_json::Value>) {
    WRITTEN.lock().unwrap_or_else(|e| e.into_inner()).get_or_insert_with(Default::default).insert(key, list);
}

/// Bookmarks saved for this project.
pub fn load(root: &Path) -> Vec<Stored> {
    let Ok(s) = std::fs::read_to_string(store_path()) else {
        set_written(root_key(root), Vec::new());
        return Vec::new();
    };
    let Ok(v) = serde_json::from_str::<serde_json::Value>(&s) else { return Vec::new() };
    let Some(list) = v.get(root_key(root)).and_then(|l| l.as_array()) else {
        set_written(root_key(root), Vec::new());
        return Vec::new();
    };
    set_written(root_key(root), list.clone());
    list.iter()
        .filter_map(|b| {
            Some(Stored {
                path: root.join(b.get("path")?.as_str()?),
                line: b.get("line")?.as_u64()? as usize,
                mnemonic: b.get("mnemonic").and_then(|m| m.as_str()).and_then(|m| m.chars().next()),
            })
        })
        .collect()
}

fn mnemonics() -> impl Iterator<Item = char> {
    ('1'..='9').chain(std::iter::once('0')).chain('A'..='Z')
}

impl Workspace {
    /// All bookmarks, by file and line.
    pub(super) fn all_bookmarks(&self, cx: &gpui::App) -> Vec<Stored> {
        let mut v = self.bookmarks.clone();
        for e in self.editors() {
            let e = e.read(cx);
            let Some(p) = e.path.clone() else { continue };
            v.retain(|b| b.path != p);
            v.extend(e.bookmarks_snapshot().into_iter().map(|(line, mnemonic)| Stored { path: p.clone(), line, mnemonic }));
        }
        v.sort_by(|a, b| a.path.cmp(&b.path).then(a.line.cmp(&b.line)));
        v
    }

    pub(super) fn persist_bookmarks(&self, cx: &gpui::App) {
        let list: Vec<serde_json::Value> = self
            .all_bookmarks(cx)
            .into_iter()
            .map(|b| {
                let rel = b.path.strip_prefix(&self.root).unwrap_or(&b.path).to_string_lossy().replace('\\', "/");
                serde_json::json!({ "path": rel, "line": b.line, "mnemonic": b.mnemonic.map(|c| c.to_string()) })
            })
            .collect();
        let key = root_key(&self.root);
        if written(&key, &list) {
            return;
        }
        let path = store_path();
        let mut all: serde_json::Map<String, serde_json::Value> = std::fs::read_to_string(&path)
            .ok()
            .and_then(|s| serde_json::from_str::<serde_json::Value>(&s).ok())
            .and_then(|v| v.as_object().cloned())
            .unwrap_or_default();
        set_written(key.clone(), list.clone());
        if list.is_empty() {
            all.remove(&key);
        } else {
            all.insert(key, serde_json::Value::Array(list));
        }
        if let Some(d) = path.parent() {
            let _ = std::fs::create_dir_all(d);
        }
        if let Ok(s) = serde_json::to_string_pretty(&serde_json::Value::Object(all)) {
            let _ = std::fs::write(path, s);
        }
    }

    /// A file was opened: its saved bookmarks move into the editor.
    pub(super) fn adopt_bookmarks(&mut self, ed: &Entity<Editor>, cx: &mut Context<Self>) {
        let Some(p) = ed.read(cx).path.clone() else { return };
        let mine: Vec<(usize, Option<char>)> = self.bookmarks.iter().filter(|b| b.path == p).map(|b| (b.line, b.mnemonic)).collect();
        if mine.is_empty() {
            return;
        }
        self.bookmarks.retain(|b| b.path != p);
        ed.update(cx, |e, cx| {
            e.set_bookmarks(&mine);
            cx.notify();
        });
    }

    /// An editor closes: keep its bookmarks.
    pub(super) fn stash_bookmarks(&mut self, ed: &Entity<Editor>, cx: &mut Context<Self>) {
        let Some(p) = ed.read(cx).path.clone() else { return };
        let marks = ed.update(cx, |e, _| e.bookmark_lines());
        self.bookmarks.retain(|b| b.path != p);
        self.bookmarks.extend(marks.into_iter().map(|(line, mnemonic)| Stored { path: p.clone(), line, mnemonic }));
    }

    /// A mnemonic is unique in the project: take it away from wherever it is.
    fn release_mnemonic(&mut self, c: char, cx: &mut Context<Self>) {
        for b in &mut self.bookmarks {
            if b.mnemonic == Some(c) {
                b.mnemonic = None;
            }
        }
        let eds: Vec<_> = self.editors().cloned().collect();
        for e in eds {
            e.update(cx, |e, cx| {
                e.clear_mnemonic(c);
                cx.notify();
            });
        }
    }

    fn toggle_bookmark_here(&mut self, mnemonic: Option<char>, window: &mut Window, cx: &mut Context<Self>) {
        let Some(ed) = self.focused_editor(window, cx).or_else(|| self.active_editor().cloned()) else { return };
        if ed.read(cx).path.is_none() {
            return;
        }
        if let Some(c) = mnemonic {
            self.release_mnemonic(c, cx);
        }
        let on = ed.update(cx, |e, cx| {
            let on = e.toggle_bookmark(mnemonic);
            cx.notify();
            on
        });
        let line = ed.read(cx).caret_line_col().0 + 1;
        self.status = match (on, mnemonic) {
            (true, Some(c)) => format!("Bookmark {c} set on line {line}"),
            (true, None) => format!("Bookmark set on line {line}"),
            (false, _) => format!("Bookmark removed from line {line}"),
        };
        self.persist_bookmarks(cx);
        cx.notify();
    }

    /// Bookmark actions and File Structure. True if handled.
    pub(super) fn bookmark_action(&mut self, action: Action, window: &mut Window, cx: &mut Context<Self>) -> bool {
        match action {
            Action::ToggleBookmark => self.toggle_bookmark_here(None, window, cx),
            Action::ToggleBookmarkWithMnemonic => {
                if self.active_editor().is_some() {
                    self.open_popup(PopupKind::BookmarkMnemonic);
                }
            }
            Action::ShowBookmarks => {
                self.structure = self.bookmark_items("", cx);
                self.open_popup(PopupKind::Bookmarks);
            }
            Action::FileStructurePopup => self.open_file_structure(window, cx),
            _ => return false,
        }
        cx.notify();
        true
    }

    /// Ctrl+0..9: jump to the bookmark with that digit. True if there is one.
    pub(super) fn bookmark_jump_key(&mut self, k: &Keystroke, window: &mut Window, cx: &mut Context<Self>) -> bool {
        let m = &k.modifiers;
        if !m.control || m.alt || m.shift || m.platform || k.key.len() != 1 {
            return false;
        }
        let Some(c) = k.key.chars().next().filter(|c| c.is_ascii_digit()) else { return false };
        let Some(b) = self.all_bookmarks(cx).into_iter().find(|b| b.mnemonic == Some(c)) else { return false };
        self.open_symbol(b.path, b.line, 0, window, cx);
        true
    }

    pub(super) fn mnemonic_items(&self, q: &str) -> Vec<PopupItem> {
        let q = q.trim().to_uppercase();
        mnemonics()
            .filter(|c| q.is_empty() || q.starts_with(*c))
            .map(|c| PopupItem { label: c.to_string(), detail: String::new(), hint: String::new(), target: Target::Mnemonic(c) })
            .collect()
    }

    pub(super) fn bookmark_items(&self, q: &str, cx: &gpui::App) -> Vec<PopupItem> {
        let ql = q.trim().to_lowercase();
        self.all_bookmarks(cx)
            .into_iter()
            .filter_map(|b| {
                let rel = b.path.strip_prefix(&self.root).unwrap_or(&b.path).to_string_lossy().replace('\\', "/");
                let text = self.line_text(&b.path, b.line, cx);
                let label = format!("{}{}:{}", b.mnemonic.map(|c| format!("{c}  ")).unwrap_or_default(), rel, b.line + 1);
                (ql.is_empty() || label.to_lowercase().contains(&ql) || text.to_lowercase().contains(&ql)).then(|| PopupItem {
                    label,
                    detail: text.trim().chars().take(80).collect(),
                    hint: String::new(),
                    target: Target::Bookmark(b.path, b.line),
                })
            })
            .collect()
    }

    /// The text of a line, from an open editor or the file.
    fn line_text(&self, p: &Path, line: usize, cx: &gpui::App) -> String {
        if let Some(e) = self.editors().find(|e| e.read(cx).path.as_deref() == Some(p)) {
            let e = e.read(cx);
            return if line < e.buffer.len_lines() { e.buffer.line(line) } else { String::new() };
        }
        std::fs::read_to_string(p).ok().and_then(|t| t.lines().nth(line).map(str::to_string)).unwrap_or_default()
    }

    /// Ctrl+F12: the current file's classes and functions, filterable; the one around the
    /// caret is selected.
    fn open_file_structure(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let Some(ed) = self.focused_editor(window, cx).or_else(|| self.active_editor().cloned()) else { return };
        let Some(path) = ed.read(cx).path.clone() else { return };
        let (items, caret) = ed.update(cx, |e, _| (e.outline(), e.caret_line_col().0));
        let current = items.iter().rposition(|o| o.line <= caret && caret <= o.end_line);
        self.structure = items
            .into_iter()
            .map(|o| PopupItem {
                label: format!("{}{}", "    ".repeat(o.depth), o.name),
                detail: format!(":{}", o.line + 1),
                hint: String::new(),
                target: Target::Symbol(path.clone(), o.line, 0, o.kind),
            })
            .collect();
        self.open_popup(PopupKind::FileStructure);
        if let (Some(p), Some(i)) = (self.popup.as_mut(), current) {
            p.selected = i;
        }
    }

    pub(super) fn structure_items(&self, q: &str) -> Vec<PopupItem> {
        let q = q.trim();
        if q.is_empty() {
            return self.structure.clone();
        }
        if matches!(self.popup.as_ref().map(|p| p.kind), Some(PopupKind::Bookmarks | PopupKind::LocalHistory)) {
            let ql = q.to_lowercase();
            return self.structure.iter().filter(|i| i.label.to_lowercase().contains(&ql) || i.detail.to_lowercase().contains(&ql)).cloned().collect();
        }
        let mut scored: Vec<(i32, &PopupItem)> =
            self.structure.iter().filter_map(|i| super::project_ui::name_score(i.label.trim(), q).map(|s| (s, i))).collect();
        scored.sort_by_key(|s| std::cmp::Reverse(s.0));
        scored.into_iter().map(|(_, i)| i.clone()).collect()
    }

    pub(super) fn pick_mnemonic(&mut self, c: char, window: &mut Window, cx: &mut Context<Self>) {
        self.toggle_bookmark_here(Some(c), window, cx);
        self.focus_editor(window, cx);
    }

    /// The Bookmarks tool window (Alt+2).
    pub(super) fn render_bookmarks(&mut self, cx: &mut Context<Self>) -> gpui::AnyElement {
        let all = self.all_bookmarks(cx);
        let rows: Vec<(Stored, String)> = all
            .into_iter()
            .map(|b| {
                let t = self.line_text(&b.path, b.line, cx);
                (b, t)
            })
            .collect();
        div()
            .id("bookmarks")
            .flex()
            .flex_col()
            .size_full()
            .overflow_y_scroll()
            .child(Self::panel_header("Bookmarks"))
            .when(rows.is_empty(), |d| {
                d.child(
                    div()
                        .px(px(12.))
                        .text_size(ui_px(13.))
                        .text_color(Theme::text_muted())
                        .child("No bookmarks. F11 sets one on the caret line, Ctrl+F11 with a letter or digit."),
                )
            })
            .children(rows.into_iter().enumerate().map(|(i, (b, text))| {
                let name = b.path.file_name().map(|n| n.to_string_lossy().into_owned()).unwrap_or_default();
                let (path, line) = (b.path.clone(), b.line);
                let (rm_path, rm_line) = (b.path.clone(), b.line);
                div()
                    .id(("bookmark", i))
                    .h(ui_px(26.))
                    .flex()
                    .items_center()
                    .gap(px(8.))
                    .px(px(12.))
                    .text_size(ui_px(13.))
                    .hover(|s| s.bg(Theme::hover_row()))
                    .cursor_pointer()
                    .on_click(cx.listener(move |this, _, window, cx| this.open_symbol(path.clone(), line, 0, window, cx)))
                    .child(
                        div()
                            .w(px(16.))
                            .h(ui_px(16.))
                            .rounded(px(3.))
                            .bg(Theme::bookmark())
                            .text_color(Theme::bg())
                            .text_size(ui_px(10.))
                            .flex()
                            .items_center()
                            .justify_center()
                            .child(b.mnemonic.map(|c| c.to_string()).unwrap_or_default()),
                    )
                    .child(format!("{name}:{}", b.line + 1))
                    .child(div().flex_1().min_w_0().overflow_hidden().text_color(Theme::text_muted()).child(text.trim().to_string()))
                    .child(div().id(("bookmark-rm", i)).group("close-x").child(crate::icons::close_x("close-x", 12.)).on_click(cx.listener(
                        move |this, _, _, cx| {
                            cx.stop_propagation();
                            this.remove_bookmark(&rm_path, rm_line, cx);
                        },
                    )))
            }))
            .into_any_element()
    }

    fn remove_bookmark(&mut self, p: &Path, line: usize, cx: &mut Context<Self>) {
        self.bookmarks.retain(|b| !(b.path == p && b.line == line));
        if let Some(e) = self.editors().find(|e| e.read(cx).path.as_deref() == Some(p)).cloned() {
            e.update(cx, |e, cx| {
                e.remove_bookmark_at(line);
                cx.notify();
            });
        }
        self.persist_bookmarks(cx);
        cx.notify();
    }
}
