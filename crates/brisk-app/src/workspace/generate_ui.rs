//! Code › Generate (Alt+Insert), Override Methods (Ctrl+O) and Implement Methods (Ctrl+I) in
//! a Python class. Base classes are found by ty's go to definition when ty runs (library
//! classes too), else in the same file, its `from ... import`s and the project's symbol
//! index; their bases follow.

use super::problems_ui::Intent;
use super::{PopupItem, PopupKind, Target, Workspace};
use brisk_lint::Pos;
use brisk_syntax::{ClassInfo, Lang, Method, OutlineKind, SyntaxTree};
use gpui::{Context, Window};
use std::path::{Path, PathBuf};

fn item(label: String, detail: String, intent: Intent) -> PopupItem {
    PopupItem { label, detail, hint: String::new(), target: Target::Intention(Box::new(intent)) }
}

/// The class `name` (or the one at `row`) in `text`.
fn class_in(text: &str, name: &str, row: Option<usize>) -> Option<ClassInfo> {
    let mut t = SyntaxTree::new(Lang::Python);
    t.parse(text);
    brisk_syntax::class_named(t.tree()?, text, name, row)
}

/// Where `from module import name [as alias]` in `text` points: candidate files and the
/// name in that module (relative imports from `file`'s folder, absolute from `root`).
fn imported(text: &str, name: &str, file: &Path, root: &Path) -> Vec<(PathBuf, String)> {
    let mut out = Vec::new();
    for l in text.lines().map(str::trim) {
        let Some((module, names)) = l.strip_prefix("from ").and_then(|r| r.split_once(" import ")) else { continue };
        for n in names.trim_matches(|c| c == '(' || c == ')').split(',') {
            let (orig, alias) = n.trim().split_once(" as ").map(|(a, b)| (a.trim(), b.trim())).unwrap_or((n.trim(), n.trim()));
            if alias != name {
                continue;
            }
            let dots = module.len() - module.trim_start_matches('.').len();
            let Some(base) = (if dots > 0 { file.ancestors().nth(dots) } else { Some(root) }) else { continue };
            let rel = module.trim_start_matches('.').replace('.', "/");
            out.push((base.join(format!("{rel}.py")), orig.to_string()));
            out.push((base.join(rel).join("__init__.py"), orig.to_string()));
        }
    }
    out
}

/// Methods of `c`'s bases, nearest first (a name once), with the class each comes from.
/// `located`: ty's (file, row) for each direct base, when ty answered.
fn base_methods(c: &ClassInfo, file: &Path, root: &Path, text: &str, located: Vec<Option<(PathBuf, usize)>>, index: &[(String, PathBuf)]) -> Vec<(Method, String)> {
    let load = |p: &Path| if p == file { Some(text.to_string()) } else { std::fs::read_to_string(p).ok() };
    let mut queue: std::collections::VecDeque<(String, Option<(PathBuf, usize)>, PathBuf)> =
        c.bases.iter().zip(located.into_iter().chain(std::iter::repeat(None))).map(|((b, _), loc)| (b.clone(), loc, file.to_path_buf())).collect();
    let mut seen = std::collections::HashSet::new();
    let mut out: Vec<(Method, String)> = Vec::new();
    while let Some((name, loc, from)) = queue.pop_front() {
        if name == "object" || !seen.insert(name.clone()) || seen.len() > 30 {
            continue;
        }
        let found = loc
            .and_then(|(p, row)| Some((class_in(&load(&p)?, &name, Some(row))?, p)))
            .or_else(|| Some((class_in(&load(&from)?, &name, None)?, from.clone())))
            .or_else(|| imported(&load(&from)?, &name, &from, root).into_iter().find_map(|(p, orig)| Some((class_in(&load(&p)?, &orig, None)?, p))))
            .or_else(|| index.iter().filter(|(n, _)| *n == name).find_map(|(_, p)| Some((class_in(&load(p)?, &name, None)?, p.clone()))));
        let Some((base, at)) = found else { continue };
        for m in base.methods {
            if !out.iter().any(|(o, _)| o.name == m.name) {
                out.push((m, base.name.clone()));
            }
        }
        queue.extend(base.bases.into_iter().map(|(b, _)| (b, None, at.clone())));
    }
    out
}

impl Workspace {
    /// Alt+Insert in a Python class.
    pub(super) fn generate_action(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let Some(ed) = self.focused_editor(window, cx).or_else(|| self.active_editor().cloned()) else { return };
        let Some(c) = ed.update(cx, |e, _| e.class_at_caret()) else {
            self.status = "Generate: put the caret inside a Python class".into();
            cx.notify();
            return;
        };
        let has = |n: &str| c.methods.iter().any(|m| m.name == n);
        let mut items = vec![
            item("Override Methods...".into(), "Ctrl+O".into(), Intent::Overrides(false)),
            item("Implement Methods...".into(), "Ctrl+I".into(), Intent::Overrides(true)),
        ];
        if !has("__init__") {
            items.push(item("__init__".into(), "from class fields".into(), Intent::Member(brisk_syntax::gen_init(&c))));
        }
        let fields = brisk_syntax::data_fields(&c);
        if !has("__repr__") {
            items.push(item("__repr__".into(), String::new(), Intent::Member(brisk_syntax::gen_repr(&c))));
        }
        if !has("__eq__") && !fields.is_empty() {
            items.push(item("__eq__".into(), String::new(), Intent::Member(brisk_syntax::gen_eq(&c))));
        }
        for f in brisk_syntax::property_candidates(&c) {
            items.push(item(format!("Property '{}'", f.trim_start_matches('_')), format!("self.{f}"), Intent::Member(brisk_syntax::gen_property(&f))));
        }
        self.problems.intents = items;
        self.problems.intents_title = Some("Generate");
        self.open_popup(PopupKind::Intentions);
        cx.notify();
    }

    /// Ctrl+O / Ctrl+I: the bases' methods (and `object`'s) to override, or the abstract
    /// ones to implement.
    pub(super) fn override_methods(&mut self, implement: bool, window: &mut Window, cx: &mut Context<Self>) {
        let what = if implement { "Implement Methods" } else { "Override Methods" };
        let Some(ed) = self.focused_editor(window, cx).or_else(|| self.active_editor().cloned()) else { return };
        let (c, path, text) = ed.update(cx, |e, _| (e.class_at_caret(), e.path.clone(), e.text_string()));
        let (Some(c), Some(path)) = (c, path) else {
            self.status = format!("{what}: put the caret inside a Python class");
            self.problems.intents.clear();
            cx.notify();
            return;
        };
        let pos = |byte: usize| {
            let start = text[..byte].rfind('\n').map(|n| n + 1).unwrap_or(0);
            Pos { line: text[..byte].matches('\n').count(), col: text[start..byte].chars().count() }
        };
        let ty = self.ty_at_caret(&ed, cx).map(|(ty, p, _)| (ty, p));
        let asks: Vec<_> = c.bases.iter().map(|(_, b)| ty.as_ref().map(|(t, p)| t.definition(p, pos(*b)))).collect();
        self.ensure_symbols(cx);
        let index: Vec<(String, PathBuf)> = match (&self.symbols.list, &self.symbols.files) {
            (Some(list), Some(files)) => list
                .iter()
                .filter(|s| s.kind == OutlineKind::Class)
                .map(|s| (s.name.to_string(), files.abs(s.file)))
                .collect(),
            _ => Vec::new(),
        };
        let root = self.root.clone();
        let bg = cx.background_executor().clone();
        cx.spawn(async move |this, cx| {
            let mut located = Vec::new();
            for ask in asks {
                let loc = match ask {
                    Some(f) => f.await.ok().and_then(|l| l.into_iter().next()).map(|l| (l.path, l.start.line)),
                    None => None,
                };
                located.push(loc);
            }
            let own: Vec<String> = c.methods.iter().map(|m| m.name.clone()).collect();
            let mut found = bg.spawn(async move { base_methods(&c, &path, &root, &text, located, &index) }).await;
            found.retain(|(m, _)| !own.contains(&m.name) && !(m.name.starts_with("__") && !m.name.ends_with("__")));
            if implement {
                found.retain(|(m, _)| m.decorators.iter().any(|d| d.contains("abstractmethod")));
            } else {
                for m in brisk_syntax::object_methods() {
                    if !own.contains(&m.name) && !found.iter().any(|(o, _)| o.name == m.name) {
                        found.push((m, "object".into()));
                    }
                }
            }
            let _ = this.update(cx, |this, cx| {
                if found.is_empty() {
                    this.problems.intents.clear();
                    this.status = format!("{what}: nothing to {}", if implement { "implement" } else { "override" });
                } else {
                    this.problems.intents = found.iter().map(|(m, base)| item(format!("{}({})", m.name, m.params), base.clone(), Intent::Member(brisk_syntax::gen_override(m, base)))).collect();
                    this.problems.intents_title = Some(if implement { "Select Methods to Implement" } else { "Select Methods to Override" });
                    this.open_popup(PopupKind::Intentions);
                }
                cx.notify();
            });
        })
        .detach();
    }
}
