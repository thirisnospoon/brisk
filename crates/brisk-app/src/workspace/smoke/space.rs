//! `BRISK_SMOKE_SPACE=1` (on an empty folder): Space typed with real key presses (posted
//! WM_KEYDOWN / WM_CHAR / WM_KEYUP, as a keyboard sends them) reaches the views that read keys
//! themselves. gpui gives Space no text on Windows, so these used to drop it. Checks the Search
//! Everywhere field and a Terminal tab (`echo 12 34` must print `12 34`). Only digits are typed
//! with the keyboard: letters would come out in whatever layout is active. Exits with code 1 on
//! the first failed check.

use super::win_input::type_keys;
use crate::workspace::Workspace;
use brisk_keymap::Action;
use gpui::{Context, Window};
use std::time::Duration;

fn check(ok: bool, what: &str, got: impl std::fmt::Debug) {
    if ok {
        eprintln!("smoke-space: ok {what}");
    } else {
        eprintln!("smoke-space: FAIL {what}: {got:?}");
        std::process::exit(1);
    }
}

impl Workspace {
    pub(in crate::workspace) fn space_smoke(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        cx.spawn_in(window, async move |this, cx| {
            let bg = cx.background_executor().clone();
            bg.timer(Duration::from_millis(1500)).await;

            // A popup field.
            let _ = this.update_in(cx, |this, window, cx| this.dispatch(Action::SearchEverywhere, window, cx));
            bg.timer(Duration::from_millis(300)).await;
            type_keys("12 34");
            bg.timer(Duration::from_millis(500)).await;
            let query = this.update(cx, |this, _| this.popup.as_ref().map(|p| p.query.clone())).ok().flatten();
            check(query.as_deref() == Some("12 34"), "popup field", &query);
            let _ = this.update_in(cx, |this, window, cx| {
                this.popup = None;
                this.focus_after_popup(window, cx);
            });

            // A shell in the Terminal: `echo '12 34'`, with `12 34` typed and the rest written
            // for it (quoted, so PowerShell prints one line, as cmd and bash do).
            let _ = this.update_in(cx, |this, window, cx| this.dispatch(Action::ToggleTerminalWindow, window, cx));
            bg.timer(Duration::from_millis(3000)).await;
            let write = |this: &mut Workspace, text: &str, cx: &mut gpui::Context<Workspace>| {
                if let Some(t) = &this.terminal {
                    t.read(cx).type_text(text);
                }
            };
            let _ = this.update(cx, |this, cx| write(this, "echo '", cx));
            type_keys("12 34");
            bg.timer(Duration::from_millis(300)).await;
            let _ = this.update(cx, |this, cx| write(this, "'\r", cx));
            let printed = |text: &str| text.lines().any(|l| l.trim() == "12 34");
            let mut text = String::new();
            for _ in 0..50 {
                bg.timer(Duration::from_millis(100)).await;
                text = this.update(cx, |this, cx| this.terminal.as_ref().and_then(|t| t.read(cx).active_text())).ok().flatten().unwrap_or_default();
                if printed(&text) {
                    break;
                }
            }
            check(printed(&text), "terminal", &text);
            eprintln!("smoke-space: PASS");
            std::process::exit(0);
        })
        .detach();
    }
}
