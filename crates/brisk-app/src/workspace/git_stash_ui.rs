//! Stash Changes… and Unstash… (filled in by the stash task).

use super::Workspace;
use gpui::{Context, KeyDownEvent, Window};

pub struct StashDialog;

impl Workspace {
    pub(super) fn git_more_action(&mut self, _action: brisk_keymap::Action, _window: &mut Window, _cx: &mut Context<Self>) {}

    pub(super) fn stash_dialog_key(&mut self, _e: &KeyDownEvent, _window: &mut Window, _cx: &mut Context<Self>) {}

    pub(super) fn render_stash_dialog(&mut self, _cx: &mut Context<Self>) -> Option<gpui::AnyElement> {
        None
    }
}
