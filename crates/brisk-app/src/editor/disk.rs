//! The editor and the file on disk: reading and saving in the file's own encoding, noticing
//! changes made by other programs (the terminal, git, a script), and never overwriting them
//! silently. Unmodified editors reload quietly (undoable); modified ones show a banner.

use super::Editor;
use crate::theme::Theme;
use brisk_text::encoding::{self, Decoded, TextEncoding};
use gpui::prelude::*;
use gpui::{div, px, Context};
use std::path::Path;
use std::time::SystemTime;

/// What we last saw on disk: modification time and size.
pub type Stamp = Option<(SystemTime, u64)>;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Conflict {
    /// Changed on disk while the editor has unsaved edits.
    Changed,
    /// Deleted on disk while the editor has unsaved edits.
    Deleted,
}

/// Result of a periodic check.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum DiskCheck {
    Same,
    /// Reloaded quietly (no local edits).
    Reloaded,
    /// The file is gone and there was nothing unsaved: the tab can close.
    Deleted,
    /// A banner is shown (changed or deleted under unsaved edits).
    Conflict,
}

/// Larger files are not opened as text (PyCharm's limit is similar).
pub const MAX_TEXT_BYTES: u64 = 20 * 1024 * 1024;

pub fn stamp(p: &Path) -> Stamp {
    let m = std::fs::metadata(p).ok()?;
    Some((m.modified().ok()?, m.len()))
}

/// Read a file: (text with LF line ends left as they are, encoding, error to show read-only).
pub fn load(p: &Path) -> (String, TextEncoding, Option<String>) {
    if let Ok(m) = std::fs::metadata(p) {
        if m.len() > MAX_TEXT_BYTES {
            let mb = m.len() as f64 / 1_048_576.0;
            return (String::new(), TextEncoding::default(), Some(format!("File too large to open as text ({mb:.1} MB)")));
        }
    }
    match std::fs::read(p) {
        Ok(bytes) => match encoding::decode(&bytes, encoding::system_legacy()) {
            Decoded::Text { text, encoding } => (text, encoding, None),
            Decoded::Binary => (String::new(), TextEncoding::default(), Some("Binary file: not shown as text".into())),
        },
        Err(e) => (String::new(), TextEncoding::default(), Some(e.to_string())),
    }
}

impl Editor {
    pub fn encoding_label(&self) -> String {
        self.encoding.label()
    }

    /// Write to disk if modified, in the file's encoding. A file changed or deleted on disk
    /// meanwhile is not overwritten: the banner asks first.
    pub fn save(&mut self) -> std::io::Result<bool> {
        if !self.buffer.is_dirty() || self.read_error.is_some() || self.disk_conflict.is_some() {
            return Ok(false);
        }
        let Some(p) = self.path.clone() else { return Ok(false) };
        let now = stamp(&p);
        if self.disk_stamp.is_some() && now != self.disk_stamp {
            self.disk_conflict = Some(if now.is_none() { Conflict::Deleted } else { Conflict::Changed });
            return Ok(false);
        }
        // .editorconfig: trailing whitespace (not on the caret line) and the final newline.
        if self.editorconfig.trim_trailing_whitespace.is_some() || self.editorconfig.insert_final_newline.is_some() {
            let old = self.buffer.text();
            let fixed = self.editorconfig.fix_on_save(&old, Some(self.caret_line_col().0));
            if fixed != old {
                self.replace_all_quiet(&fixed);
            }
        }
        let text = self.buffer.text_for_save();
        let bytes = match encoding::encode(&text, self.encoding) {
            Ok(b) => b,
            Err(e) => {
                // A character the legacy encoding lacks: keep the text, switch the file to UTF-8.
                self.encoding_note = Some(format!("{e}; saved as UTF-8"));
                self.encoding = TextEncoding::default();
                encoding::encode(&text, self.encoding).map_err(std::io::Error::other)?
            }
        };
        std::fs::write(&p, bytes)?;
        crate::local_history::record(&p, &self.buffer.text(), crate::local_history::Label::Saved);
        self.buffer.mark_saved();
        self.disk_stamp = stamp(&p);
        Ok(true)
    }

    /// Replace the buffer with the file's current contents (one undoable step).
    fn load_from_disk(&mut self) -> bool {
        let Some(p) = self.path.clone() else { return false };
        let (text, enc, err) = load(&p);
        if err.is_some() {
            return false;
        }
        self.encoding = enc;
        self.disk_stamp = stamp(&p);
        self.disk_conflict = None;
        let text = text.replace("\r\n", "\n");
        crate::local_history::record(&p, &text, crate::local_history::Label::External);
        if text == self.buffer.text() {
            self.buffer.mark_saved();
            return false;
        }
        let caret = self.buffer.primary().head;
        self.buffer.set_text(&text);
        self.buffer.set_caret(caret.min(self.buffer.len_chars()), false);
        self.buffer.mark_saved();
        true
    }

    /// Pick up changes made outside the editor. Unsaved edits always win (a banner asks).
    pub fn reload_from_disk(&mut self) -> bool {
        matches!(self.disk_check(), DiskCheck::Reloaded)
    }

    /// Compare with the disk and react; cheap (one metadata call) when nothing changed.
    pub fn disk_check(&mut self) -> DiskCheck {
        let Some(p) = self.path.clone() else { return DiskCheck::Same };
        if self.read_error.is_some() {
            return DiskCheck::Same;
        }
        let now = stamp(&p);
        if now == self.disk_stamp {
            return DiskCheck::Same;
        }
        match (now, self.buffer.is_dirty()) {
            (None, false) => DiskCheck::Deleted,
            (None, true) => {
                self.disk_conflict = Some(Conflict::Deleted);
                DiskCheck::Conflict
            }
            (Some(_), false) => {
                if self.load_from_disk() {
                    DiskCheck::Reloaded
                } else {
                    DiskCheck::Same
                }
            }
            (Some(_), true) => {
                if self.disk_conflict != Some(Conflict::Changed) {
                    self.disk_conflict = Some(Conflict::Changed);
                    return DiskCheck::Conflict;
                }
                DiskCheck::Same
            }
        }
    }

    /// Banner buttons: take the disk version, or keep the editor's (saved over the file next).
    pub fn resolve_conflict(&mut self, take_disk: bool, cx: &mut Context<Self>) {
        let Some(p) = self.path.clone() else { return };
        if take_disk && p.exists() {
            self.load_from_disk();
        } else {
            self.disk_stamp = stamp(&p);
            self.disk_conflict = None;
            let _ = self.save();
        }
        cx.notify();
    }

    pub(super) fn render_disk_banner(&self, cx: &mut Context<Self>) -> Option<gpui::AnyElement> {
        let c = self.disk_conflict?;
        let (msg, keep) = match c {
            Conflict::Changed => ("The file was changed on disk, and this editor has unsaved changes.", "Keep Mine"),
            Conflict::Deleted => ("The file was deleted on disk. Your text is still here.", "Save Again"),
        };
        let button = |id: &'static str, label: &'static str, take_disk: bool, cx: &mut Context<Self>| {
            div()
                .id(id)
                .px(px(10.))
                .h(px(24.))
                .flex()
                .items_center()
                .rounded(px(4.))
                .border_1()
                .border_color(Theme::divider())
                .cursor_pointer()
                .hover(|s| s.bg(Theme::hover_row()))
                .on_click(cx.listener(move |this, _, _, cx| this.resolve_conflict(take_disk, cx)))
                .child(label)
        };
        Some(
            div()
                .id("disk-banner")
                .flex()
                .items_center()
                .gap(px(8.))
                .px(px(12.))
                .py(px(6.))
                .bg(Theme::warning_bg())
                .border_b_1()
                .border_color(Theme::divider())
                .text_size(px(13.))
                .text_color(Theme::text())
                .cursor(gpui::CursorStyle::Arrow)
                .child(div().text_color(Theme::warning()).child("⚠"))
                .child(div().flex_1().child(msg))
                .when(c == Conflict::Changed, |d| d.child(button("disk-reload", "Reload from Disk", true, cx)))
                .child(button("disk-keep", keep, false, cx))
                .into_any_element(),
        )
    }
}
