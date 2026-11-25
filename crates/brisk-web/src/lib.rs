//! Web framework awareness, like PyCharm's Django and FastAPI support: the project's URL
//! routes (Django `urls.py` with `include()` and namespaces, FastAPI and Flask decorators with
//! router and blueprint prefixes), its template and static folders, and the references
//! between them (`render(request, "polls/detail.html")`, `{% url 'polls:detail' %}`,
//! `{% extends "base.html" %}`, `url_for("read_item")`).
//!
//! Everything is read from the source with tree-sitter; nothing is imported or run.

mod refs;
mod scan;
mod ts;

pub use refs::{looks_like_template, python_ref, template_ref, templates_in_scope, Ref};

use std::path::{Path, PathBuf};

#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub enum Framework {
    Django,
    FastApi,
    Flask,
}

impl Framework {
    pub fn name(self) -> &'static str {
        match self {
            Framework::Django => "Django",
            Framework::FastApi => "FastAPI",
            Framework::Flask => "Flask",
        }
    }
}

/// A place in a file: 0-based line and column (in characters).
#[derive(Clone, Debug, PartialEq, Eq, PartialOrd, Ord)]
pub struct Loc {
    pub path: PathBuf,
    pub line: usize,
    pub col: usize,
}

#[derive(Clone, Debug, PartialEq)]
pub struct Route {
    pub framework: Framework,
    /// `GET`, `POST`, `WS`…; empty for Django (any method).
    pub methods: Vec<String>,
    /// The full URL path: `/polls/<int:pk>/`, `/api/items/{item_id}`.
    pub path: String,
    /// What `reverse()` / `{% url %}` / `url_for()` take: `polls:detail`, `read_item`,
    /// `auth.login` (a Flask blueprint).
    pub name: Option<String>,
    /// The view as written: `views.detail`, `views.DetailView.as_view()`, `read_item`.
    pub handler: String,
    /// The route's declaration: the `path(...)` call or the decorator.
    pub at: Loc,
    /// The view function or class, when found.
    pub handler_at: Option<Loc>,
}

/// The web side of a project, built by [`Index::scan`] (tens of milliseconds for a few
/// hundred Python files).
#[derive(Clone, Debug, Default)]
pub struct Index {
    pub root: PathBuf,
    pub routes: Vec<Route>,
    /// Folders templates are looked up in: every `templates` folder of the project, plus
    /// `Jinja2Templates(directory=...)` / `Flask(template_folder=...)` folders.
    pub template_dirs: Vec<PathBuf>,
    pub static_dirs: Vec<PathBuf>,
    pub py_files: Vec<PathBuf>,
    pub template_files: Vec<PathBuf>,
}

impl Index {
    pub fn scan(root: &Path) -> Index {
        scan::scan(root)
    }

    /// [`Index::scan`] over the project's file list, without walking the project again.
    pub fn scan_files(list: &brisk_project::FileList) -> Index {
        scan::scan_files(list)
    }

    /// Whether the project uses a web framework at all.
    pub fn is_web(&self) -> bool {
        !self.routes.is_empty() || !self.template_dirs.is_empty()
    }

    /// Where a reference leads. Several places when the name is ambiguous (two template
    /// folders with the same file, a URL name defined twice).
    pub fn resolve(&self, from: &Path, r: &Ref) -> Vec<Loc> {
        let at_start = |path: PathBuf| Loc { path, line: 0, col: 0 };
        match r {
            Ref::Template(name) => self.template_dirs.iter().map(|d| d.join(name)).filter(|p| p.is_file()).map(at_start).collect(),
            Ref::Static(name) => self.static_dirs.iter().map(|d| d.join(name)).filter(|p| p.is_file()).map(at_start).collect(),
            Ref::Module(m) => scan::resolve_module(self, from, m).into_iter().map(at_start).collect(),
            Ref::Url(name) => {
                let exact: Vec<&Route> = self.routes.iter().filter(|r| r.name.as_deref() == Some(name.as_str())).collect();
                let found = if exact.is_empty() {
                    // `{% url 'detail' %}` inside an app, `url_for('index')` for `bp.index`.
                    self.routes.iter().filter(|r| r.name.as_deref().and_then(|n| n.rsplit([':', '.']).next()) == Some(name.as_str())).collect()
                } else {
                    exact
                };
                found.into_iter().map(|r| r.at.clone()).collect()
            }
        }
    }

    /// The names a template file is known by: `polls/detail.html` under `polls/templates/`.
    pub fn template_names(&self, file: &Path) -> Vec<String> {
        self.template_dirs.iter().filter_map(|d| file.strip_prefix(d).ok()).map(|rel| rel.to_string_lossy().replace('\\', "/")).collect()
    }

    /// Python files and templates that mention a template by name (`render(...)`,
    /// `template_name = ...`, `{% extends %}`, `{% include %}`): PyCharm's "related views".
    /// Views come first.
    pub fn template_usages(&self, file: &Path) -> Vec<Loc> {
        let names = self.template_names(file);
        if names.is_empty() {
            return Vec::new();
        }
        let mut out = Vec::new();
        for f in self.py_files.iter().chain(self.template_files.iter().filter(|f| f.as_path() != file)) {
            let Ok(text) = std::fs::read_to_string(f) else { continue };
            for name in &names {
                for q in ['"', '\''] {
                    let needle = format!("{q}{name}{q}");
                    let mut from = 0;
                    while let Some(i) = text[from..].find(&needle) {
                        let at = from + i + 1;
                        let (line, col) = line_col(&text, at);
                        out.push(Loc { path: f.clone(), line, col });
                        from = at;
                    }
                }
            }
        }
        out.sort_by(|a, b| {
            let py = |l: &Loc| l.path.extension().is_some_and(|e| e == "py");
            py(b).cmp(&py(a)).then_with(|| a.cmp(b))
        });
        out.dedup();
        out
    }
}

/// 0-based line and character column of a byte offset.
pub(crate) fn line_col(text: &str, byte: usize) -> (usize, usize) {
    let byte = byte.min(text.len());
    let before = &text[..byte];
    let line = before.matches('\n').count();
    let start = before.rfind('\n').map(|i| i + 1).unwrap_or(0);
    (line, before[start..].chars().count())
}

#[cfg(test)]
mod tests;
