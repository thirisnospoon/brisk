//! Find in Files (Ctrl+Shift+F) / Replace in Files (Ctrl+Shift+R) popup and the Find tool window
//! (Alt+3, or Ctrl+Enter from the popup). Searches run in the background, restart on every
//! keystroke (debounced), and are cancelled when superseded.

use super::*;
use brisk_search::{Match, Options, Query, Results};
use std::sync::atomic::{AtomicBool, Ordering};
use std::time::Duration;

mod tree;

const LIMIT: usize = 10_000;
const ROW_H: f32 = 24.0;
const PREVIEW_CONTEXT: usize = 4;

#[derive(Clone, Copy, PartialEq, Eq)]
enum Field {
    Query,
    Replace,
    Mask,
}

pub struct FindWindow {
    title: String,
    files: Arc<FileList>,
    results: Arc<Results>,
    rows: Vec<WRow>,
    selected: usize,
    scroll: UniformListScrollHandle,
    pub focus: FocusHandle,
    /// Find Usages: each match's index in `tree::USAGE_GROUPS`; empty for a text search.
    group_of: Vec<u8>,
    /// Folded nodes: `(group, file)`, `(group, usize::MAX)` for a whole group.
    collapsed: std::collections::HashSet<(u8, usize)>,
}

#[derive(Clone, Copy, PartialEq, Eq)]
enum WRow {
    /// Usage group, number of usages.
    Group(u8, usize),
    /// Group, file, number of matches.
    File(u8, usize, usize),
    Match(usize),
}

pub struct Find {
    pub open: bool,
    replace_mode: bool,
    opts: Options,
    replace: String,
    mask_on: bool,
    field: Field,
    /// Prefilled text is "selected": the first typed char replaces it.
    seed_selected: bool,
    files: Option<Arc<FileList>>,
    results: Arc<Results>,
    selected: usize,
    error: Option<String>,
    searching: bool,
    took: Duration,
    gen: u64,
    cancel: Arc<AtomicBool>,
    scroll: UniformListScrollHandle,
    preview: Option<(usize, Arc<Vec<String>>)>,
    confirm_replace: bool,
    pub window: Option<FindWindow>,
}

impl FindWindow {
    pub fn rows_len(&self) -> usize {
        self.rows.len()
    }
}

impl Find {
    pub fn new() -> Self {
        Self {
            open: false,
            replace_mode: false,
            opts: Options::default(),
            replace: String::new(),
            mask_on: false,
            field: Field::Query,
            seed_selected: false,
            files: None,
            results: Arc::new(Results::default()),
            selected: 0,
            error: None,
            searching: false,
            took: Duration::ZERO,
            gen: 0,
            cancel: Arc::new(AtomicBool::new(false)),
            scroll: UniformListScrollHandle::new(),
            preview: None,
            confirm_replace: false,
            window: None,
        }
    }

    fn effective_opts(&self) -> Options {
        let mut o = self.opts.clone();
        if !self.mask_on {
            o.file_mask.clear();
        }
        o
    }

    fn field_mut(&mut self) -> &mut String {
        match self.field {
            Field::Query => &mut self.opts.query,
            Field::Replace => &mut self.replace,
            Field::Mask => &mut self.opts.file_mask,
        }
    }

    pub fn set_query_for_smoke(&mut self, q: &str) {
        self.opts.query = q.to_string();
    }

    pub fn idle(&self) -> bool {
        !self.searching && self.error.is_none()
    }

    pub fn summary(&self) -> String {
        format!(
            "{} matches in {} files (searched {}), {:.1?}, error={:?}",
            self.results.matches.len(),
            self.results.files_with_matches,
            self.results.files_searched,
            self.took,
            self.error
        )
    }

    fn rel(&self, m: &Match) -> &str {
        self.files.as_ref().map(|f| f.files[m.file].as_str()).unwrap_or("")
    }
}

fn split_rel(rel: &str) -> (&str, &str) {
    match rel.rsplit_once('/') {
        Some((d, n)) => (n, d),
        None => (rel, ""),
    }
}

/// A result line: leading whitespace trimmed, the match highlighted.
fn match_line(m: &Match) -> gpui::Div {
    let trimmed = m.text.len() - m.text.trim_start().len();
    let cut = trimmed.min(m.hl.start);
    let t = &m.text[cut..];
    let e = m.hl.end.saturating_sub(cut).min(t.len());
    let s = (m.hl.start - cut).min(e);
    let (s, e) = if t.is_char_boundary(s) && t.is_char_boundary(e) { (s, e) } else { (0, 0) };
    div()
        .flex()
        .min_w_0()
        .overflow_hidden()
        .whitespace_nowrap()
        .child(t[..s].to_string())
        .child(div().bg(Theme::search_hit()).rounded(px(2.)).child(t[s..e].to_string()))
        .child(t[e..].to_string())
}

impl Workspace {
    pub(super) fn open_find(&mut self, replace: bool, window: &mut Window, cx: &mut Context<Self>) {
        if self.find.open && self.find.replace_mode != replace {
            self.find.replace_mode = replace;
            self.find.field = if replace { Field::Replace } else { Field::Query };
            cx.notify();
            return;
        }
        self.save_all(cx); // search what the user sees
        let seed = self.focused_editor(window, cx).and_then(|e| e.read(cx).search_seed());
        let f = &mut self.find;
        f.open = true;
        f.replace_mode = replace;
        f.field = Field::Query;
        f.confirm_replace = false;
        if let Some(s) = seed {
            f.opts.query = s;
        }
        f.seed_selected = !f.opts.query.is_empty();
        window.focus(&self.focus);
        self.schedule_search(cx);
        cx.notify();
    }

    fn close_find(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        self.find.open = false;
        self.find.cancel.store(true, Ordering::Relaxed);
        self.focus_editor(window, cx);
        cx.notify();
    }

    pub(super) fn schedule_search(&mut self, cx: &mut Context<Self>) {
        let f = &mut self.find;
        f.cancel.store(true, Ordering::Relaxed);
        f.gen += 1;
        f.confirm_replace = false;
        let gen = f.gen;
        if f.opts.query.is_empty() {
            f.results = Arc::new(Results::default());
            f.error = None;
            f.searching = false;
            f.preview = None;
            return;
        }
        let Some(files) = self.files.clone() else {
            f.error = Some("Indexing…".into());
            return;
        };
        let cancel = Arc::new(AtomicBool::new(false));
        f.cancel = cancel.clone();
        f.searching = true;
        let opts = f.effective_opts();
        let timer = cx.background_executor().timer(Duration::from_millis(90));
        let bg = cx.background_executor().clone();
        cx.spawn(async move |this, cx| {
            timer.await;
            if cancel.load(Ordering::Relaxed) {
                return;
            }
            let q = match Query::new(&opts) {
                Ok(q) => q,
                Err(e) => {
                    let _ = this.update(cx, |this, cx| {
                        if this.find.gen == gen {
                            this.find.error = Some(e);
                            this.find.searching = false;
                            cx.notify();
                        }
                    });
                    return;
                }
            };
            let started = Instant::now();
            let (f2, c2) = (files.clone(), cancel.clone());
            let res = bg.spawn(async move { brisk_search::search(&f2.root, &f2.files, &q, LIMIT, &c2) }).await;
            if cancel.load(Ordering::Relaxed) {
                return;
            }
            let took = started.elapsed();
            let _ = this.update(cx, |this, cx| {
                if this.find.gen != gen {
                    return;
                }
                crate::stall::note("find: apply results");
                let f = &mut this.find;
                f.results = Arc::new(res);
                f.files = Some(files);
                f.error = None;
                f.searching = false;
                f.took = took;
                f.selected = 0;
                f.scroll.scroll_to_item(0, gpui::ScrollStrategy::Top);
                this.load_preview();
                cx.notify();
            });
        })
        .detach();
    }

    fn load_preview(&mut self) {
        let f = &mut self.find;
        let Some(m) = f.results.matches.get(f.selected) else {
            f.preview = None;
            return;
        };
        if f.preview.as_ref().is_some_and(|(i, _)| *i == m.file) {
            return;
        }
        let Some(files) = &f.files else { return };
        let text = brisk_search::read_text(&files.root.join(&files.files[m.file])).unwrap_or_default();
        f.preview = Some((m.file, Arc::new(text.lines().map(|l| l.replace('\t', "    ")).collect())));
    }

    fn open_match(&mut self, files: &Arc<FileList>, m: &Match, window: &mut Window, cx: &mut Context<Self>) {
        let path = files.root.join(&files.files[m.file]);
        self.open_file(&path, window, cx);
        if let Some(ed) = self.active_editor().cloned() {
            let (line, col) = (m.line, m.col.clone());
            ed.update(cx, |e, cx| e.select_on_line(line, col, cx));
        }
    }

    fn open_in_find_window(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let f = &self.find;
        let Some(files) = f.files.clone() else { return };
        if f.results.matches.is_empty() {
            return;
        }
        let results = f.results.clone();
        let title = format!("“{}” in Project", f.opts.query);
        let focus = cx.focus_handle();
        let mut w = FindWindow { title, files, results, rows: Vec::new(), selected: 0, scroll: UniformListScrollHandle::new(), focus: focus.clone(), group_of: Vec::new(), collapsed: Default::default() };
        w.rebuild_rows();
        w.selected = w.rows.iter().position(|r| matches!(r, WRow::Match(_))).unwrap_or(0);
        self.find.window = Some(w);
        self.find.open = false;
        self.bottom = Some(BottomTool::Find);
        window.focus(&focus);
        cx.notify();
    }

    /// Find Usages results in the Find tool window: `(file, line, char columns)`.
    /// Line text comes from the open editor when the file is open (unsaved edits), else disk.
    pub(super) fn show_usages(&mut self, title: String, mut usages: Vec<(PathBuf, usize, std::ops::Range<usize>)>, window: &mut Window, cx: &mut Context<Self>) -> usize {
        usages.sort_by(|a, b| (&a.0, a.1, a.2.start).cmp(&(&b.0, b.1, b.2.start)));
        usages.dedup();
        let truncated = usages.len() > LIMIT;
        usages.truncate(LIMIT);
        let mut files: Vec<String> = Vec::new();
        let mut matches = Vec::new();
        let mut groups = Vec::new();
        let mut cache: Option<(PathBuf, Vec<String>)> = None;
        for (path, line, cols) in usages {
            if cache.as_ref().map(|c| &c.0) != Some(&path) {
                let open = self.editors().find(|e| e.read(cx).path.as_deref() == Some(path.as_path())).map(|e| e.read(cx).text_string());
                let text = open.or_else(|| brisk_search::read_text(&path)).unwrap_or_default();
                cache = Some((path.clone(), text.lines().map(|l| l.replace('\t', "    ")).collect()));
            }
            let text = cache.as_ref().and_then(|c| c.1.get(line).cloned()).unwrap_or_default();
            let byte = |c: usize| text.char_indices().nth(c).map(|(b, _)| b).unwrap_or(text.len());
            // As in PyCharm, the definition itself (`def add`, `class A`) is not a usage.
            let before: String = text.chars().take(cols.start).collect();
            let before = before.trim();
            if before == "def" || before == "class" || before.ends_with(" def") {
                continue;
            }
            let rel = match path.strip_prefix(&self.root) {
                Ok(r) => r.to_string_lossy().replace('\\', "/"),
                Err(_) => path.to_string_lossy().into_owned(),
            };
            if files.last() != Some(&rel) {
                files.push(rel);
            }
            let hl = byte(cols.start)..byte(cols.end).max(byte(cols.start));
            groups.push(tree::usage_group(&text, cols.clone()));
            matches.push(Match { file: files.len() - 1, line, col: cols, text, hl });
        }
        let n_files = files.len();
        let n = matches.len();
        if n == 0 {
            return 0;
        }
        let list = Arc::new(FileList { root: self.root.clone(), files });
        let results = Results { matches, files_with_matches: n_files, files_searched: n_files, truncated };
        self.find.files = Some(list.clone());
        self.find.results = Arc::new(results);
        self.find.opts.query = String::new();
        self.open_in_find_window(window, cx);
        if let Some(w) = self.find.window.as_mut() {
            w.title = title;
            w.group_of = groups;
            w.rebuild_rows();
            w.selected = w.rows.iter().position(|r| matches!(r, WRow::Match(_))).unwrap_or(0);
        }
        n
    }

    /// The Find window's tree as text and a key pressed in it, for smokes.
    pub(super) fn find_window_rows(&self) -> Vec<String> {
        self.find.window.as_ref().map(|w| w.rows_text()).unwrap_or_default()
    }

    pub(super) fn dispatch_find_window_for_smoke(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        self.open_in_find_window(window, cx);
    }

    pub(super) fn toggle_find_window(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        if self.bottom == Some(BottomTool::Find) {
            self.bottom = None;
            self.focus_editor(window, cx);
        } else {
            self.bottom = Some(BottomTool::Find);
            if let Some(w) = &self.find.window {
                window.focus(&w.focus.clone());
            }
        }
    }

    pub(super) fn find_window_focused(&self, window: &Window) -> bool {
        self.find.window.as_ref().is_some_and(|w| w.focus.is_focused(window))
    }

    pub(super) fn find_window_key(&mut self, k: &gpui::Keystroke, window: &mut Window, cx: &mut Context<Self>) -> bool {
        let Some(w) = self.find.window.as_mut() else { return false };
        let n = w.rows.len();
        match k.key.as_str() {
            "up" => w.selected = w.selected.saturating_sub(1),
            "down" => w.selected = (w.selected + 1).min(n.saturating_sub(1)),
            "left" => w.left(),
            "right" => w.right(),
            "enter" | "f4" => {
                match w.rows.get(w.selected).copied() {
                    Some(WRow::Match(i)) => {
                        let (files, m) = (w.files.clone(), w.results.matches[i].clone());
                        self.open_match(&files, &m, window, cx);
                    }
                    // Enter on a group or file folds it, as in PyCharm's trees.
                    Some(_) if k.key == "enter" => {
                        let at = w.selected;
                        w.toggle(at);
                        cx.notify();
                    }
                    _ => {}
                }
                return true;
            }
            _ => return false,
        }
        w.scroll.scroll_to_item(w.selected, gpui::ScrollStrategy::Center);
        cx.notify();
        true
    }

    fn replace_all(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let f = &self.find;
        let Some(files) = f.files.clone() else { return };
        if f.results.matches.is_empty() {
            return;
        }
        if f.results.truncated {
            return self.show_toast(format!("More than {LIMIT} matches: narrow the search before replacing"), true, cx);
        }
        let mut targets: Vec<usize> = f.results.matches.iter().map(|m| m.file).collect();
        targets.dedup();
        if !f.confirm_replace {
            self.find.confirm_replace = true;
            cx.notify();
            return;
        }
        let q = match Query::new(&f.effective_opts()) {
            Ok(q) => q,
            Err(e) => return self.show_toast(e, true, cx),
        };
        let with = f.replace.clone();
        let (mut total, mut nfiles, mut failed) = (0usize, 0usize, Vec::new());
        for fi in targets {
            let path = files.root.join(&files.files[fi]);
            // Open documents are changed through the editor so the replacement can be undone.
            let open = self.editors().find(|e| e.read(cx).path.as_deref() == Some(path.as_path())).cloned();
            let n = match open {
                Some(ed) => ed.update(cx, |e, cx| {
                    let (new, n) = q.replace(&e.buffer.text(), &with);
                    if n > 0 {
                        e.buffer.set_text(&new);
                        let _ = e.save();
                        cx.notify();
                    }
                    n
                }),
                None => match brisk_search::read_text(&path) {
                    Some(text) => {
                        let (new, n) = q.replace(&text, &with);
                        if n > 0 && std::fs::write(&path, new).is_err() {
                            failed.push(files.files[fi].clone());
                            0
                        } else {
                            n
                        }
                    }
                    None => 0,
                },
            };
            if n > 0 {
                total += n;
                nfiles += 1;
            }
        }
        if failed.is_empty() {
            self.show_toast(format!("Replaced {total} occurrence{} in {nfiles} file{}", if total == 1 { "" } else { "s" }, if nfiles == 1 { "" } else { "s" }), false, cx);
        } else {
            self.show_toast(format!("Replaced {total} occurrences; could not write: {}", failed.join(", ")), true, cx);
        }
        self.find.open = false;
        self.focus_editor(window, cx);
        self.refresh_git(cx);
        self.schedule_search(cx);
    }

    /// All keys while the popup is open.
    pub(super) fn find_key(&mut self, e: &KeyDownEvent, window: &mut Window, cx: &mut Context<Self>) {
        let k = &e.keystroke;
        let m = k.modifiers;
        let n = self.find.results.matches.len();
        let mut research = false;
        match k.key.as_str() {
            "escape" => return self.close_find(window, cx),
            "up" | "down" | "pageup" | "pagedown" => {
                let f = &mut self.find;
                let step = if k.key.starts_with("page") { 10 } else { 1 };
                f.selected = if k.key == "up" || k.key == "pageup" { f.selected.saturating_sub(step) } else { (f.selected + step).min(n.saturating_sub(1)) };
                f.scroll.scroll_to_item(f.selected, gpui::ScrollStrategy::Center);
                self.load_preview();
            }
            "enter" if m.control => return self.open_in_find_window(window, cx),
            "enter" if m.alt || (self.find.field == Field::Replace && self.find.confirm_replace) => return self.replace_all(window, cx),
            "enter" => {
                let f = &self.find;
                if let (Some(files), Some(mt)) = (f.files.clone(), f.results.matches.get(f.selected).cloned()) {
                    self.find.open = false;
                    self.open_match(&files, &mt, window, cx);
                }
            }
            "tab" => {
                let f = &mut self.find;
                f.seed_selected = false;
                let order: Vec<Field> = [Some(Field::Query), f.replace_mode.then_some(Field::Replace), f.mask_on.then_some(Field::Mask)].into_iter().flatten().collect();
                let i = order.iter().position(|x| *x == f.field).unwrap_or(0);
                f.field = order[if m.shift { (i + order.len() - 1) % order.len() } else { (i + 1) % order.len() }];
            }
            "c" | "w" | "x" | "m" if m.alt && !m.control => {
                let f = &mut self.find;
                match k.key.as_str() {
                    "c" => f.opts.case_sensitive = !f.opts.case_sensitive,
                    "w" => f.opts.whole_words = !f.opts.whole_words,
                    "x" => f.opts.regex = !f.opts.regex,
                    _ => {
                        f.mask_on = !f.mask_on;
                        if f.mask_on && f.opts.file_mask.is_empty() {
                            f.opts.file_mask = "*.py".into();
                        }
                        f.field = if f.mask_on { Field::Mask } else { Field::Query };
                    }
                }
                research = true;
            }
            "a" if m.alt && self.find.replace_mode => return self.replace_all(window, cx),
            "f" | "r" if m.control && m.shift => {
                let replace = k.key == "r";
                return self.open_find(replace, window, cx);
            }
            "backspace" => {
                let f = &mut self.find;
                if f.seed_selected || m.control {
                    f.field_mut().clear();
                } else {
                    f.field_mut().pop();
                }
                f.seed_selected = false;
                research = f.field != Field::Replace;
            }
            "v" if m.control => {
                if let Some(t) = cx.read_from_clipboard().and_then(|c| c.text()) {
                    let line = t.lines().next().unwrap_or("").to_string();
                    let f = &mut self.find;
                    if f.seed_selected {
                        f.field_mut().clear();
                    }
                    f.field_mut().push_str(&line);
                    f.seed_selected = false;
                    research = f.field != Field::Replace;
                }
            }
            "a" if m.control => self.find.seed_selected = true,
            _ => {
                if !(m.control || m.alt || m.platform) {
                    if let Some(ch) = &k.key_char {
                        if !ch.chars().any(|c| c.is_control()) {
                            let f = &mut self.find;
                            if f.seed_selected {
                                f.field_mut().clear();
                                f.seed_selected = false;
                            }
                            f.field_mut().push_str(ch);
                            research = f.field != Field::Replace;
                            f.confirm_replace = false;
                        }
                    }
                }
            }
        }
        if research {
            self.schedule_search(cx);
        }
        cx.notify();
    }

    // --- Rendering -------------------------------------------------------------------

    pub(super) fn render_find_popup(&mut self, cx: &mut Context<Self>) -> Option<gpui::AnyElement> {
        if !self.find.open {
            return None;
        }
        let f = &self.find;
        let n = f.results.matches.len();
        let count = if f.opts.query.is_empty() {
            String::new()
        } else if f.searching {
            "Searching…".into()
        } else if let Some(e) = &f.error {
            e.clone()
        } else if n == 0 {
            "No matches".into()
        } else {
            format!(
                "{}{} match{} in {} file{} · {:.0?}",
                n,
                if f.results.truncated { "+" } else { "" },
                if n == 1 { "" } else { "es" },
                f.results.files_with_matches,
                if f.results.files_with_matches == 1 { "" } else { "s" },
                f.took
            )
        };
        let toggle = |id: &'static str, label: &'static str, on: bool, tip: &'static str, which: u8, cx: &mut Context<Self>| {
            let tip: SharedString = tip.into();
            div()
                .id(id)
                .px(px(6.))
                .py(px(1.))
                .rounded(px(4.))
                .text_size(px(12.))
                .font_family(mono_font())
                .cursor_pointer()
                .when(on, |d| d.bg(Theme::selection_row()).text_color(Theme::text()))
                .when(!on, |d| d.text_color(Theme::text_muted()))
                .when(!on, |d| d.hover(|s| s.bg(Theme::hover_row())))
                .tooltip(move |_, cx| cx.new(|_| Tip(tip.clone())).into())
                .child(label)
                .on_click(cx.listener(move |this, _, _, cx| {
                    let f = &mut this.find;
                    match which {
                        0 => f.opts.case_sensitive = !f.opts.case_sensitive,
                        1 => f.opts.whole_words = !f.opts.whole_words,
                        2 => f.opts.regex = !f.opts.regex,
                        _ => {
                            f.mask_on = !f.mask_on;
                            if f.mask_on && f.opts.file_mask.is_empty() {
                                f.opts.file_mask = "*.py".into();
                            }
                            f.field = if f.mask_on { Field::Mask } else { Field::Query };
                        }
                    }
                    this.schedule_search(cx);
                    cx.notify();
                }))
        };
        let field_box = |id: &'static str, value: &str, placeholder: &'static str, focused: bool, selected: bool, which: Field, cx: &mut Context<Self>| {
            crate::ui_kit::input_frame(focused, false, crate::ui_kit::CONTROL_H)
                .id(id)
                .mx(px(10.))
                .px(px(8.))
                .font_family(mono_font())
                .cursor_text()
                .on_click(cx.listener(move |this, _, _, cx| {
                    this.find.field = which;
                    this.find.seed_selected = false;
                    cx.notify();
                }))
                .child(if value.is_empty() {
                    div().flex().child(div().when(focused, |d| d.w(px(1.5)).h(px(16.)).bg(Theme::caret()))).child(div().text_color(Theme::text_dim()).child(placeholder)).into_any_element()
                } else {
                    div()
                        .flex()
                        .min_w_0()
                        .overflow_hidden()
                        .whitespace_nowrap()
                        .child(div().when(selected, |d| d.bg(Theme::selection())).child(value.to_string()))
                        .when(focused, |d| d.child(div().w(px(1.5)).h(px(16.)).bg(Theme::caret())))
                        .into_any_element()
                })
        };

        let header = div()
            .flex()
            .items_center()
            .gap(px(6.))
            .px(px(14.))
            .pt(px(10.))
            .pb(px(6.))
            .child(div().font_weight(gpui::FontWeight::SEMIBOLD).child(if f.replace_mode { "Replace in Files" } else { "Find in Files" }))
            .child(div().pl(px(8.)).text_size(px(12.)).text_color(if f.error.is_some() { Theme::error() } else { Theme::text_muted() }).child(count))
            .child(div().flex_1())
            .child(toggle("ft-case", "Cc", f.opts.case_sensitive, "Match Case  Alt+C", 0, cx))
            .child(toggle("ft-words", "W", f.opts.whole_words, "Words  Alt+W", 1, cx))
            .child(toggle("ft-regex", ".*", f.opts.regex, "Regex  Alt+X", 2, cx))
            .child(toggle("ft-mask", "File mask", f.mask_on, "File mask  Alt+M  (*.py, !test_*)", 3, cx));
        let scope = div()
            .flex()
            .gap(px(4.))
            .px(px(10.))
            .pb(px(6.))
            .text_size(px(12.))
            .child(div().px(px(10.)).py(px(3.)).rounded(px(5.)).bg(Theme::selection_row()).child("In Project"))
            .when(f.mask_on, |d| d.child(div().flex_1()).child(div().w(px(220.)).child(field_box("fx-mask", &f.opts.file_mask, "*.py", f.field == Field::Mask, false, Field::Mask, cx))));

        let selected = f.selected;
        let list = uniform_list(
            "find-results",
            n,
            cx.processor(move |this, range: std::ops::Range<usize>, _, cx| {
                let f = &this.find;
                range
                    .map(|i| {
                        let m = &f.results.matches[i];
                        let (name, _) = split_rel(f.rel(m));
                        div()
                            .id(("fr", i))
                            .h(px(ROW_H))
                            .mx(px(6.))
                            .px(px(8.))
                            .flex()
                            .items_center()
                            .gap(px(12.))
                            .rounded(px(5.))
                            .when(i == selected, |d| d.bg(Theme::selection_row()))
                            .when(!(i == selected), |d| d.hover(|s| s.bg(Theme::hover_row())))
                            .cursor_pointer()
                            .on_click(cx.listener(move |this, e: &ClickEvent, window, cx| {
                                this.find.selected = i;
                                this.load_preview();
                                if e.click_count() >= 2 {
                                    let f = &this.find;
                                    if let (Some(files), Some(m)) = (f.files.clone(), f.results.matches.get(i).cloned()) {
                                        this.find.open = false;
                                        this.open_match(&files, &m, window, cx);
                                    }
                                }
                                cx.notify();
                            }))
                            .child(match_line(m).flex_1().font_family(mono_font()).text_size(px(12.5)))
                            .child(div().flex_shrink_0().text_size(px(12.)).text_color(Theme::text_muted()).whitespace_nowrap().child(format!("{name} {}", m.line + 1)))
                    })
                    .collect()
            }),
        )
        .track_scroll(f.scroll.clone())
        .flex_1();

        // Preview of the selected match with a few lines of context.
        let preview = f.results.matches.get(f.selected).and_then(|m| {
            let (_, lines) = f.preview.as_ref().filter(|(fi, _)| *fi == m.file)?;
            let lo = m.line.saturating_sub(PREVIEW_CONTEXT);
            let hi = (m.line + PREVIEW_CONTEXT + 1).min(lines.len());
            let rel = f.rel(m).to_string();
            Some(
                div()
                    .flex()
                    .flex_col()
                    .h(px(190.))
                    .flex_shrink_0()
                    .border_t_1()
                    .border_color(Theme::divider())
                    .bg(Theme::editor_bg())
                    .child(div().px(px(12.)).py(px(4.)).text_size(px(12.)).text_color(Theme::text_muted()).child(rel))
                    .children((lo..hi).map(|l| {
                        let text = lines[l].clone();
                        let row = div().h(px(18.)).flex().font_family(mono_font()).text_size(px(12.5));
                        let num = div().w(px(48.)).flex_shrink_0().pr(px(10.)).flex().justify_end().text_color(Theme::gutter_fg()).child((l + 1).to_string());
                        if l == m.line {
                            row.bg(Theme::current_line()).child(num).child(match_line(&Match { text: m.text.clone(), hl: m.hl.clone(), ..m.clone() }).flex_1())
                        } else {
                            row.child(num).child(div().whitespace_nowrap().overflow_hidden().child(text))
                        }
                    })),
            )
        });

        let replace_bar = f.replace_mode.then(|| {
            let label = if f.confirm_replace {
                format!("Confirm: replace {} in {} files", n, f.results.files_with_matches)
            } else {
                "Replace All  Alt+A".to_string()
            };
            div()
                .flex()
                .items_center()
                .gap(px(8.))
                .px(px(10.))
                .pt(px(6.))
                .child(div().flex_1().child(field_box("fx-replace", &f.replace, "Replace with", f.field == Field::Replace, false, Field::Replace, cx).mx(px(0.))))
                .child(
                    div()
                        .id("btn-replace-all")
                        .px(px(12.))
                        .py(px(5.))
                        .rounded(px(5.))
                        .text_size(px(13.))
                        .cursor_pointer()
                        .when(f.confirm_replace, |d| d.bg(Theme::error()))
                        .when(!f.confirm_replace, |d| d.bg(Theme::accent()))
                        .child(label)
                        .on_click(cx.listener(|this, _, window, cx| this.replace_all(window, cx))),
                )
        });

        Some(
            div()
                .absolute()
                .top_0()
                .left_0()
                .size_full()
                .flex()
                .justify_center()
                .pt(px(60.))
                .occlude()
                .child(
                    div()
                        .id("find-popup")
                        .on_mouse_down_out(cx.listener(|this, _, window, cx| this.close_find(window, cx)))
                        .w(px(860.))
                        .h(px(620.))
                        .flex()
                        .flex_col()
                        .bg(Theme::panel())
                        .border_1()
                        .border_color(Theme::divider())
                        .rounded(px(10.))
                        .shadow_lg()
                        .overflow_hidden()
                        .text_size(px(13.))
                        .text_color(Theme::text())
                        .child(header)
                        .child(scope)
                        .child(field_box("fx-query", &f.opts.query, "Text to find", f.field == Field::Query, f.seed_selected, Field::Query, cx))
                        .children(replace_bar)
                        .child(div().h(px(6.)))
                        .child(list)
                        .children(preview)
                        .child(
                            div()
                                .h(px(26.))
                                .flex()
                                .items_center()
                                .px(px(12.))
                                .gap(px(14.))
                                .border_t_1()
                                .border_color(Theme::divider())
                                .text_size(px(11.))
                                .text_color(Theme::text_dim())
                                .child("Enter — open")
                                .child("Ctrl+Enter — open in Find window")
                                .child("Tab — next field")
                                .child("Esc — close"),
                        ),
                )
                .into_any_element(),
        )
    }

    pub(super) fn render_find_window(&mut self, cx: &mut Context<Self>) -> gpui::AnyElement {
        let Some(w) = &self.find.window else {
            return Self::render_placeholder("Find", "Search with Ctrl+Shift+F, then Ctrl+Enter to show results here.");
        };
        let n = w.rows.len();
        let focus = w.focus.clone();
        let (one, many) = if w.group_of.is_empty() { ("match", "matches") } else { ("usage", "usages") };
        let header = format!(
            "{}   {} {} in {} file{}",
            w.title,
            w.results.matches.len(),
            if w.results.matches.len() == 1 { one } else { many },
            w.results.files_with_matches,
            if w.results.files_with_matches == 1 { "" } else { "s" }
        );
        let list = uniform_list(
            "find-window",
            n,
            cx.processor(move |this, range: std::ops::Range<usize>, window, cx| {
                let Some(w) = this.find.window.as_ref() else { return Vec::new() };
                let focused = w.focus.is_focused(window);
                range
                    .map(|i| {
                        let selected = w.selected == i;
                        let base = div()
                            .id(("fw", i))
                            .h(px(22.))
                            .flex()
                            .items_center()
                            .gap(px(8.))
                            .text_size(px(13.))
                            .w_full()
                            .when(selected, |d| d.bg(if focused { Theme::selection_row() } else { Theme::hover_row() }))
                            .when(!selected, |d| d.hover(|s| s.bg(Theme::hover_row())))
                            .cursor_pointer();
                        let row = w.rows[i];
                        let indent = 10. + 18. * w.depth(row) as f32;
                        // Folding: a click on the arrow or a double click on the node.
                        let node_click = cx.listener(move |this, e: &ClickEvent, window, cx| {
                            if let Some(w) = this.find.window.as_mut() {
                                w.selected = i;
                                window.focus(&w.focus.clone());
                                if e.click_count() >= 2 {
                                    w.toggle(i);
                                }
                            }
                            cx.notify();
                        });
                        let arrow = div()
                            .id(("fw-fold", i))
                            .w(px(12.))
                            .text_color(Theme::text_muted())
                            .child(if w.is_collapsed(row) { "▸" } else { "▾" })
                            .on_click(cx.listener(move |this, e: &ClickEvent, _, cx| {
                                if e.click_count() == 1 {
                                    if let Some(w) = this.find.window.as_mut() {
                                        w.toggle(i);
                                    }
                                    cx.stop_propagation();
                                    cx.notify();
                                }
                            }));
                        let (one, many) = if w.group_of.is_empty() { ("match", "matches") } else { ("usage", "usages") };
                        let count_label = move |n: usize| format!("{n} {}", if n == 1 { one } else { many });
                        match row {
                            WRow::Group(g, count) => base
                                .pl(px(indent))
                                .on_click(node_click)
                                .child(arrow)
                                .child(div().font_weight(gpui::FontWeight::SEMIBOLD).child(tree::USAGE_GROUPS[g as usize]))
                                .child(div().text_color(Theme::text_muted()).text_size(px(12.)).child(count_label(count))),
                            WRow::File(_, fi, count) => {
                                let (name, dir) = split_rel(&w.files.files[fi]);
                                let (icon, color) = file_icon(name, false);
                                base.pl(px(indent))
                                    .on_click(node_click)
                                    .child(arrow)
                                    .child(div().text_color(color).child(icon))
                                    .child(name.to_string())
                                    .child(div().text_color(Theme::text_dim()).text_size(px(12.)).child(dir.to_string()))
                                    .child(div().text_color(Theme::text_muted()).text_size(px(12.)).child(count_label(count)))
                            }
                            WRow::Match(mi) => {
                                let m = &w.results.matches[mi];
                                base.pl(px(indent))
                                    .on_click(cx.listener(move |this, e: &ClickEvent, window, cx| {
                                        let Some(w) = this.find.window.as_mut() else { return };
                                        w.selected = i;
                                        window.focus(&w.focus.clone());
                                        if e.click_count() >= 2 {
                                            let (files, m) = (w.files.clone(), w.results.matches[mi].clone());
                                            this.open_match(&files, &m, window, cx);
                                        }
                                        cx.notify();
                                    }))
                                    .child(div().w(px(40.)).flex_shrink_0().flex().justify_end().text_color(Theme::gutter_fg_active()).font_family(mono_font()).text_size(px(12.)).child((m.line + 1).to_string()))
                                    .child(match_line(m).flex_1().font_family(mono_font()).text_size(px(12.5)))
                            }
                        }
                    })
                    .collect()
            }),
        )
        .track_scroll(w.scroll.clone())
        .flex_1();
        div()
            .id("find-window-root")
            .track_focus(&focus)
            .key_context("FindView")
            .size_full()
            .flex()
            .flex_col()
            .child(
                div()
                    .h(px(30.))
                    .flex()
                    .items_center()
                    .px(px(12.))
                    .gap(px(12.))
                    .text_size(px(13.))
                    .border_b_1()
                    .border_color(Theme::divider())
                    .child(div().font_weight(gpui::FontWeight::SEMIBOLD).child("Find"))
                    .child(div().text_color(Theme::text_muted()).child(header))
                    .child(div().flex_1())
                    .child(
                        div().id("fw-rerun").px(px(5.)).rounded(px(4.)).cursor_pointer().text_color(Theme::text_muted()).hover(|s| s.bg(Theme::hover_row())).child("⟳")
                            .on_click(cx.listener(|this, _, window, cx| {
                                this.find.open = true;
                                this.find.seed_selected = false;
                                window.focus(&this.focus);
                                this.schedule_search(cx);
                                cx.notify();
                            })),
                    ),
            )
            .child(list)
            .into_any_element()
    }
}
