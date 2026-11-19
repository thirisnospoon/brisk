//! Resizing tool windows, as in PyCharm: the gap between an island and the editor (a 1 px
//! line in Classic) is a splitter, invisible until hovered (a resize cursor), dragged to set
//! the tool window's width or height. Sizes are kept per project, the bottom ones per tool
//! window.

use super::{BottomTool, ToolSide, Workspace};
use crate::theme::Theme;
use gpui::{div, px, prelude::*, Context, CursorStyle, DragMoveEvent, MouseButton, MouseDownEvent, Window};
use std::collections::HashMap;
use std::path::{Path, PathBuf};

/// The smallest tool window, and how much of the window the editor keeps.
const MIN_SIDE: f32 = 160.;
const MIN_BOTTOM: f32 = 90.;
const KEEP_W: f32 = 360.;
const KEEP_H: f32 = 160.;

#[derive(Default)]
pub(super) struct Splits {
    loaded: bool,
    left: Option<f32>,
    right: Option<f32>,
    /// Bottom tool windows' heights by title.
    bottom: HashMap<String, f32>,
    /// A drag in progress: the side, where it started, the size then.
    drag: Option<(ToolSide, gpui::Point<gpui::Pixels>, f32)>,
}

/// What a splitter drag carries; nothing follows the pointer.
#[derive(Clone, Copy)]
pub(super) struct SplitDrag;

impl gpui::Render for SplitDrag {
    fn render(&mut self, _: &mut Window, _: &mut Context<Self>) -> impl IntoElement {
        div()
    }
}

fn store_path() -> PathBuf {
    brisk_db::config_dir().join("layout.json")
}

fn root_key(root: &Path) -> String {
    root.to_string_lossy().to_lowercase()
}

impl Workspace {
    fn load_splits(&mut self) {
        if self.splits.loaded {
            return;
        }
        self.splits.loaded = true;
        let v = std::fs::read_to_string(store_path()).ok().and_then(|s| serde_json::from_str::<serde_json::Value>(&s).ok());
        let Some(v) = v.as_ref().and_then(|v| v.get(root_key(&self.root))) else { return };
        let num = |k: &str| v.get(k).and_then(|x| x.as_f64()).map(|x| x as f32);
        self.splits.left = num("left");
        self.splits.right = num("right");
        if let Some(b) = v.get("bottom").and_then(|b| b.as_object()) {
            self.splits.bottom = b.iter().filter_map(|(k, x)| Some((k.clone(), x.as_f64()? as f32))).collect();
        }
    }

    fn save_splits(&self) {
        let path = store_path();
        let mut all: serde_json::Map<String, serde_json::Value> = std::fs::read_to_string(&path)
            .ok()
            .and_then(|s| serde_json::from_str::<serde_json::Value>(&s).ok())
            .and_then(|v| v.as_object().cloned())
            .unwrap_or_default();
        let s = &self.splits;
        all.insert(root_key(&self.root), serde_json::json!({ "left": s.left, "right": s.right, "bottom": s.bottom }));
        if let Some(d) = path.parent() {
            let _ = std::fs::create_dir_all(d);
        }
        if let Ok(text) = serde_json::to_string_pretty(&serde_json::Value::Object(all)) {
            let _ = std::fs::write(path, text);
        }
    }

    /// The left tool window's width (300 until dragged).
    pub(super) fn left_width(&mut self) -> f32 {
        self.load_splits();
        self.splits.left.unwrap_or(300.)
    }

    /// The right tool window's width (320 until dragged).
    pub(super) fn right_width(&mut self) -> f32 {
        self.load_splits();
        self.splits.right.unwrap_or(320.)
    }

    /// The height of bottom tool window `b` (its own default until dragged).
    pub(super) fn bottom_height(&mut self, b: BottomTool) -> f32 {
        self.load_splits();
        let default = match b {
            BottomTool::Git => 320.,
            BottomTool::Services => 280.,
            BottomTool::Debug | BottomTool::PythonConsole => 300.,
            _ => 240.,
        };
        self.splits.bottom.get(b.title()).copied().unwrap_or(default)
    }

    fn split_size(&mut self, side: ToolSide) -> f32 {
        match side {
            ToolSide::Left => self.left_width(),
            ToolSide::Right => self.right_width(),
            ToolSide::Bottom => match self.bottom {
                Some(b) => self.bottom_height(b),
                None => 240.,
            },
        }
    }

    /// The splitter between the tool window on `side` and the editor: the island gap (a
    /// 1 px line in Classic, grabbed 3 px around it).
    pub(super) fn splitter(&self, side: ToolSide, cx: &mut Context<Self>) -> gpui::AnyElement {
        let islands = crate::theme::islands();
        let across = side == ToolSide::Bottom;
        let (id, line) = match side {
            ToolSide::Left => ("split-left", Theme::border()),
            ToolSide::Right => ("split-right", Theme::border()),
            ToolSide::Bottom => ("split-bottom", Theme::divider()),
        };
        let thick = if islands { crate::ui_kit::ISLAND_GAP } else { 1. };
        let grab = div()
            .id(id)
            .absolute()
            .cursor(if across { CursorStyle::ResizeUpDown } else { CursorStyle::ResizeLeftRight })
            .map(|d| match (islands, across) {
                (true, _) => d.top_0().left_0().size_full(),
                (false, true) => d.left_0().w_full().top(px(-3.)).h(px(7.)),
                (false, false) => d.top_0().h_full().left(px(-3.)).w(px(7.)),
            })
            .on_mouse_down(MouseButton::Left, cx.listener(move |this, e: &MouseDownEvent, _, cx| {
                let size = this.split_size(side);
                this.splits.drag = Some((side, e.position, size));
                cx.stop_propagation();
            }))
            .on_drag(SplitDrag, |d, _, _, cx| cx.new(|_| *d));
        div()
            .relative()
            .flex_none()
            .map(|d| if across { d.w_full().h(px(thick)) } else { d.h_full().w(px(thick)) })
            .when(!islands, |d| d.bg(line))
            .child(grab)
            .into_any_element()
    }

    /// The pointer moved during a splitter drag: the new size, kept within the window.
    pub(super) fn split_drag_move(&mut self, e: &DragMoveEvent<SplitDrag>, window: &mut Window, cx: &mut Context<Self>) {
        let Some((side, start, size)) = self.splits.drag else { return };
        let (p, vp) = (e.event.position, window.viewport_size());
        let (v, max) = match side {
            ToolSide::Left => (size + f32::from(p.x - start.x), f32::from(vp.width) - KEEP_W),
            ToolSide::Right => (size - f32::from(p.x - start.x), f32::from(vp.width) - KEEP_W),
            ToolSide::Bottom => (size - f32::from(p.y - start.y), f32::from(vp.height) - KEEP_H),
        };
        let min = if side == ToolSide::Bottom { MIN_BOTTOM } else { MIN_SIDE };
        let v = v.clamp(min, max.max(min)).round();
        match side {
            ToolSide::Left => self.splits.left = Some(v),
            ToolSide::Right => self.splits.right = Some(v),
            ToolSide::Bottom => {
                if let Some(b) = self.bottom {
                    self.splits.bottom.insert(b.title().to_string(), v);
                }
            }
        }
        cx.notify();
    }

    /// The button went up: a splitter drag ends and its size is saved.
    pub(super) fn split_drag_end(&mut self) {
        if self.splits.drag.take().is_some() {
            self.save_splits();
        }
    }

    /// While a splitter is dragged the resize cursor stays, wherever the pointer is.
    pub(super) fn split_cursor(&self) -> Option<gpui::AnyElement> {
        let (side, _, _) = self.splits.drag?;
        let style = if side == ToolSide::Bottom { CursorStyle::ResizeUpDown } else { CursorStyle::ResizeLeftRight };
        Some(gpui::canvas(|_, _, _| {}, move |_, _, window, _| window.set_window_cursor_style(style)).absolute().size_full().into_any_element())
    }
}
