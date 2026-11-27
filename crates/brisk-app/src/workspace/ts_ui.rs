//! TypeScript 7's language server for JS and TS files: started on the first such file, from
//! the project's `node_modules` or Brisk's own download. Without either, a balloon offers
//! the download (about 10 MB); declining keeps Brisk's own JS / TS support.

use super::lsp_ui::TyState;
use super::Workspace;
use crate::theme::{ui_px, Theme};
use brisk_lsp::{typescript, Ty};
use gpui::prelude::*;
use gpui::{div, px, Context};
use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::sync::Arc;

pub(super) struct TsLsp {
    pub server: Option<Arc<Ty>>,
    pub state: TyState,
    /// Buffer version the server last saw for each open file.
    pub synced: HashMap<PathBuf, u64>,
    /// The download balloon is showing.
    pub offer: bool,
    /// Declined in this window: not offered again.
    pub declined: bool,
    pub downloading: bool,
    /// The last code-insight request was in a JS / TS file (for "needs …" messages).
    pub last: bool,
}

impl Default for TsLsp {
    fn default() -> Self {
        TsLsp { server: None, state: TyState::Unknown, synced: HashMap::new(), offer: false, declined: false, downloading: false, last: false }
    }
}

/// A file TypeScript's server handles.
pub(super) fn is_ts_path(path: &Path) -> bool {
    path.extension().and_then(|e| e.to_str()).is_some_and(|e| {
        let e = e.to_ascii_lowercase();
        matches!(e.as_str(), "ts" | "tsx" | "mts" | "cts" | "js" | "jsx" | "mjs" | "cjs") && !path.to_string_lossy().ends_with(".min.js")
    })
}

impl Workspace {
    /// Start the server once a JS / TS file needs it. Cheap to call often.
    pub(super) fn ensure_ts(&mut self, cx: &mut Context<Self>) {
        if let Some(s) = &self.lsp.ts.server {
            if !s.is_alive() {
                self.lsp.ts.server = None;
                self.lsp.ts.synced.clear();
                self.lsp.ts.state = TyState::Failed("TypeScript exited".into());
                self.status = "TypeScript language server exited; reopen the project to restart it".into();
            }
            return;
        }
        if self.lsp.ts.state != TyState::Unknown {
            return;
        }
        let Some(tsc) = typescript::find(&self.root) else {
            self.lsp.ts.state = TyState::Missing;
            if !self.lsp.ts.declined && std::env::var_os("BRISK_TSC").is_none() {
                self.lsp.ts.offer = true;
                cx.notify();
            }
            return;
        };
        self.lsp.ts.state = TyState::Starting;
        let root = self.root.clone();
        let task = cx.background_executor().spawn(async move { Ty::start_typescript(&tsc, &root) });
        cx.spawn(async move |this, cx| {
            let res = task.await;
            let _ = this.update(cx, |this, cx| {
                match res {
                    Ok(s) => {
                        this.lsp.ts.server = Some(Arc::new(s));
                        this.lsp.ts.state = TyState::Ready;
                        this.relint_all(cx);
                    }
                    Err(e) => {
                        this.status = format!("TypeScript: {e}");
                        this.lsp.ts.state = TyState::Failed(e);
                    }
                }
                cx.notify();
            });
        })
        .detach();
    }

    fn ts_download(&mut self, cx: &mut Context<Self>) {
        self.lsp.ts.offer = false;
        self.lsp.ts.downloading = true;
        self.status = "Downloading TypeScript…".into();
        let task = cx.background_executor().spawn(async move { typescript::download() });
        cx.spawn(async move |this, cx| {
            let res = task.await;
            let _ = this.update(cx, |this, cx| {
                this.lsp.ts.downloading = false;
                match res {
                    Ok(_) => {
                        this.status = "TypeScript downloaded".into();
                        this.lsp.ts.state = TyState::Unknown;
                        this.ensure_ts(cx);
                    }
                    Err(e) => {
                        this.show_toast(format!("Cannot download TypeScript: {e}"), true, cx);
                        this.lsp.ts.state = TyState::Failed(e);
                    }
                }
                cx.notify();
            });
        })
        .detach();
    }

    pub(super) fn render_ts_offer(&self, cx: &mut Context<Self>) -> Option<gpui::AnyElement> {
        if !self.lsp.ts.offer {
            return None;
        }
        let button = |id: &'static str, text: &'static str, primary: bool| {
            div()
                .id(id)
                .h(ui_px(24.))
                .px(px(10.))
                .flex()
                .items_center()
                .rounded(px(5.))
                .cursor_pointer()
                .when(primary, |d| d.bg(Theme::accent()).text_color(gpui::white()))
                .when(!primary, |d| d.border_1().border_color(Theme::border()))
                .map(|d| crate::ui_kit::button_states(d, primary))
                .child(text)
        };
        Some(
            div()
                .id("ts-offer")
                .occlude()
                .absolute()
                .bottom(ui_px(28.) + px(60.))
                .right(super::toast_right())
                .w(ui_px(420.))
                .p(px(12.))
                .flex()
                .flex_col()
                .gap(px(8.))
                .rounded(px(8.))
                .bg(Theme::panel())
                .border_1()
                .border_color(Theme::divider())
                .map(crate::ui_kit::floating)
                .text_size(ui_px(13.))
                .text_color(Theme::text())
                .child(div().font_weight(gpui::FontWeight::SEMIBOLD).child("Code insight for JavaScript and TypeScript"))
                .child(
                    div()
                        .text_size(ui_px(12.5))
                        .text_color(Theme::text_muted())
                        .child("Download TypeScript 7 (about 10 MB) for type errors, completion, navigation and rename in JS and TS files?"),
                )
                .child(
                    div()
                        .flex()
                        .justify_end()
                        .gap(px(8.))
                        .child(button("ts-offer-no", "Not Now", false).on_click(cx.listener(|this, _, _, cx| {
                            this.lsp.ts.offer = false;
                            this.lsp.ts.declined = true;
                            cx.notify();
                        })))
                        .child(button("ts-offer-yes", "Download", true).on_click(cx.listener(|this, _, _, cx| this.ts_download(cx)))),
                )
                .into_any_element(),
        )
    }
}
