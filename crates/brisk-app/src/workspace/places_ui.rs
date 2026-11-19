//! Where the caret has been, as PyCharm keeps it: a caret that moves to another file or more
//! than a few lines away starts a new place (and the old one goes on the Back list), an edit
//! marks the place as changed. Recent Locations (Ctrl+Shift+E; again: changed only) lists them
//! with the line's code, Last Edit Location (Ctrl+Shift+Backspace) steps back through the
//! changed ones.

use super::lsp_ui::NavPos;
use super::{PopupItem, PopupKind, Target, Workspace};
use crate::editor::Editor;
use gpui::{Context, Entity, Window};
use std::path::PathBuf;

/// Caret moves within this many lines stay the same place.
const NEAR: usize = 10;
const LIMIT: usize = 60;

#[derive(Clone, Debug)]
pub(super) struct Place {
    pub path: PathBuf,
    pub line: usize,
    pub col: usize,
    pub changed: bool,
}

#[derive(Default)]
pub(super) struct Places {
    /// Oldest first.
    pub list: Vec<Place>,
    /// Buffer version per file at the last look (an edit changes it).
    versions: std::collections::HashMap<PathBuf, u64>,
    /// Back / Forward and Last Edit Location jump: the caret move they cause is not a new place.
    pub jumping: bool,
    /// Last Edit Location pressed again: how many changed places back it went.
    pub edit_steps: usize,
    /// Recent Locations shows changed places only.
    pub changed_only: bool,
}

impl Place {
    fn near(&self, path: &std::path::Path, line: usize) -> bool {
        self.path == path && self.line.abs_diff(line) <= NEAR
    }
}

impl Workspace {
    /// The editor notified: a new place, the same one, or an edit in it.
    pub(super) fn track_place(&mut self, ed: &Entity<Editor>, cx: &mut Context<Self>) {
        if self.active_editor() != Some(ed) {
            return;
        }
        let e = ed.read(cx);
        let Some(path) = e.path.clone() else { return };
        let (line, col) = e.caret_line_col();
        let v = e.buffer.version();
        let edited = self.places.versions.insert(path.clone(), v).is_some_and(|old| old != v);
        let jumping = std::mem::take(&mut self.places.jumping);
        if edited {
            self.places.edit_steps = 0;
        }
        let p = &mut self.places;
        match p.list.last_mut() {
            Some(last) if last.near(&path, line) => {
                last.line = line;
                last.col = col;
                last.changed |= edited;
            }
            last => {
                // A jump away: the place left goes on the Back list (Back / Forward themselves
                // and opening a file by Go to Declaration put it there already).
                if let Some(prev) = last.cloned() {
                    let back = &mut self.nav.back;
                    if !jumping && !back.last().is_some_and(|b| prev.near(&b.path, b.line)) {
                        back.push(NavPos { path: prev.path.clone(), line: prev.line, col: prev.col });
                        if back.len() > 100 {
                            back.remove(0);
                        }
                        self.nav.forward.clear();
                    }
                }
                let p = &mut self.places;
                // One entry per spot: an older visit nearby is replaced (keeping its "changed").
                let mut changed = edited;
                p.list.retain(|o| {
                    let same = o.near(&path, line);
                    changed |= same && o.changed;
                    !same
                });
                p.list.push(Place { path, line, col, changed });
                if p.list.len() > LIMIT {
                    p.list.remove(0);
                }
            }
        }
    }

    pub(super) fn recent_locations(&mut self, cx: &mut Context<Self>) {
        // Pressed again while open: changed places only (PyCharm's "Show changed only").
        let again = self.popup.as_ref().is_some_and(|p| p.kind == PopupKind::RecentLocations);
        self.places.changed_only = again && !self.places.changed_only;
        self.structure = self.place_items(cx);
        self.open_popup(PopupKind::RecentLocations);
        // The place we are at is first: start on the one before it, like Recent Files.
        if let Some(p) = self.popup.as_mut() {
            p.selected = usize::from(p.items.len() > 1);
        }
        cx.notify();
    }

    fn place_items(&self, cx: &gpui::App) -> Vec<PopupItem> {
        let changed_only = self.places.changed_only;
        self.places
            .list
            .iter()
            .rev()
            .filter(|p| !changed_only || p.changed)
            .filter(|p| p.path.is_file())
            .map(|p| {
                let name = p.path.file_name().map(|n| n.to_string_lossy().into_owned()).unwrap_or_default();
                // The code on that line, from the open editor if there is one.
                let code = self
                    .editors()
                    .find(|e| e.read(cx).path.as_deref() == Some(p.path.as_path()))
                    .map(|e| e.read(cx).buffer.line(p.line.min(e.read(cx).buffer.len_lines().saturating_sub(1))))
                    .or_else(|| std::fs::read_to_string(&p.path).ok().and_then(|t| t.lines().nth(p.line).map(str::to_string)))
                    .unwrap_or_default();
                PopupItem {
                    label: format!("{name}:{}{}", p.line + 1, if p.changed { "  •" } else { "" }),
                    detail: code.trim().chars().take(120).collect(),
                    hint: String::new(),
                    target: Target::Place(p.path.clone(), p.line, p.col),
                }
            })
            .collect()
    }

    /// Ctrl+Shift+Backspace: the last changed place; pressed again, the one before it.
    pub(super) fn last_edit_location(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let here = self.active_editor().and_then(|e| {
            let e = e.read(cx);
            Some((e.path.clone()?, e.caret_line_col().0))
        });
        let changed: Vec<Place> = self.places.list.iter().rev().filter(|p| p.changed && p.path.is_file()).cloned().collect();
        // Skip the place the caret is at (the first press from inside the last edit goes on).
        let skip = usize::from(here.as_ref().is_some_and(|(f, l)| changed.first().is_some_and(|p| p.near(f, *l)))).max(self.places.edit_steps);
        let Some(to) = changed.get(skip).or(changed.last()).cloned() else {
            self.status = "No edit locations".into();
            return cx.notify();
        };
        self.places.edit_steps = skip + 1;
        let steps = self.places.edit_steps;
        self.places.jumping = true;
        self.navigate_to(NavPos { path: to.path, line: to.line, col: to.col }, window, cx);
        self.places.edit_steps = steps;
    }
}
