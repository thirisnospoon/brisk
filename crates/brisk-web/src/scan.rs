//! Building the [`Index`]: one pass over the project's files, a tree-sitter parse of the
//! Python files that can declare routes, then the routes joined across files (Django
//! `include()`, FastAPI `include_router`, Flask `register_blueprint`).

use crate::{line_col, ts, Framework, Index, Loc, Route};
use std::collections::{BTreeSet, HashMap};
use std::path::{Path, PathBuf};

/// Python files over this size are not read (generated code, data).
const MAX_PY: u64 = 1 << 20;

/// Decorator methods that declare a route, and the HTTP method they stand for.
fn deco_methods(m: &str) -> Option<Vec<String>> {
    Some(match m {
        "get" | "post" | "put" | "delete" | "patch" | "head" | "options" | "trace" => vec![m.to_ascii_uppercase()],
        "websocket" => vec!["WS".into()],
        // `methods=` decides; GET without it.
        "route" | "api_route" => Vec::new(),
        _ => return None,
    })
}

#[derive(Default)]
struct Obj {
    fw: Option<Framework>,
    prefix: String,
    /// A Flask blueprint's name (its endpoints are `name.func`).
    blueprint: Option<String>,
}

enum UrlTarget {
    View { handler: String, name: Option<String> },
    Include { module: String, namespace: Option<String> },
}

struct UrlEntry {
    route: String,
    target: UrlTarget,
    at: (usize, usize),
}

struct DecoRoute {
    obj: String,
    methods: Vec<String>,
    path: String,
    name: Option<String>,
    func: String,
    at: (usize, usize),
    def_at: (usize, usize),
}

struct Include {
    includer: String,
    expr: String,
    prefix: String,
}

#[derive(Default)]
struct FileInfo {
    path: PathBuf,
    src: String,
    /// `from X import a as b` → (b, X, Some(a)); `import X.Y as Z` → (Z, X.Y, None);
    /// `import X.Y` → (X, X, None).
    imports: Vec<(String, String, Option<String>)>,
    urls: Vec<UrlEntry>,
    has_urlpatterns: bool,
    app_name: Option<String>,
    root_urlconf: Option<String>,
    objs: HashMap<String, Obj>,
    deco: Vec<DecoRoute>,
    includes: Vec<Include>,
    template_dirs: Vec<String>,
}

pub fn scan(root: &Path) -> Index {
    let list = brisk_project::FileList::scan(root);
    let mut idx = Index { root: root.to_path_buf(), ..Default::default() };
    let (mut tdirs, mut sdirs) = (BTreeSet::new(), BTreeSet::new());
    for rel in &list.files {
        let parts: Vec<&str> = rel.split('/').collect();
        let dirs = &parts[..parts.len().saturating_sub(1)];
        if let Some(i) = dirs.iter().position(|p| *p == "templates") {
            tdirs.insert(root.join(parts[..=i].join("/")));
        }
        if let Some(i) = dirs.iter().position(|p| *p == "static") {
            sdirs.insert(root.join(parts[..=i].join("/")));
        }
        if rel.ends_with(".py") {
            idx.py_files.push(root.join(rel));
        }
    }
    let mut infos: Vec<FileInfo> = Vec::new();
    for p in &idx.py_files {
        if std::fs::metadata(p).map(|m| m.len() > MAX_PY).unwrap_or(true) {
            continue;
        }
        let Ok(src) = std::fs::read_to_string(p) else { continue };
        let lower = src.to_ascii_lowercase();
        let relevant = ["urlpatterns", "root_urlconf", "fastapi", "flask", "apirouter", "blueprint", "jinja2templates"].iter().any(|k| lower.contains(k));
        if relevant {
            if let Some(info) = read_file(p, src) {
                infos.push(info);
            }
        }
    }
    for info in &infos {
        for d in &info.template_dirs {
            for base in [root, info.path.parent().unwrap_or(root)] {
                let dir = base.join(d);
                if dir.is_dir() {
                    tdirs.insert(dir);
                }
            }
        }
    }
    idx.template_dirs = tdirs.into_iter().collect();
    idx.static_dirs = sdirs.into_iter().collect();
    idx.template_files = list.files.iter().map(|r| root.join(r)).filter(|p| idx.template_dirs.iter().any(|d| p.starts_with(d))).collect();
    let mut routes = django_routes(&idx, &infos);
    routes.extend(decorator_routes(&idx, &infos));
    routes.sort_by(|a, b| (a.framework, &a.path, &a.methods).cmp(&(b.framework, &b.path, &b.methods)));
    idx.routes = routes;
    idx
}

fn pos(src: &str, n: tree_sitter::Node) -> (usize, usize) {
    line_col(src, n.start_byte())
}

fn read_file(path: &Path, src: String) -> Option<FileInfo> {
    let tree = ts::parse(&src)?;
    let s = src.as_str();
    let mut info = FileInfo { path: path.to_path_buf(), has_urlpatterns: s.contains("urlpatterns"), ..Default::default() };
    let root = tree.root_node();
    let mut c = root.walk();
    for st in root.named_children(&mut c) {
        match st.kind() {
            "import_from_statement" => {
                let Some(m) = st.child_by_field_name("module_name") else { continue };
                let module = ts::text(m, s).to_string();
                let mut cc = st.walk();
                for n in st.children_by_field_name("name", &mut cc) {
                    let (name, alias) = match n.kind() {
                        "aliased_import" => (n.child_by_field_name("name"), n.child_by_field_name("alias")),
                        _ => (Some(n), Some(n)),
                    };
                    if let (Some(name), Some(alias)) = (name, alias) {
                        info.imports.push((ts::text(alias, s).to_string(), module.clone(), Some(ts::text(name, s).to_string())));
                    }
                }
            }
            "import_statement" => {
                let mut cc = st.walk();
                for n in st.children_by_field_name("name", &mut cc) {
                    if n.kind() == "aliased_import" {
                        if let (Some(name), Some(alias)) = (n.child_by_field_name("name"), n.child_by_field_name("alias")) {
                            info.imports.push((ts::text(alias, s).to_string(), ts::text(name, s).to_string(), None));
                        }
                    } else {
                        let full = ts::text(n, s).to_string();
                        let first = full.split('.').next().unwrap_or("").to_string();
                        info.imports.push((first.clone(), first, None));
                        if full.contains('.') {
                            info.imports.push((full.clone(), full, None));
                        }
                    }
                }
            }
            "expression_statement" => {
                let Some(a) = st.named_child(0).filter(|a| a.kind() == "assignment") else { continue };
                let (Some(left), Some(right)) = (a.child_by_field_name("left"), a.child_by_field_name("right")) else { continue };
                if left.kind() != "identifier" {
                    continue;
                }
                let var = ts::text(left, s).to_string();
                match var.as_str() {
                    "app_name" => info.app_name = ts::string_value(right, s),
                    "ROOT_URLCONF" => info.root_urlconf = ts::string_value(right, s),
                    _ => {}
                }
                if right.kind() != "call" {
                    continue;
                }
                let (pos_args, kw) = ts::args(right);
                let kw_str = |k: &str| ts::kwarg(&kw, k, s).and_then(|v| ts::string_value(v, s));
                let obj = match ts::call_name(right, s).unwrap_or("") {
                    "FastAPI" => Some(Obj { fw: Some(Framework::FastApi), ..Default::default() }),
                    "APIRouter" => Some(Obj { fw: Some(Framework::FastApi), prefix: kw_str("prefix").unwrap_or_default(), blueprint: None }),
                    "Flask" => {
                        info.template_dirs.extend(kw_str("template_folder"));
                        Some(Obj { fw: Some(Framework::Flask), ..Default::default() })
                    }
                    "Blueprint" => {
                        info.template_dirs.extend(kw_str("template_folder"));
                        let name = pos_args.first().and_then(|n| ts::string_value(*n, s)).or_else(|| kw_str("name"));
                        Some(Obj { fw: Some(Framework::Flask), prefix: kw_str("url_prefix").unwrap_or_default(), blueprint: name })
                    }
                    "Jinja2Templates" => {
                        info.template_dirs.extend(kw_str("directory").or_else(|| pos_args.first().and_then(|n| ts::string_value(*n, s))));
                        None
                    }
                    _ => None,
                };
                if let Some(o) = obj {
                    info.objs.insert(var, o);
                }
            }
            _ => {}
        }
    }
    let has_urlpatterns = info.has_urlpatterns;
    ts::walk(root, &mut |n| match n.kind() {
        "call" if has_urlpatterns => {
            if let Some(e) = url_entry(n, s) {
                info.urls.push(e);
            }
            include_edge(n, s, &mut info.includes);
        }
        "call" => include_edge(n, s, &mut info.includes),
        "decorated_definition" => deco_routes(n, s, &mut info.deco),
        _ => {}
    });
    info.src = src;
    Some(info)
}

/// `path("polls/", include("polls.urls"))`, `path("<int:pk>/", views.detail, name="detail")`.
fn url_entry(call: tree_sitter::Node, s: &str) -> Option<UrlEntry> {
    let name = ts::call_name(call, s)?;
    if !matches!(name, "path" | "re_path" | "url") {
        return None;
    }
    let (args, kw) = ts::args(call);
    let route = ts::string_value(*args.first()?, s)?;
    let route = if name == "path" { route } else { route.trim_start_matches('^').trim_end_matches('$').to_string() };
    let view = match args.get(1) {
        Some(v) => *v,
        None => ts::kwarg(&kw, "view", s)?,
    };
    let target = if view.kind() == "call" && ts::call_name(view, s) == Some("include") {
        let (iargs, ikw) = ts::args(view);
        let first = *iargs.first()?;
        let (module, ns) = match first.kind() {
            "tuple" => {
                let items = ts::string_list(first, s);
                (items.first()?.clone(), items.get(1).cloned())
            }
            "string" => (ts::string_value(first, s)?, None),
            _ => (ts::text(first, s).to_string(), None),
        };
        let namespace = ts::kwarg(&ikw, "namespace", s).and_then(|v| ts::string_value(v, s)).or(ns);
        UrlTarget::Include { module, namespace }
    } else {
        UrlTarget::View {
            handler: ts::text(view, s).split_whitespace().collect::<Vec<_>>().join(""),
            name: ts::kwarg(&kw, "name", s).and_then(|v| ts::string_value(v, s)),
        }
    };
    Some(UrlEntry { route, target, at: pos(s, call) })
}

/// `app.include_router(items.router, prefix="/items")`, `app.register_blueprint(bp, url_prefix="/auth")`.
fn include_edge(call: tree_sitter::Node, s: &str, out: &mut Vec<Include>) {
    let Some(name @ ("include_router" | "register_blueprint")) = ts::call_name(call, s) else { return };
    let Some(f) = call.child_by_field_name("function") else { return };
    let Some(obj) = f.child_by_field_name("object").filter(|o| o.kind() == "identifier") else { return };
    let (args, kw) = ts::args(call);
    let Some(first) = args.first() else { return };
    let key = if name == "include_router" { "prefix" } else { "url_prefix" };
    let prefix = ts::kwarg(&kw, key, s).and_then(|v| ts::string_value(v, s)).unwrap_or_default();
    out.push(Include { includer: ts::text(obj, s).to_string(), expr: ts::text(*first, s).to_string(), prefix });
}

/// `@app.get("/items/{id}")`, `@router.post(...)`, `@bp.route("/", methods=["GET", "POST"])`.
fn deco_routes(n: tree_sitter::Node, s: &str, out: &mut Vec<DecoRoute>) {
    let Some(def) = n.child_by_field_name("definition").filter(|d| d.kind() == "function_definition") else { return };
    let Some(fname) = def.child_by_field_name("name") else { return };
    let mut c = n.walk();
    for d in n.named_children(&mut c).filter(|d| d.kind() == "decorator") {
        let Some(call) = d.named_child(0).filter(|e| e.kind() == "call") else { continue };
        let Some(f) = call.child_by_field_name("function").filter(|f| f.kind() == "attribute") else { continue };
        let (Some(obj), Some(attr)) = (f.child_by_field_name("object"), f.child_by_field_name("attribute")) else { continue };
        if obj.kind() != "identifier" {
            continue;
        }
        let Some(mut methods) = deco_methods(ts::text(attr, s)) else { continue };
        let (args, kw) = ts::args(call);
        let Some(path) = args
            .first()
            .and_then(|a| ts::string_value(*a, s))
            .or_else(|| ["path", "rule"].iter().find_map(|k| ts::kwarg(&kw, k, s).and_then(|v| ts::string_value(v, s))))
        else {
            continue;
        };
        if methods.is_empty() {
            methods = ts::kwarg(&kw, "methods", s).map(|v| ts::string_list(v, s).into_iter().map(|m| m.to_ascii_uppercase()).collect()).unwrap_or_default();
            if methods.is_empty() {
                methods.push("GET".into());
            }
        }
        let name = ["name", "endpoint"].iter().find_map(|k| ts::kwarg(&kw, k, s).and_then(|v| ts::string_value(v, s)));
        out.push(DecoRoute {
            obj: ts::text(obj, s).to_string(),
            methods,
            path,
            name,
            func: ts::text(fname, s).to_string(),
            at: pos(s, d),
            def_at: pos(s, fname),
        });
    }
}

/// The file of a module: absolute (`polls.urls`, matched as a path suffix inside the
/// project, so a project under `src/` works) or relative to `from` (`.views`, `..core`).
pub fn resolve_module(idx: &Index, from: &Path, module: &str) -> Option<PathBuf> {
    let dots = module.chars().take_while(|c| *c == '.').count();
    let rel = module[dots..].replace('.', "/");
    let candidates = |base: &Path| -> Option<PathBuf> {
        let file = if rel.is_empty() { base.join("__init__.py") } else { base.join(format!("{rel}.py")) };
        let pkg = base.join(&rel).join("__init__.py");
        [file, pkg].into_iter().find(|p| p.is_file())
    };
    if dots > 0 {
        let mut base = from.parent()?;
        for _ in 1..dots {
            base = base.parent()?;
        }
        return candidates(base);
    }
    if rel.is_empty() {
        return None;
    }
    let (as_file, as_pkg) = (format!("/{rel}.py"), format!("/{rel}/__init__.py"));
    idx.py_files
        .iter()
        .filter(|p| {
            let r = p.strip_prefix(&idx.root).map(|r| format!("/{}", r.to_string_lossy().replace('\\', "/"))).unwrap_or_default();
            r.ends_with(&as_file) || r.ends_with(&as_pkg)
        })
        .min_by_key(|p| p.as_os_str().len())
        .cloned()
}

fn join_module(module: &str, name: &str) -> String {
    if module.ends_with('.') {
        format!("{module}{name}")
    } else {
        format!("{module}.{name}")
    }
}

/// What an expression in `info`'s file names: the file it lives in and its name there.
/// `items.router` with `from .routers import items` → (`routers/items.py`, `router`);
/// `views.detail` → (`views.py`, `detail`); a local name → (this file, name).
fn resolve_target(idx: &Index, info: &FileInfo, expr: &str) -> Option<(PathBuf, String)> {
    let expr = expr.split('(').next().unwrap_or(expr).trim();
    let expr = expr.strip_suffix(".as_view").unwrap_or(expr);
    let segs: Vec<&str> = expr.split('.').filter(|s| !s.is_empty()).collect();
    let first = *segs.first()?;
    let Some((_, module, name)) = info.imports.iter().rev().find(|(b, _, _)| b == first) else {
        return Some((info.path.clone(), first.to_string()));
    };
    let (file, rest): (PathBuf, Vec<&str>) = match name {
        Some(name) => match resolve_module(idx, &info.path, &join_module(module, name)) {
            Some(f) => (f, segs[1..].to_vec()),
            None => (resolve_module(idx, &info.path, module)?, std::iter::once(name.as_str()).chain(segs[1..].iter().copied()).collect()),
        },
        None => (resolve_module(idx, &info.path, module)?, segs[1..].to_vec()),
    };
    Some((file, rest.first()?.to_string()))
}

/// `def name` / `class name` in a file, top-level first.
fn find_def(path: &Path, name: &str) -> Option<Loc> {
    let src = std::fs::read_to_string(path).ok()?;
    let mut best: Option<(usize, Loc)> = None;
    for (i, line) in src.lines().enumerate() {
        let t = line.trim_start();
        let indent = line.len() - t.len();
        let t = t.strip_prefix("async ").unwrap_or(t);
        let Some(rest) = t.strip_prefix("def ").or_else(|| t.strip_prefix("class ")) else { continue };
        if rest.strip_prefix(name).is_some_and(|r| r.starts_with(['(', ':', ' '])) {
            let col = line[..line.len() - rest.len()].chars().count();
            if best.as_ref().is_none_or(|b| indent < b.0) {
                best = Some((indent, Loc { path: path.to_path_buf(), line: i, col }));
            }
        }
    }
    best.map(|b| b.1)
}

fn url_path(parts: &str) -> String {
    let mut p = format!("/{}", parts.trim_start_matches('/'));
    while p.contains("//") {
        p = p.replace("//", "/");
    }
    p
}

fn django_routes(idx: &Index, infos: &[FileInfo]) -> Vec<Route> {
    let by_path: HashMap<&Path, usize> = infos.iter().enumerate().map(|(i, f)| (f.path.as_path(), i)).collect();
    let url_files: Vec<usize> = (0..infos.len()).filter(|&i| infos[i].has_urlpatterns && !infos[i].urls.is_empty()).collect();
    let mut roots: Vec<usize> = infos
        .iter()
        .filter_map(|f| f.root_urlconf.as_deref())
        .filter_map(|m| resolve_module(idx, &idx.root, m))
        .filter_map(|p| by_path.get(p.as_path()).copied())
        .collect();
    let mut out = Vec::new();
    let mut walk = Walk { seen: vec![false; infos.len()], stack: vec![false; infos.len()] };
    // Then every urls file nobody includes (a project without settings, a second site).
    roots.extend(url_files.iter().copied());
    for r in roots {
        if !walk.seen[r] {
            walk_urls(idx, infos, &by_path, r, "", "", 0, &mut walk, &mut out);
        }
    }
    out
}

/// The urls files reached so far (not walked again as roots), and the ones on the current
/// include chain (an include cycle stops there). The same urlconf included under two
/// prefixes is walked once per prefix.
struct Walk {
    seen: Vec<bool>,
    stack: Vec<bool>,
}

#[allow(clippy::too_many_arguments)]
fn walk_urls(
    idx: &Index,
    infos: &[FileInfo],
    by_path: &HashMap<&Path, usize>,
    i: usize,
    prefix: &str,
    ns: &str,
    depth: usize,
    walk: &mut Walk,
    out: &mut Vec<Route>,
) {
    if depth > 8 || walk.stack[i] {
        return;
    }
    walk.seen[i] = true;
    walk.stack[i] = true;
    let info = &infos[i];
    for e in &info.urls {
        let at = Loc { path: info.path.clone(), line: e.at.0, col: e.at.1 };
        match &e.target {
            UrlTarget::View { handler, name } => {
                let name = name.as_ref().map(|n| if ns.is_empty() { n.clone() } else { format!("{ns}:{n}") });
                let handler_at = resolve_target(idx, info, handler).and_then(|(f, n)| find_def(&f, &n));
                out.push(Route {
                    framework: Framework::Django,
                    methods: Vec::new(),
                    path: url_path(&format!("{prefix}{}", e.route)),
                    name,
                    handler: handler.clone(),
                    at,
                    handler_at,
                });
            }
            UrlTarget::Include { module, namespace } => {
                let Some(child) = resolve_module(idx, &info.path, module).and_then(|p| by_path.get(p.as_path()).copied()) else { continue };
                let child_ns = namespace.clone().or_else(|| infos[child].app_name.clone());
                let ns = match (ns.is_empty(), child_ns) {
                    (_, None) => ns.to_string(),
                    (true, Some(c)) => c,
                    (false, Some(c)) => format!("{ns}:{c}"),
                };
                walk_urls(idx, infos, by_path, child, &format!("{prefix}{}", e.route), &ns, depth + 1, walk, out);
            }
        }
    }
    walk.stack[i] = false;
}

fn decorator_routes(idx: &Index, infos: &[FileInfo]) -> Vec<Route> {
    let by_path: HashMap<&Path, usize> = infos.iter().enumerate().map(|(i, f)| (f.path.as_path(), i)).collect();
    // The object a decorator is called on: defined in its file, or imported from another.
    let obj_of = |i: usize, var: &str| -> Option<(usize, String)> {
        if infos[i].objs.contains_key(var) {
            return Some((i, var.to_string()));
        }
        let (f, v) = resolve_target(idx, &infos[i], var)?;
        let j = *by_path.get(f.as_path())?;
        infos[j].objs.contains_key(&v).then_some((j, v))
    };
    fn prefix_of(idx: &Index, infos: &[FileInfo], by_path: &HashMap<&Path, usize>, i: usize, var: &str, depth: usize) -> String {
        let own = infos[i].objs.get(var).map(|o| o.prefix.clone()).unwrap_or_default();
        if depth > 4 {
            return own;
        }
        for (k, inc_file) in infos.iter().enumerate() {
            for inc in &inc_file.includes {
                let Some((f, v)) = resolve_target(idx, inc_file, &inc.expr) else { continue };
                if by_path.get(f.as_path()) == Some(&i) && v == var {
                    let outer = match inc_file.objs.contains_key(&inc.includer) {
                        true => prefix_of(idx, infos, by_path, k, &inc.includer, depth + 1),
                        false => String::new(),
                    };
                    return format!("{outer}{}{own}", inc.prefix);
                }
            }
        }
        own
    }
    let mut out = Vec::new();
    for (i, info) in infos.iter().enumerate() {
        let lower = info.src.to_ascii_lowercase();
        for d in &info.deco {
            let (fw, prefix, bp) = match obj_of(i, &d.obj) {
                Some((j, v)) => {
                    let o = &infos[j].objs[&v];
                    (o.fw, prefix_of(idx, infos, &by_path, j, &v, 0), o.blueprint.clone())
                }
                // An object Brisk cannot trace: the file's imports decide.
                None if lower.contains("fastapi") => (Some(Framework::FastApi), String::new(), None),
                None if lower.contains("flask") => (Some(Framework::Flask), String::new(), None),
                None => (None, String::new(), None),
            };
            let Some(fw) = fw else { continue };
            let base = d.name.clone().unwrap_or_else(|| d.func.clone());
            let name = match (&bp, fw) {
                (Some(b), Framework::Flask) => format!("{b}.{base}"),
                _ => base,
            };
            out.push(Route {
                framework: fw,
                methods: d.methods.clone(),
                path: url_path(&format!("{prefix}{}", d.path)),
                name: Some(name),
                handler: d.func.clone(),
                at: Loc { path: info.path.clone(), line: d.at.0, col: d.at.1 },
                handler_at: Some(Loc { path: info.path.clone(), line: d.def_at.0, col: d.def_at.1 }),
            });
        }
    }
    out
}
