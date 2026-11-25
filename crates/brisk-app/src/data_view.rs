//! View as DataFrame / View as Array: a pandas DataFrame or Series or a numpy array of the
//! paused program in an editor tab. Rows come in pages of `PAGE` through debugpy `evaluate`
//! (`data_view.py` runs in the program), only for the pages on screen, so a 1M-row frame opens
//! as fast as a 10-row one. A filter (`df.eval` expression, or `x[:, 0] > 5` for arrays), a
//! sort by a column header click and a float format ("%.3f") are applied in the program too.
//! On every stop the view evaluates its expression again in the selected frame, as PyCharm's.

use crate::editor::Editor;
use crate::theme::{mono_font, ui_font, Theme};
use crate::ui_kit;
use brisk_dap::Client;
use gpui::prelude::*;
use gpui::{
    div, px, uniform_list, App, ClipboardItem, Context, Entity, FocusHandle, Focusable, Keystroke, ScrollHandle, ScrollStrategy, Task, UniformListScrollHandle,
    Window,
};
use serde_json::{json, Value};
use std::collections::{HashMap, HashSet};
use std::sync::Arc;
use std::time::Duration;

const ROW_H: f32 = 22.;
const CHAR_W: f32 = 7.0;
const PAGE: usize = 200;
const HELPER: &str = include_str!("data_view.py");

/// What the program reported about the object (from any page reply).
#[derive(Clone, Debug, PartialEq)]
pub struct Info {
    pub kind: String,
    pub shape: Vec<usize>,
    pub cols: Vec<String>,
    pub dtypes: Vec<String>,
    /// Rows after the filter.
    pub total: usize,
}

/// One page reply: the row labels and the cells.
#[derive(Clone, Debug, PartialEq)]
pub struct Page {
    pub index: Vec<String>,
    pub rows: Vec<Vec<String>>,
}

/// Parse an `evaluate` result of `_brisk_page`: a repr of a hex string of JSON.
pub fn parse_reply(result: &str) -> Result<(Info, usize, Page), String> {
    let hex = result.trim().trim_matches(|c| c == '\'' || c == '"');
    let bytes = decode_hex(hex).ok_or_else(|| short(result))?;
    let v: Value = serde_json::from_slice(&bytes).map_err(|e| e.to_string())?;
    if let Some(e) = v.get("error").and_then(Value::as_str) {
        return Err(e.to_string());
    }
    let strs = |k: &str| -> Vec<String> {
        v.get(k).and_then(Value::as_array).map(|a| a.iter().map(|x| x.as_str().unwrap_or("").to_string()).collect()).unwrap_or_default()
    };
    let info = Info {
        kind: v.get("kind").and_then(Value::as_str).unwrap_or("").to_string(),
        shape: v.get("shape").and_then(Value::as_array).map(|a| a.iter().filter_map(Value::as_u64).map(|n| n as usize).collect()).unwrap_or_default(),
        cols: strs("cols"),
        dtypes: strs("dtypes"),
        total: v.get("total").and_then(Value::as_u64).unwrap_or(0) as usize,
    };
    let a = v.get("a").and_then(Value::as_u64).unwrap_or(0) as usize;
    let rows = v
        .get("rows")
        .and_then(Value::as_array)
        .map(|rs| rs.iter().map(|r| r.as_array().map(|c| c.iter().map(|x| x.as_str().unwrap_or("").to_string()).collect()).unwrap_or_default()).collect())
        .unwrap_or_default();
    Ok((info, a, Page { index: strs("index"), rows }))
}

fn short(s: &str) -> String {
    let s: String = s.chars().take(300).collect();
    format!("Unexpected reply: {s}")
}

fn decode_hex(s: &str) -> Option<Vec<u8>> {
    if s.len() % 2 != 0 {
        return None;
    }
    (0..s.len()).step_by(2).map(|i| u8::from_str_radix(s.get(i..i + 2)?, 16).ok()).collect()
}

fn encode_hex(b: &[u8]) -> String {
    b.iter().map(|x| format!("{x:02x}")).collect()
}

/// The expression that returns rows `a..b` of `expr` (a Python expression of the frame).
pub fn page_expression(expr: &str, a: usize, b: usize, sort: Option<(usize, bool)>, filter: &str, format: &str, gen: u64) -> String {
    let lit = |s: &str| serde_json::to_string(s).unwrap_or_else(|_| "''".into());
    let (sort, asc) = match sort {
        Some((c, asc)) => (c.to_string(), if asc { "True" } else { "False" }),
        None => ("None".to_string(), "True"),
    };
    format!(
        "(lambda _o, _g={{}}: (exec(bytes.fromhex('{}').decode(), _g), _g['_brisk_page'](_o, {a}, {b}, {sort}, {asc}, {}, {}, {gen}))[1])(({expr}))",
        encode_hex(HELPER.as_bytes()),
        lit(filter),
        lit(format),
    )
}

/// The types View as DataFrame / Array opens (debugpy's `type` of a variable).
pub fn viewable(type_name: &str) -> Option<&'static str> {
    match type_name {
        "DataFrame" | "Series" => Some("View as DataFrame"),
        "ndarray" | "matrix" | "memmap" => Some("View as Array"),
        _ => None,
    }
}

pub struct DataView {
    pub title: String,
    pub expr: String,
    client: Arc<Client>,
    frame: Option<i64>,
    /// Bumped on every stop: the program caches sort/filter results per stop.
    stop: u64,
    /// Bumped on every change of sort, filter or format: late pages of the old query are dropped.
    query: u64,
    pub info: Option<Info>,
    pages: HashMap<usize, Page>,
    inflight: HashSet<usize>,
    pub sort: Option<(usize, bool)>,
    filter: Entity<Editor>,
    format: Entity<Editor>,
    applied: (String, String),
    pub error: Option<String>,
    /// The program runs (or the session ended): the rows shown are of the last stop.
    pub stale: Option<String>,
    sel: (usize, usize),
    widths: Vec<f32>,
    focus: FocusHandle,
    scroll: UniformListScrollHandle,
    hscroll: ScrollHandle,
    debounce: Option<Task<()>>,
}

fn field(cx: &mut Context<DataView>) -> Entity<Editor> {
    cx.new(|cx| {
        let mut e = Editor::scratch("", cx);
        e.buffer.auto_pairs = false;
        e.is_field = true;
        e
    })
}

impl DataView {
    pub fn new(expr: String, client: Arc<Client>, frame: Option<i64>, cx: &mut Context<Self>) -> Self {
        let filter = field(cx);
        let format = field(cx);
        for f in [&filter, &format] {
            cx.observe(f, |this: &mut Self, _, cx| this.fields_changed(cx)).detach();
        }
        let mut v = Self {
            title: expr.clone(),
            expr,
            client,
            frame,
            stop: 1,
            query: 0,
            info: None,
            pages: HashMap::new(),
            inflight: HashSet::new(),
            sort: None,
            filter,
            format,
            applied: (String::new(), String::new()),
            error: None,
            stale: None,
            sel: (0, 0),
            widths: Vec::new(),
            focus: cx.focus_handle(),
            scroll: UniformListScrollHandle::new(),
            hscroll: ScrollHandle::new(),
            debounce: None,
        };
        v.fetch(0, cx);
        v
    }

    /// A new stop (or another frame): evaluate the expression again there.
    pub fn refresh(&mut self, client: Arc<Client>, frame: Option<i64>, cx: &mut Context<Self>) {
        self.client = client;
        self.frame = frame;
        self.stop += 1;
        self.stale = None;
        self.reset(cx);
    }

    /// The program runs again: keep the rows, say they are of the last stop.
    pub fn set_stale(&mut self, why: &str, cx: &mut Context<Self>) {
        self.stale = Some(why.to_string());
        self.inflight.clear();
        cx.notify();
    }

    fn reset(&mut self, cx: &mut Context<Self>) {
        self.query += 1;
        self.pages.clear();
        self.inflight.clear();
        self.error = None;
        self.fetch(0, cx);
        cx.notify();
    }

    fn fields_changed(&mut self, cx: &mut Context<Self>) {
        let now = (self.filter.read(cx).text_string().trim().to_string(), self.format.read(cx).text_string().trim().to_string());
        if now == self.applied {
            return;
        }
        // Typing a filter: evaluate once the user pauses, not on every key.
        self.debounce = Some(cx.spawn(async move |this, cx| {
            cx.background_executor().timer(Duration::from_millis(350)).await;
            let _ = this.update(cx, |this, cx| {
                this.applied = now;
                this.reset(cx);
            });
        }));
    }

    pub fn set_sort(&mut self, col: usize, cx: &mut Context<Self>) {
        self.sort = match self.sort {
            Some((c, true)) if c == col => Some((col, false)),
            Some((c, false)) if c == col => None,
            _ => Some((col, true)),
        };
        self.reset(cx);
    }

    fn fetch(&mut self, page: usize, cx: &mut Context<Self>) {
        if self.stale.is_some() || self.inflight.contains(&page) || self.pages.contains_key(&page) {
            return;
        }
        self.inflight.insert(page);
        let (filter, format) = self.applied.clone();
        let expr = page_expression(&self.expr, page * PAGE, (page + 1) * PAGE, self.sort, &filter, &format, self.stop);
        let mut args = json!({"expression": expr, "context": "clipboard"});
        if let Some(f) = self.frame {
            args["frameId"] = json!(f);
        }
        let rx = self.client.request("evaluate", args);
        let query = self.query;
        cx.spawn(async move |this, cx| {
            let res = rx.await;
            let _ = this.update(cx, |this, cx| {
                if this.query != query {
                    return;
                }
                this.inflight.remove(&page);
                match res {
                    Ok(Ok(v)) => match parse_reply(v.get("result").and_then(Value::as_str).unwrap_or("")) {
                        Ok((info, _, p)) => {
                            if this.info.as_ref().map(|i| (&i.cols, &i.dtypes)) != Some((&info.cols, &info.dtypes)) {
                                this.widths = Vec::new();
                            }
                            this.info = Some(info);
                            this.error = None;
                            this.pages.insert(page, p);
                            this.measure();
                        }
                        Err(e) => this.error = Some(e),
                    },
                    Ok(Err(e)) => this.error = Some(e),
                    Err(_) => this.error = Some("The debug session ended".into()),
                }
                cx.notify();
            });
        })
        .detach();
    }

    /// Column widths from the header and the first loaded page.
    fn measure(&mut self) {
        let Some(info) = &self.info else { return };
        if !self.widths.is_empty() && self.widths.len() == info.cols.len() + 1 {
            return;
        }
        let page = self.pages.get(&0);
        let idx_w = page.map(|p| p.index.iter().map(|s| s.chars().count()).max().unwrap_or(1)).unwrap_or(4).max(3);
        let mut w = vec![(idx_w as f32 * CHAR_W + 18.).clamp(44., 220.)];
        for (c, name) in info.cols.iter().enumerate() {
            let mut n = name.chars().count() + 2;
            if let Some(p) = page {
                for r in p.rows.iter().take(100) {
                    n = n.max(r.get(c).map_or(0, |v| v.chars().count().min(60)));
                }
            }
            w.push((n as f32 * CHAR_W + 18.).clamp(56., 420.));
        }
        self.widths = w;
    }

    /// Smoke hooks: a row's cells (when its page came), and loading a row's page.
    #[cfg(feature = "smoke")]
    pub fn row_text(&self, r: usize) -> Option<(String, Vec<String>)> {
        let p = self.pages.get(&(r / PAGE))?;
        Some((p.index.get(r % PAGE)?.clone(), p.rows.get(r % PAGE)?.clone()))
    }

    #[cfg(feature = "smoke")]
    pub fn ensure_row(&mut self, r: usize, cx: &mut Context<Self>) {
        self.fetch(r / PAGE, cx);
    }

    #[cfg(feature = "smoke")]
    pub fn is_loading(&self) -> bool {
        !self.inflight.is_empty()
    }

    /// Filter and format as if typed (without the typing delay).
    #[cfg(feature = "smoke")]
    pub fn set_query(&mut self, filter: &str, format: &str, cx: &mut Context<Self>) {
        self.applied = (filter.to_string(), format.to_string());
        self.reset(cx);
    }

    fn cell(&self, r: usize, c: usize) -> Option<&String> {
        self.pages.get(&(r / PAGE)).and_then(|p| p.rows.get(r % PAGE)).and_then(|row| row.get(c))
    }

    pub fn copy(&self, cx: &mut App) -> bool {
        let Some(v) = self.cell(self.sel.0, self.sel.1) else { return false };
        cx.write_to_clipboard(ClipboardItem::new_string(v.clone()));
        true
    }

    fn key(&mut self, k: &Keystroke, cx: &mut Context<Self>) -> bool {
        if k.modifiers.control && k.key == "c" {
            return self.copy(cx);
        }
        let Some(info) = &self.info else { return false };
        let (nr, nc) = (info.total, info.cols.len());
        if nr == 0 || nc == 0 {
            return false;
        }
        let ctrl = k.modifiers.control;
        let (mut r, mut c) = self.sel;
        let page = 20;
        match k.key.as_str() {
            "up" => r = r.saturating_sub(1),
            "down" => r = (r + 1).min(nr - 1),
            "left" => c = c.saturating_sub(1),
            "right" | "tab" => c = (c + 1).min(nc - 1),
            "pageup" => r = r.saturating_sub(page),
            "pagedown" => r = (r + page).min(nr - 1),
            "home" if ctrl => (r, c) = (0, 0),
            "end" if ctrl => (r, c) = (nr - 1, nc - 1),
            "home" => c = 0,
            "end" => c = nc - 1,
            _ => return false,
        }
        self.sel = (r, c);
        self.scroll.scroll_to_item(r, ScrollStrategy::Top);
        cx.notify();
        true
    }

    fn render_bar(&self, window: &Window, cx: &mut Context<Self>) -> impl IntoElement {
        let label = |s: &'static str| div().text_size(px(12.)).text_color(Theme::text_muted()).child(s);
        let f_focus = self.filter.read(cx).focus_handle(cx).is_focused(window);
        let m_focus = self.format.read(cx).focus_handle(cx).is_focused(window);
        let summary = match (&self.info, &self.error) {
            (_, Some(e)) => e.clone(),
            (Some(i), None) => {
                let shape = i.shape.iter().map(|n| n.to_string()).collect::<Vec<_>>().join(" × ");
                let rows = i.shape.first().copied().unwrap_or(0);
                if i.total != rows {
                    format!("{} {shape}, {} rows match", i.kind, i.total)
                } else {
                    format!("{} {shape}", i.kind)
                }
            }
            (None, None) => "Loading…".into(),
        };
        let is_array = self.info.as_ref().is_some_and(|i| i.kind != "DataFrame" && i.kind != "Series");
        div()
            .h(px(36.))
            .flex_shrink_0()
            .flex()
            .items_center()
            .gap(px(8.))
            .px(px(8.))
            .border_b_1()
            .border_color(Theme::divider())
            .font_family(ui_font())
            .child(div().font_family(mono_font()).text_size(px(12.5)).text_color(Theme::text()).child(self.expr.clone()))
            .child(label("Filter"))
            .child(
                ui_kit::editor_field(
                    &self.filter,
                    if is_array { "x[:, 0] > 5" } else { "price > 10 and city == 'Kyiv'" },
                    f_focus,
                    false,
                    ui_kit::COMPACT_H,
                    cx,
                )
                .w(px(260.)),
            )
            .child(label("Format"))
            .child(ui_kit::editor_field(&self.format, "%.3f", m_focus, false, ui_kit::COMPACT_H, cx).w(px(90.)))
            .child(
                div()
                    .flex_1()
                    .min_w_0()
                    .overflow_hidden()
                    .whitespace_nowrap()
                    .text_ellipsis()
                    .text_size(px(12.))
                    .text_color(if self.error.is_some() { Theme::error() } else { Theme::text_muted() })
                    .child(summary),
            )
            .when_some(self.stale.clone(), |d, s| d.child(div().text_size(px(12.)).text_color(Theme::warning()).child(s)))
    }

    fn render_table(&self, info: &Info, cx: &mut Context<Self>) -> impl IntoElement {
        let total_w: f32 = self.widths.iter().sum();
        let widths = self.widths.clone();
        let sort = self.sort;
        let header = div()
            .h(px(ROW_H + 4.))
            .flex()
            .flex_shrink_0()
            .bg(Theme::panel_alt())
            .border_b_1()
            .border_color(Theme::divider())
            .child(div().w(px(widths.first().copied().unwrap_or(44.))).flex_shrink_0().border_r_1().border_color(Theme::divider()))
            .children(info.cols.iter().enumerate().map(|(c, name)| {
                let w = widths.get(c + 1).copied().unwrap_or(80.);
                let arrow = match sort {
                    Some((s, true)) if s == c => Some("▲"),
                    Some((s, false)) if s == c => Some("▼"),
                    _ => None,
                };
                let dtype = info.dtypes.get(c).cloned().unwrap_or_default();
                div()
                    .id(("dv-col", c))
                    .w(px(w))
                    .flex_shrink_0()
                    .px(px(6.))
                    .flex()
                    .items_center()
                    .gap(px(4.))
                    .overflow_hidden()
                    .whitespace_nowrap()
                    .border_r_1()
                    .border_color(Theme::divider())
                    .cursor_pointer()
                    .hover(|s| s.bg(Theme::hover_row()))
                    .tooltip(move |_, cx| cx.new(|_| crate::workspace::Tip(format!("{dtype} · click to sort").into())).into())
                    .child(div().min_w_0().overflow_hidden().text_ellipsis().font_weight(gpui::FontWeight::SEMIBOLD).child(name.clone()))
                    .when_some(arrow, |d, a| d.child(div().text_size(px(9.)).text_color(Theme::accent()).child(a)))
                    .on_click(cx.listener(move |this, _, window, cx| {
                        window.focus(&this.focus);
                        this.set_sort(c, cx);
                    }))
            }));
        let list = uniform_list(
            "dv-rows",
            info.total,
            cx.processor(move |this, range: std::ops::Range<usize>, window, cx| {
                let focused = this.focus.is_focused(window);
                let widths = this.widths.clone();
                let ncol = this.info.as_ref().map_or(0, |i| i.cols.len());
                let mut missing: Vec<usize> = Vec::new();
                let rows = range
                    .map(|r| {
                        let page = this.pages.get(&(r / PAGE));
                        if page.is_none() && !missing.contains(&(r / PAGE)) {
                            missing.push(r / PAGE);
                        }
                        let label = page.and_then(|p| p.index.get(r % PAGE)).cloned().unwrap_or_default();
                        let row = page.and_then(|p| p.rows.get(r % PAGE));
                        div()
                            .id(("dv-row", r))
                            .h(px(ROW_H))
                            .flex()
                            .when(r % 2 == 1, |d| d.bg(Theme::panel_alt()))
                            .child(
                                div()
                                    .w(px(widths.first().copied().unwrap_or(44.)))
                                    .flex_shrink_0()
                                    .px(px(6.))
                                    .flex()
                                    .items_center()
                                    .justify_end()
                                    .overflow_hidden()
                                    .whitespace_nowrap()
                                    .text_color(Theme::text_dim())
                                    .child(label),
                            )
                            .children((0..ncol).map(|c| {
                                let w = widths.get(c + 1).copied().unwrap_or(80.);
                                let selected = this.sel == (r, c);
                                let v = row.and_then(|row| row.get(c)).cloned();
                                let numeric = v.as_deref().is_some_and(|s| s.parse::<f64>().is_ok());
                                div()
                                    .id(("dv-cell", r * 4096 + c))
                                    .w(px(w))
                                    .flex_shrink_0()
                                    .px(px(6.))
                                    .flex()
                                    .items_center()
                                    .when(numeric, |d| d.justify_end())
                                    .overflow_hidden()
                                    .whitespace_nowrap()
                                    .border_r_1()
                                    .border_color(Theme::indent_guide())
                                    .when(selected, |d| d.bg(if focused { Theme::selection() } else { Theme::hover_row() }))
                                    .child(match v {
                                        Some(s) => div().child(s),
                                        None => div().text_color(Theme::text_dim()).child("…"),
                                    })
                                    .on_click(cx.listener(move |this, _, window, cx| {
                                        window.focus(&this.focus);
                                        this.sel = (r, c);
                                        cx.notify();
                                    }))
                            }))
                    })
                    .collect();
                for p in missing {
                    this.fetch(p, cx);
                }
                rows
            }),
        )
        .track_scroll(self.scroll.clone())
        .flex_1();
        div()
            .id("dv-hscroll")
            .flex_1()
            .min_h_0()
            .overflow_x_scroll()
            .track_scroll(&self.hscroll)
            .child(div().w(px(total_w.max(200.))).h_full().flex().flex_col().child(header).child(list))
    }
}

impl Focusable for DataView {
    fn focus_handle(&self, _: &App) -> FocusHandle {
        self.focus.clone()
    }
}

impl Render for DataView {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let info = self.info.clone();
        div()
            .id("data-view")
            .key_context("DataGrid")
            .track_focus(&self.focus)
            .size_full()
            .flex()
            .flex_col()
            .bg(Theme::editor_bg())
            .text_color(Theme::text())
            .on_key_down(cx.listener(|this, e: &gpui::KeyDownEvent, _, cx| {
                if this.key(&e.keystroke, cx) {
                    cx.stop_propagation();
                }
            }))
            .child(self.render_bar(window, cx))
            .child(
                div()
                    .flex_1()
                    .min_h_0()
                    .flex()
                    .flex_col()
                    .font_family(mono_font())
                    .text_size(px(12.5))
                    .when_some(info, |d, info| d.child(self.render_table(&info, cx))),
            )
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn reply_round_trip() {
        let json =
            r#"{"kind":"DataFrame","shape":[3,2],"cols":["a","b"],"dtypes":["int64","object"],"total":2,"a":0,"index":["0","2"],"rows":[["1","x"],["3","z"]]}"#;
        let repr = format!("'{}'", encode_hex(json.as_bytes()));
        let (info, a, page) = parse_reply(&repr).unwrap();
        assert_eq!(info.kind, "DataFrame");
        assert_eq!(info.shape, vec![3, 2]);
        assert_eq!(info.total, 2);
        assert_eq!(a, 0);
        assert_eq!(page.index, vec!["0", "2"]);
        assert_eq!(page.rows[1], vec!["3", "z"]);
        let err = format!("'{}'", encode_hex(br#"{"error":"NameError: name 'q' is not defined"}"#));
        assert_eq!(parse_reply(&err).unwrap_err(), "NameError: name 'q' is not defined");
        assert!(parse_reply("'zz'").is_err());
    }

    #[test]
    fn page_expression_quotes_its_arguments() {
        let e = page_expression("df[df.a > 1]", 200, 400, Some((2, false)), "city == \"Kyiv\"", "%.3f", 7);
        assert!(e.ends_with("((df[df.a > 1]))"));
        assert!(e.contains("_o, 200, 400, 2, False, \"city == \\\"Kyiv\\\"\", \"%.3f\", 7)"));
        assert_eq!(viewable("DataFrame"), Some("View as DataFrame"));
        assert_eq!(viewable("ndarray"), Some("View as Array"));
        assert_eq!(viewable("list"), None);
    }
}
