//! Keymap (Find Action → Keymap, or File → Keymap): every action with its shortcuts and a
//! filter that also matches shortcuts ("ctrl+d"). Enter on an action offers Add Keyboard
//! Shortcut, Remove and Reset; Add opens the "press the keys" box, which shows the actions
//! that already use them and takes the shortcut away from them on Enter. Also Import
//! PyCharm Keymap (the active custom keymap of the newest PyCharm) and Reset All.

use super::git_merge_ui::dialog_frame;
use super::{PopupItem, PopupKind, Target, Workspace};
use crate::keymap_store;
use crate::theme::{ui_px, Theme};
use brisk_keymap::{chord_id, pycharm_keymap, Action, KeyResolver, Keystroke, ALL_ACTIONS};
use gpui::prelude::*;
use gpui::{div, px, Context, KeyDownEvent, Window};

/// The "press the keys" box.
pub struct KeyCapture {
    pub action: Action,
    pub strokes: Vec<Keystroke>,
}

fn chord_label(c: &[Keystroke]) -> String {
    c.iter().map(|k| k.to_string()).collect::<Vec<_>>().join(", ")
}

impl Workspace {
    /// A new keymap was saved (here or in another window): use it.
    pub(super) fn sync_keymap(&mut self) {
        let g = keymap_store::generation();
        if g != self.keymap_gen {
            self.keymap_gen = g;
            self.resolver = KeyResolver::new(keymap_store::effective());
        }
    }

    pub(super) fn keymap_items(&self, q: &str) -> Vec<PopupItem> {
        let km = self.resolver.keymap();
        let user = keymap_store::user();
        let ql = q.trim().to_lowercase();
        // "ctrl+d", "Ctrl+Shift+A": the query is a shortcut.
        let as_keys = ql.replace(['+', ' '], "-");
        let mut v = Vec::new();
        if let Some(e) = keymap_store::load_error() {
            v.push(PopupItem {
                label: "Your keymap.json could not be read: the defaults are in use".into(),
                detail: e,
                hint: String::new(),
                target: Target::Note,
            });
        }
        if ql.is_empty() || "import pycharm keymap".contains(&ql) {
            v.push(PopupItem {
                label: "Import PyCharm Keymap".into(),
                detail: "your active custom keymap from PyCharm's settings".into(),
                hint: String::new(),
                target: Target::KeymapImport,
            });
        }
        if !user.is_empty() && (ql.is_empty() || "reset all shortcuts".contains(&ql)) {
            v.push(PopupItem {
                label: "Reset All Shortcuts".into(),
                detail: format!("{} changed action(s) back to PyCharm's defaults", user.bindings.len()),
                hint: String::new(),
                target: Target::KeymapResetAll,
            });
        }
        for &a in ALL_ACTIONS {
            let info = a.info();
            let chords = km.chords_for(a);
            let mut shortcuts: Vec<String> = Vec::new();
            for t in km.triggers_for(a) {
                // The same keys in two contexts (Commit and Git log) are shown once.
                let t = t.to_string();
                if !shortcuts.contains(&t) {
                    shortcuts.push(t);
                }
            }
            let hit = ql.is_empty()
                || info.title.to_lowercase().contains(&ql)
                || info.group.to_lowercase().contains(&ql)
                || chords.iter().any(|c| chord_id(c) == as_keys)
                || shortcuts.iter().any(|s| s.to_lowercase() == ql);
            if !hit {
                continue;
            }
            let changed = user.bindings.contains_key(info.id);
            v.push(PopupItem {
                label: info.title.into(),
                detail: if changed { format!("{} · changed", info.group) } else { info.group.into() },
                hint: shortcuts.join("  "),
                target: Target::KeymapPick(a),
            });
        }
        v
    }

    pub(super) fn keymap_action_items(&self, a: Action) -> Vec<PopupItem> {
        let km = self.resolver.keymap();
        let mut v = vec![PopupItem { label: "Add Keyboard Shortcut…".into(), detail: String::new(), hint: String::new(), target: Target::KeymapAdd(a) }];
        for c in km.chords_for(a) {
            v.push(PopupItem {
                label: format!("Remove {}", chord_label(&c)),
                detail: String::new(),
                hint: String::new(),
                target: Target::KeymapRemove(a, chord_id(&c)),
            });
        }
        if keymap_store::user().bindings.contains_key(a.info().id) {
            let defaults: Vec<String> = pycharm_keymap().chords_for(a).iter().map(|c| chord_label(c)).collect();
            let detail = if defaults.is_empty() { "no shortcut".to_string() } else { defaults.join(", ") };
            v.push(PopupItem { label: "Reset to Default".into(), detail, hint: String::new(), target: Target::KeymapReset(a) });
        }
        v
    }

    /// Applies `f` to the saved keymap; None (and a notification) when it cannot be saved.
    fn keymap_change<R>(&mut self, f: impl FnOnce(&mut brisk_keymap::UserKeymap) -> R, done: String, cx: &mut Context<Self>) -> Option<R> {
        let res = keymap_store::change(f);
        cx.notify();
        match res {
            Ok(r) => {
                self.sync_keymap();
                self.status = done;
                Some(r)
            }
            Err(e) => {
                self.show_toast(format!("Cannot save the keymap: {e}"), true, cx);
                None
            }
        }
    }

    pub(super) fn keymap_pick(&mut self, t: Target, window: &mut Window, cx: &mut Context<Self>) {
        let base = pycharm_keymap();
        match t {
            Target::KeymapPick(a) => {
                self.open_popup(PopupKind::KeymapAction(a));
                cx.notify();
            }
            Target::KeymapAdd(a) => {
                self.popup = None;
                self.key_capture = Some(KeyCapture { action: a, strokes: Vec::new() });
                cx.notify();
            }
            Target::KeymapRemove(a, c) => {
                self.popup = None;
                if let Some(chord) = brisk_keymap::parse_chord(&c) {
                    let label = chord_label(&chord);
                    self.keymap_change(|u| u.remove(&base, a, &chord), format!("Removed {label} from {}", a.info().title), cx);
                }
                self.focus_editor(window, cx);
            }
            Target::KeymapReset(a) => {
                self.popup = None;
                self.keymap_change(|u| u.reset(a), format!("{}: default shortcuts", a.info().title), cx);
                self.focus_editor(window, cx);
            }
            Target::KeymapResetAll => {
                self.popup = None;
                self.keymap_change(|u| *u = Default::default(), "All shortcuts are PyCharm's defaults again".into(), cx);
                self.focus_editor(window, cx);
            }
            Target::KeymapImport => {
                self.popup = None;
                self.import_pycharm_keymap(cx);
                self.focus_editor(window, cx);
            }
            _ => {}
        }
    }

    /// The active custom keymap of the newest PyCharm that has one.
    fn import_pycharm_keymap(&mut self, cx: &mut Context<Self>) {
        let dirs = brisk_keymap::pycharm_config_dirs();
        if dirs.is_empty() {
            self.show_toast("No PyCharm settings found in %APPDATA%\\JetBrains".into(), true, cx);
            return;
        }
        let Some((dir, (name, text, others))) = dirs.iter().find_map(|d| brisk_keymap::active_keymap(d).map(|k| (d.clone(), k))) else {
            self.show_toast("PyCharm uses its built-in keymap, which is Brisk's default already. Nothing to import.".into(), false, cx);
            return;
        };
        match brisk_keymap::import_xml(&text, &others) {
            Ok(imp) => {
                let version = dir.file_name().map(|n| n.to_string_lossy().into_owned()).unwrap_or_default();
                let mut msg = format!("Imported '{name}' from {version}: {} action(s)", imp.actions);
                if imp.skipped > 0 {
                    msg.push_str(&format!(", {} skipped (actions or keys Brisk does not have)", imp.skipped));
                }
                if let Some(p) = &imp.foreign_parent {
                    msg.push_str(&format!(". It is based on '{p}'; only its own changes were taken, on top of PyCharm's default keymap"));
                }
                if self.keymap_change(|u| u.bindings.extend(imp.keymap.bindings), msg.clone(), cx).is_some() {
                    self.show_toast(msg, false, cx);
                }
            }
            Err(e) => self.show_toast(format!("Cannot read the PyCharm keymap '{name}': {e}"), true, cx),
        }
    }

    /// Keys while the "press the keys" box is open. Esc cancels, Enter applies (once
    /// something was pressed), a third keystroke starts over.
    pub(super) fn key_capture_key(&mut self, e: &KeyDownEvent, window: &mut Window, cx: &mut Context<Self>) {
        let Some(cap) = self.key_capture.as_mut() else { return };
        let stroke = crate::keys::convert(&e.keystroke);
        if stroke.is_modifier_only() {
            return;
        }
        let plain = !stroke.mods.ctrl && !stroke.mods.alt && !stroke.mods.shift && !stroke.mods.meta;
        if plain && stroke.key == "escape" {
            self.key_capture = None;
            self.focus_editor(window, cx);
        } else if plain && stroke.key == "enter" && !cap.strokes.is_empty() {
            let (a, chord) = (cap.action, std::mem::take(&mut cap.strokes));
            self.key_capture = None;
            let base = pycharm_keymap();
            let label = chord_label(&chord);
            // A failed save says so (a notification) instead of "assigned".
            if let Some(taken) = self.keymap_change(|u| u.assign(&base, a, chord), String::new(), cx) {
                self.status = if taken.is_empty() {
                    format!("{label}: {}", a.info().title)
                } else {
                    format!("{label}: {} (removed from {})", a.info().title, taken.iter().map(|t| t.info().title).collect::<Vec<_>>().join(", "))
                };
            }
            self.focus_editor(window, cx);
        } else {
            if cap.strokes.len() >= 2 {
                cap.strokes.clear();
            }
            cap.strokes.push(stroke);
        }
        cx.notify();
    }

    pub(super) fn render_key_capture(&self) -> Option<gpui::AnyElement> {
        let cap = self.key_capture.as_ref()?;
        let km = self.resolver.keymap();
        let conflicts = if cap.strokes.is_empty() { Vec::new() } else { km.conflicts(cap.action, &cap.strokes) };
        let keys = if cap.strokes.is_empty() { "Press the keys…".to_string() } else { chord_label(&cap.strokes) };
        let mut body = dialog_frame("key-capture", 420., &format!("Keyboard Shortcut for '{}'", cap.action.info().title)).child(
            div()
                .px(px(10.))
                .py(px(8.))
                .rounded(px(6.))
                .border_1()
                .border_color(Theme::accent())
                .bg(Theme::bg())
                .text_size(ui_px(15.))
                .text_color(if cap.strokes.is_empty() { Theme::text_muted() } else { Theme::text() })
                .child(keys),
        );
        if !conflicts.is_empty() {
            let names = conflicts.iter().map(|a| a.info().title).collect::<Vec<_>>().join(", ");
            body = body.child(div().pt(px(8.)).text_color(Theme::excluded()).child(format!("Already used by {names}; Enter takes it away from them.")));
        }
        body = body.child(
            div()
                .pt(px(10.))
                .text_size(ui_px(12.))
                .text_color(Theme::text_muted())
                .child("Up to two keystrokes (Ctrl+K, Ctrl+C). Enter applies, Esc cancels, a third keystroke starts over."),
        );
        Some(div().absolute().top_0().left_0().size_full().flex().justify_center().items_start().pt(px(120.)).occlude().child(body).into_any_element())
    }
}
