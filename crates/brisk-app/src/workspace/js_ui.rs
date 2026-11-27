//! Ctrl+B / Ctrl+Click in JS, TS, HTML and CSS without a TypeScript server: a path in an
//! import, `require`, `src`, `href`, `@import` or `url()` opens that file; an imported name
//! opens its module at the export; any other name goes to its declaration in the file, else
//! to the project's symbols of that name (several: listed in the Find window).

use super::lsp_ui::NavPos;
use super::Workspace;
use crate::editor::Editor;
use brisk_lsp::CompletionItem;
use brisk_project::jsmod;
use brisk_syntax::{css, Lang, OutlineItem, SyntaxTree};
use gpui::{Context, Entity, Window};
use std::path::{Path, PathBuf};

/// A file this navigation is for.
pub(super) fn is_web_file(path: &Path) -> bool {
    Lang::from_path(path).is_some_and(|l| l.is_js() || matches!(l, Lang::Html | Lang::Css))
        || path.extension().is_some_and(|e| matches!(e.to_str(), Some("vue" | "svelte" | "scss" | "less")))
}

/// Definitions and top-level names of a JS / TS text.
fn declarations(lang: Lang, text: &str) -> Vec<OutlineItem> {
    let mut t = SyntaxTree::new(lang);
    t.parse(text);
    let mut v = t.outline(text);
    v.extend(t.module_variables(text));
    v
}

/// Where `name` is declared in a JS / TS text: a definition, a top-level name, else
/// `let` / `const` / `var` / `function` before `before_line`.
fn declared_in(lang: Lang, text: &str, name: &str, before_line: Option<usize>) -> Option<(usize, usize)> {
    let d = declarations(lang, text);
    if let Some(o) = d.iter().find(|o| o.name == name && o.depth == 0).or_else(|| d.iter().find(|o| o.name == name)) {
        let line_start = text[..o.byte].rfind('\n').map_or(0, |i| i + 1);
        return Some((o.line, text[line_start..o.byte].chars().count()));
    }
    // A local: the nearest `const name` / `let name` / `var name` above the use.
    let ident = |c: char| c.is_alphanumeric() || c == '_' || c == '$';
    let mut best = None;
    for (i, line) in text.lines().enumerate() {
        if before_line.is_some_and(|b| i > b) {
            break;
        }
        for kw in ["const ", "let ", "var ", "function "] {
            let mut from = 0;
            while let Some(k) = line[from..].find(kw) {
                let at = from + k + kw.len();
                let starts = from + k == 0 || !line[..from + k].ends_with(ident);
                if starts && line[at..].starts_with(name) && !line[at + name.len()..].starts_with(ident) {
                    best = Some((i, line[..at].chars().count()));
                }
                from = at;
            }
        }
    }
    best
}

impl Workspace {
    /// Ctrl+B / Ctrl+Click in a web file; false when it is not one (or, in HTML, nothing
    /// here is a path: Django templates go on to their own navigation).
    pub(super) fn js_goto(&mut self, ed: &Entity<Editor>, at: Option<usize>, window: &mut Window, cx: &mut Context<Self>) -> bool {
        let (path, text, byte, word, line) = {
            let e = ed.read(cx);
            let Some(path) = e.path.clone().filter(|p| is_web_file(p)) else { return false };
            // TypeScript's server knows better when it runs.
            if super::ts_ui::is_ts_path(&path) && self.lsp.ts.state == super::lsp_ui::TyState::Ready {
                return false;
            }
            let rope = e.buffer.rope();
            let pos = at.unwrap_or_else(|| e.buffer.primary().head).min(rope.len_chars());
            let r = e.buffer.word_range_at(pos);
            (path, e.text_string(), rope.char_to_byte(pos), rope.slice(r).to_string(), rope.char_to_line(pos))
        };
        let lang = Lang::from_path(&path);
        if let Some(spec) = jsmod::reference_at(&text, byte).filter(|s| !s.is_empty()) {
            match jsmod::resolve(&spec, &path, &self.root) {
                Some(p) => self.navigate_to(NavPos { path: p, line: 0, col: 0 }, window, cx),
                None => self.status = format!("Cannot find '{spec}'"),
            }
            cx.notify();
            return true;
        }
        let Some(js) = lang.filter(|l| l.is_js()) else { return false };
        if word.is_empty() || !word.chars().next().is_some_and(|c| c.is_alphabetic() || c == '_' || c == '$') {
            return false;
        }
        // An imported name: its module, at the export.
        if let Some((module, exported)) = jsmod::import_of(&text, &word) {
            let Some(file) = jsmod::resolve(&module, &path, &self.root) else {
                self.status = format!("Cannot find module '{module}'");
                cx.notify();
                return true;
            };
            let target = Lang::from_path(&file).filter(|l| l.is_js());
            let (line, col) = match (target, exported.as_str()) {
                (Some(l), name) if name != "default" && name != "*" => {
                    std::fs::read_to_string(&file).ok().and_then(|t| declared_in(l, &t, name, None)).unwrap_or((0, 0))
                }
                (Some(_), "default") => {
                    std::fs::read_to_string(&file).ok().and_then(|t| t.lines().position(|l| l.contains("export default")).map(|i| (i, 0))).unwrap_or((0, 0))
                }
                _ => (0, 0),
            };
            self.navigate_to(NavPos { path: file, line, col }, window, cx);
            cx.notify();
            return true;
        }
        if let Some((l, c)) = declared_in(js, &text, &word, Some(line)) {
            if l != line {
                self.navigate_to(NavPos { path, line: l, col: c }, window, cx);
                return true;
            }
        }
        // The project's symbols of that name.
        self.ensure_symbols(cx);
        let found: Vec<(PathBuf, usize, usize)> = match (&self.symbols.list, &self.symbols.files) {
            (Some(list), Some(files)) => list.iter().filter(|s| s.name == word).map(|s| (files.abs(s.file), s.line, s.col)).collect(),
            _ => Vec::new(),
        };
        match found.as_slice() {
            [] => self.status = format!("Cannot find declaration of '{word}'"),
            [(p, l, c)] => self.navigate_to(NavPos { path: p.clone(), line: *l, col: *c }, window, cx),
            _ => {
                let usages = found.into_iter().map(|(p, l, c)| (p, l, c..c)).collect();
                self.show_usages(format!("Declarations of '{word}'"), usages, window, cx);
            }
        }
        cx.notify();
        true
    }

    /// Completion of a path being typed in an import, `require`, `src`, `href`, `@import` or
    /// `url()`: the folder's entries (JS / TS modules without their extension in imports), or
    /// the packages of `node_modules` for a bare name. False when the caret is not in one.
    pub(super) fn js_path_completion(&mut self, ed: &Entity<Editor>, cx: &mut Context<Self>) -> bool {
        let (path, text, byte) = {
            let e = ed.read(cx);
            let Some(path) = e.path.clone().filter(|p| is_web_file(p)) else { return false };
            let rope = e.buffer.rope();
            let pos = e.buffer.primary().head.min(rope.len_chars());
            (path, e.text_string(), rope.char_to_byte(pos))
        };
        let Some((_, range)) = jsmod::reference_range_at(&text, byte) else { return false };
        let typed = &text[range.start..byte];
        let name_start = range.start + typed.rfind('/').map_or(0, |i| i + 1);
        let is_import = Lang::from_path(&path).is_some_and(|l| l.is_js());
        let mut items: Vec<CompletionItem> = Vec::new();
        let item = |label: String, dir: bool| CompletionItem {
            insert: if dir { format!("{label}/") } else { label.clone() },
            label,
            detail: String::new(),
            kind: if dir { 19 } else { 17 },
            sort: String::new(),
            range: None,
            resolve: None,
            extra: Vec::new(),
            doc: String::new(),
        };
        match jsmod::completion_dir(typed, &path, &self.root) {
            Some(dir) => {
                let mut entries: Vec<(String, bool)> = std::fs::read_dir(&dir)
                    .into_iter()
                    .flatten()
                    .flatten()
                    .filter_map(|e| {
                        let name = e.file_name().to_string_lossy().into_owned();
                        let is_dir = e.file_type().is_ok_and(|t| t.is_dir());
                        let hidden = name.starts_with('.') || matches!(name.as_str(), "node_modules" | "__pycache__");
                        (!hidden || typed[name_start - range.start..].starts_with('.')).then_some((name, is_dir))
                    })
                    .collect();
                entries.sort_by(|a, b| b.1.cmp(&a.1).then_with(|| a.0.to_lowercase().cmp(&b.0.to_lowercase())));
                for (name, is_dir) in entries.into_iter().take(500) {
                    // Imports name a module without its extension, and a folder's index by the folder.
                    let label = if is_import && !is_dir {
                        match name.rsplit_once('.') {
                            Some((stem, "ts" | "tsx" | "js" | "jsx" | "mjs" | "cjs" | "mts" | "cts")) if !stem.ends_with(".d") => stem.to_string(),
                            _ => name,
                        }
                    } else {
                        name
                    };
                    if !items.iter().any(|i| i.label == label) {
                        items.push(item(label, is_dir));
                    }
                }
            }
            None if is_import && !typed.contains('/') && !typed.starts_with('.') => {
                items = jsmod::packages(&path, &self.root).into_iter().map(|p| item(p, false)).collect();
            }
            None => return false,
        }
        if items.is_empty() {
            return false;
        }
        let start = ed.read(cx).buffer.rope().byte_to_char(name_start);
        ed.update(cx, |e, cx| e.show_path_completion(items, start, cx))
    }

    /// Completion of CSS property names and their values in a stylesheet, from the built-in
    /// table (brisk_syntax::css). False when the caret is not in a declaration.
    pub(super) fn css_completion(&mut self, ed: &Entity<Editor>, cx: &mut Context<Self>) -> bool {
        let (text, byte) = {
            let e = ed.read(cx);
            if !e.is_stylesheet() {
                return false;
            }
            let rope = e.buffer.rope();
            let pos = e.buffer.primary().head.min(rope.len_chars());
            (e.text_string(), rope.char_to_byte(pos))
        };
        let item = |label: &str, insert: String, kind: u32| CompletionItem {
            label: label.to_string(),
            insert,
            detail: String::new(),
            kind,
            sort: String::new(),
            range: None,
            resolve: None,
            extra: Vec::new(),
            doc: String::new(),
        };
        let (items, start): (Vec<CompletionItem>, usize) = match css::spot(&text[..byte]) {
            Some(css::Spot::Property { start }) => (css::properties().map(|p| item(p, format!("{p}: "), 10)).collect(), start),
            Some(css::Spot::Value { property, start }) => (css::values(&property).into_iter().map(|v| item(v, v.to_string(), 12)).collect(), start),
            None => return false,
        };
        if items.is_empty() {
            return false;
        }
        let start = ed.read(cx).buffer.rope().byte_to_char(start);
        ed.update(cx, |e, cx| e.show_path_completion(items, start, cx))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn local_and_top_level_declarations() {
        let t = "export function load() {}\nconst limit = 10;\nfunction f() {\n  let count = 0;\n  return count + limit;\n}\n";
        assert_eq!(declared_in(Lang::JavaScript, t, "load", None), Some((0, 16)));
        assert_eq!(declared_in(Lang::JavaScript, t, "limit", Some(4)), Some((1, 6)));
        assert_eq!(declared_in(Lang::JavaScript, t, "count", Some(4)), Some((3, 6)));
        assert_eq!(declared_in(Lang::JavaScript, t, "nothing", Some(4)), None);
    }
}
