//! Language server client, UI-free: JSON-RPC over the server's stdio, plus a typed API for
//! any server (diagnostics, definition, hover, completion, signature help, references,
//! rename) with adapters for ty (Python) and TypeScript 7 (JS / TS). Positions are UTF-32
//! (characters), the editor's own unit; UTF-16 servers are converted (see `encoding`).

use brisk_lint::{Diagnostic, Edit, Pos, Severity};
use futures::channel::oneshot;
use serde_json::{json, Value};
use std::collections::HashMap;
use std::future::Future;
use std::io::{BufRead, BufReader, Write};
use std::path::{Path, PathBuf};
use std::process::{Child, ChildStdin, Command, Stdio};
use std::sync::atomic::{AtomicBool, AtomicI64, Ordering};
use std::sync::{Arc, Mutex};

type Reply = Result<Value, String>;
type Pending = Arc<Mutex<HashMap<i64, oneshot::Sender<Reply>>>>;

/// A running language server.
pub struct Client {
    stdin: Mutex<ChildStdin>,
    pending: Pending,
    next: AtomicI64,
    child: Mutex<Child>,
    alive: Arc<AtomicBool>,
    /// The answer to `workspace/configuration` for the server's own section.
    config: Arc<Mutex<Value>>,
}

fn write_msg(w: &mut impl Write, v: &Value) -> std::io::Result<()> {
    let body = serde_json::to_vec(v)?;
    write!(w, "Content-Length: {}\r\n\r\n", body.len())?;
    w.write_all(&body)?;
    w.flush()
}

fn read_msg(r: &mut impl BufRead) -> Option<Value> {
    let mut len = None;
    loop {
        let mut line = String::new();
        if r.read_line(&mut line).ok()? == 0 {
            return None;
        }
        let line = line.trim();
        if line.is_empty() {
            break;
        }
        if let Some((k, v)) = line.split_once(':') {
            if k.eq_ignore_ascii_case("content-length") {
                len = v.trim().parse::<usize>().ok();
            }
        }
    }
    let mut body = vec![0u8; len?];
    r.read_exact(&mut body).ok()?;
    serde_json::from_slice(&body).ok()
}

impl Client {
    /// What the server gets when it asks for its configuration.
    pub fn set_config(&self, v: Value) {
        *self.config.lock().unwrap_or_else(|e| e.into_inner()) = v;
    }

    pub fn start(program: &Path, args: &[&str], cwd: &Path, env: &[(String, String)]) -> std::io::Result<Arc<Client>> {
        let mut c = Command::new(program);
        c.args(args).current_dir(cwd).stdin(Stdio::piped()).stdout(Stdio::piped()).stderr(Stdio::null());
        for (k, v) in env {
            c.env(k, v);
        }
        brisk_platform::process::no_window(&mut c);
        let mut child = c.spawn()?;
        brisk_platform::process::adopt(&child);
        let stdout = child.stdout.take().ok_or_else(|| std::io::Error::other("no stdout"))?;
        let stdin = child.stdin.take().ok_or_else(|| std::io::Error::other("no stdin"))?;
        let client = Arc::new(Client {
            stdin: Mutex::new(stdin),
            pending: Arc::default(),
            next: AtomicI64::new(1),
            child: Mutex::new(child),
            alive: Arc::new(AtomicBool::new(true)),
            config: Arc::new(Mutex::new(Value::Null)),
        });
        let weak = Arc::downgrade(&client);
        let (pending, alive, config) = (client.pending.clone(), client.alive.clone(), client.config.clone());
        std::thread::spawn(move || {
            let mut r = BufReader::new(stdout);
            while let Some(msg) = read_msg(&mut r) {
                let id = msg.get("id").cloned();
                match (msg.get("method").and_then(Value::as_str), id) {
                    // A response to one of our requests.
                    (None, Some(id)) => {
                        let Some(id) = id.as_i64() else { continue };
                        let tx = pending.lock().unwrap_or_else(|e| e.into_inner()).remove(&id);
                        if let Some(tx) = tx {
                            let reply = match msg.get("error") {
                                Some(e) => Err(e.get("message").and_then(Value::as_str).unwrap_or("language server error").to_string()),
                                None => Ok(msg.get("result").cloned().unwrap_or(Value::Null)),
                            };
                            let _ = tx.send(reply);
                        }
                    }
                    // A request from the server: answer so it never waits on us.
                    (Some(method), Some(id)) => {
                        let result =
                            match method {
                                "workspace/configuration" => {
                                    let items = msg.pointer("/params/items").and_then(Value::as_array).cloned().unwrap_or_default();
                                    let own = config.lock().unwrap_or_else(|e| e.into_inner()).clone();
                                    // Sections other than the server's own (`ty`) get nothing.
                                    Value::Array(
                                        items
                                            .iter()
                                            .map(|i| {
                                                if i.get("section").and_then(Value::as_str).is_some_and(|s| s.contains('.')) {
                                                    Value::Null
                                                } else {
                                                    own.clone()
                                                }
                                            })
                                            .collect(),
                                    )
                                }
                                _ => Value::Null,
                            };
                        if let Some(c) = weak.upgrade() {
                            c.send(json!({"jsonrpc": "2.0", "id": id, "result": result}));
                        }
                    }
                    // Notifications (progress, logs, pushed diagnostics): diagnostics are pulled.
                    _ => {}
                }
            }
            alive.store(false, Ordering::SeqCst);
            for (_, tx) in pending.lock().unwrap_or_else(|e| e.into_inner()).drain() {
                let _ = tx.send(Err("the language server exited".into()));
            }
        });
        Ok(client)
    }

    fn send(&self, v: Value) {
        let mut w = self.stdin.lock().unwrap_or_else(|e| e.into_inner());
        if write_msg(&mut *w, &v).is_err() {
            self.alive.store(false, Ordering::SeqCst);
        }
    }

    pub fn is_alive(&self) -> bool {
        self.alive.load(Ordering::SeqCst)
    }

    pub fn request(&self, method: &str, params: Value) -> oneshot::Receiver<Reply> {
        let (tx, rx) = oneshot::channel();
        if !self.is_alive() {
            let _ = tx.send(Err("the language server is not running".into()));
            return rx;
        }
        let id = self.next.fetch_add(1, Ordering::Relaxed);
        self.pending.lock().unwrap_or_else(|e| e.into_inner()).insert(id, tx);
        self.send(json!({"jsonrpc": "2.0", "id": id, "method": method, "params": params}));
        rx
    }

    pub fn notify(&self, method: &str, params: Value) {
        self.send(json!({"jsonrpc": "2.0", "method": method, "params": params}));
    }
}

impl Drop for Client {
    fn drop(&mut self) {
        self.notify("exit", Value::Null);
        let _ = self.child.lock().unwrap_or_else(|e| e.into_inner()).kill();
    }
}

// --- URIs ------------------------------------------------------------------------

pub fn path_to_uri(p: &Path) -> String {
    let s = p.to_string_lossy().replace('\\', "/");
    let s = if s.starts_with('/') { s } else { format!("/{s}") };
    let mut out = String::from("file://");
    for b in s.bytes() {
        if b.is_ascii_alphanumeric() || b"/:-._~".contains(&b) {
            out.push(b as char);
        } else {
            out.push_str(&format!("%{b:02X}"));
        }
    }
    out
}

pub fn uri_to_path(uri: &str) -> Option<PathBuf> {
    let rest = uri.strip_prefix("file://")?;
    let bytes = rest.as_bytes();
    let mut v = Vec::with_capacity(bytes.len());
    let mut i = 0;
    while i < bytes.len() {
        if bytes[i] == b'%' && i + 2 < bytes.len() {
            if let Some(b) = std::str::from_utf8(&bytes[i + 1..i + 3]).ok().and_then(|h| u8::from_str_radix(h, 16).ok()) {
                v.push(b);
                i += 3;
                continue;
            }
        }
        v.push(bytes[i]);
        i += 1;
    }
    let s = String::from_utf8(v).ok()?;
    if cfg!(windows) {
        // "/c:/x" -> "C:\x"
        let s = s.strip_prefix('/').filter(|t| t.as_bytes().get(1) == Some(&b':')).map(str::to_string).unwrap_or(s);
        let mut s = s.replace('/', "\\");
        if s.as_bytes().get(1) == Some(&b':') {
            s = s[..1].to_ascii_uppercase() + &s[1..];
        }
        Some(PathBuf::from(s))
    } else {
        Some(PathBuf::from(s))
    }
}

// --- Typed results -----------------------------------------------------------------

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Location {
    pub path: PathBuf,
    pub start: Pos,
    pub end: Pos,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct CompletionItem {
    pub label: String,
    /// Type or signature, shown dimmed on the right.
    pub detail: String,
    /// LSP CompletionItemKind (3 function, 6 variable, 7 class, 9 module, ...).
    pub kind: u32,
    pub sort: String,
    /// Text to insert in place of the word before the caret.
    pub insert: String,
    /// The server's own replacement range, when it gives one.
    pub range: Option<(Pos, Pos)>,
    /// Edits elsewhere in the file, applied with the item: the import line of an
    /// auto-import completion.
    pub extra: Vec<Edit>,
    /// Documentation shown beside the selected item.
    pub doc: String,
}

/// A quick fix (Alt+Enter) from the server.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct CodeAction {
    pub title: String,
    pub preferred: bool,
    pub edits: Vec<FileEdits>,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Signature {
    pub label: String,
    /// Character ranges of the parameters in `label`.
    pub params: Vec<std::ops::Range<usize>>,
    pub active: Option<usize>,
    pub doc: String,
    /// Every overload's label in order (empty with one signature); `label` is `overloads[current]`.
    pub overloads: Vec<String>,
    pub current: usize,
}

/// A file's edits from a rename.
pub type FileEdits = (PathBuf, Vec<Edit>);

/// Edits from several answers combined per file: duplicates and edits overlapping one
/// already taken are dropped (the wider edit at a position wins), so each file's edits can
/// be applied together.
pub fn merge_file_edits(groups: Vec<FileEdits>) -> Vec<FileEdits> {
    let mut by_file: Vec<FileEdits> = Vec::new();
    for (path, edits) in groups {
        match by_file.iter_mut().find(|(p, _)| *p == path) {
            Some((_, e)) => e.extend(edits),
            None => by_file.push((path, edits)),
        }
    }
    for (_, edits) in &mut by_file {
        edits.sort_by(|a, b| a.start.cmp(&b.start).then(b.end.cmp(&a.end)));
        let mut kept: Vec<Edit> = Vec::with_capacity(edits.len());
        for e in edits.drain(..) {
            if kept.last().is_some_and(|k| e.start < k.end || (e.start == k.start && e.end == k.end)) {
                continue;
            }
            kept.push(e);
        }
        *edits = kept;
    }
    by_file.retain(|(_, e)| !e.is_empty());
    by_file
}

fn pos(v: &Value) -> Pos {
    Pos { line: v["line"].as_u64().unwrap_or(0) as usize, col: v["character"].as_u64().unwrap_or(0) as usize }
}

fn lsp_pos(p: Pos) -> Value {
    json!({"line": p.line, "character": p.col})
}

fn location(v: &Value) -> Option<Location> {
    // Location or LocationLink.
    let uri = v.get("uri").or_else(|| v.get("targetUri"))?.as_str()?;
    let range = v.get("targetSelectionRange").or_else(|| v.get("range")).or_else(|| v.get("targetRange"))?;
    Some(Location { path: uri_to_path(uri)?, start: pos(&range["start"]), end: pos(&range["end"]) })
}

/// An inlay hint: an inferred type after a name (`: list[int]`) or a parameter name before
/// an argument (`a=`), drawn inside the line without being part of the text.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct InlayHint {
    pub pos: Pos,
    pub label: String,
    /// A parameter-name hint (LSP kind 2); otherwise a type hint.
    pub parameter: bool,
}

/// `textDocument/inlayHint` reply: labels may be a string or label parts.
pub fn parse_inlay_hints(v: &Value) -> Vec<InlayHint> {
    let Some(items) = v.as_array() else { return Vec::new() };
    let mut out: Vec<InlayHint> = items
        .iter()
        .filter_map(|h| {
            let label = match &h["label"] {
                Value::String(s) => s.clone(),
                Value::Array(parts) => parts.iter().filter_map(|p| p["value"].as_str()).collect(),
                _ => return None,
            };
            let mut label = label;
            if h["paddingLeft"].as_bool() == Some(true) {
                label.insert(0, ' ');
            }
            if h["paddingRight"].as_bool() == Some(true) {
                label.push(' ');
            }
            (!label.trim().is_empty()).then(|| InlayHint { pos: pos(&h["position"]), label, parameter: h["kind"].as_u64() == Some(2) })
        })
        .collect();
    out.sort_by_key(|h| h.pos);
    out
}

pub fn parse_locations(v: &Value) -> Vec<Location> {
    match v {
        Value::Array(a) => a.iter().filter_map(location).collect(),
        Value::Null => Vec::new(),
        one => location(one).into_iter().collect(),
    }
}

pub fn parse_diagnostics(path: &Path, v: &Value) -> Vec<Diagnostic> {
    let items = v.get("items").and_then(Value::as_array).cloned().unwrap_or_default();
    let mut out: Vec<Diagnostic> = items
        .iter()
        .map(|d| {
            let unnecessary = d["tags"].as_array().is_some_and(|t| t.iter().any(|t| t == 1));
            let severity = match (d["severity"].as_u64(), unnecessary) {
                (_, true) => Severity::Unused,
                (Some(1), _) => Severity::Error,
                (Some(3 | 4), _) => Severity::Unused,
                _ => Severity::Warning,
            };
            let code = match &d["code"] {
                Value::String(s) => s.clone(),
                Value::Number(n) => n.to_string(),
                _ => "ty".into(),
            };
            Diagnostic {
                file: path.to_path_buf(),
                code,
                message: d["message"].as_str().unwrap_or("").to_string(),
                severity,
                start: pos(&d["range"]["start"]),
                end: pos(&d["range"]["end"]),
                fix: None,
                noqa_line: None,
            }
        })
        .collect();
    out.sort_by_key(|d| (d.start, d.severity));
    out
}

fn doc_text(v: &Value) -> String {
    match v {
        Value::String(s) => s.clone(),
        Value::Object(o) => o.get("value").and_then(Value::as_str).unwrap_or("").to_string(),
        Value::Array(a) => a.iter().map(doc_text).collect::<Vec<_>>().join("\n\n"),
        _ => String::new(),
    }
}

pub fn parse_completion(v: &Value) -> Vec<CompletionItem> {
    let items = v.get("items").or(Some(v)).and_then(Value::as_array).cloned().unwrap_or_default();
    let mut out: Vec<CompletionItem> = items
        .iter()
        .map(|i| {
            let label = i["label"].as_str().unwrap_or("").to_string();
            let (insert, range) = match i.get("textEdit") {
                Some(te) => {
                    let r = te.get("range").or_else(|| te.get("replace")).cloned().unwrap_or(Value::Null);
                    (te["newText"].as_str().unwrap_or(&label).to_string(), (!r.is_null()).then(|| (pos(&r["start"]), pos(&r["end"]))))
                }
                None => (i["insertText"].as_str().unwrap_or(&label).to_string(), None),
            };
            let mut detail = i["detail"].as_str().or_else(|| i.pointer("/labelDetails/description").and_then(Value::as_str)).unwrap_or("").to_string();
            // ty labels auto-import items "OrderedDict (import collections)": show the name,
            // with the module on the right like PyCharm.
            let label = match label.strip_suffix(')').and_then(|l| l.split_once(" (import ")) {
                Some((name, module)) => {
                    if detail.is_empty() {
                        detail = module.to_string();
                    }
                    name.to_string()
                }
                None => label,
            };
            CompletionItem {
                extra: edits(&i["additionalTextEdits"]),
                doc: doc_text(&i["documentation"]),
                detail,
                kind: i["kind"].as_u64().unwrap_or(0) as u32,
                sort: i["sortText"].as_str().unwrap_or(&label).to_string(),
                label,
                insert,
                range,
            }
        })
        .collect();
    out.sort_by(|a, b| a.sort.cmp(&b.sort).then_with(|| a.label.cmp(&b.label)));
    out
}

pub fn parse_signature(v: &Value) -> Option<Signature> {
    let sigs = v.get("signatures")?.as_array()?;
    let current = (v["activeSignature"].as_u64().unwrap_or(0) as usize).min(sigs.len().saturating_sub(1));
    let s = sigs.get(current)?;
    let overloads: Vec<String> = if sigs.len() > 1 { sigs.iter().filter_map(|s| s["label"].as_str().map(String::from)).collect() } else { Vec::new() };
    let label = s["label"].as_str()?.to_string();
    let params = s["parameters"]
        .as_array()
        .map(|ps| {
            let mut from = 0;
            ps.iter()
                .filter_map(|p| match &p["label"] {
                    Value::String(t) => {
                        let at = label[from..].find(t.as_str())? + from;
                        from = at + t.len();
                        let start = label[..at].chars().count();
                        Some(start..start + t.chars().count())
                    }
                    Value::Array(a) => Some(a.first()?.as_u64()? as usize..a.get(1)?.as_u64()? as usize),
                    _ => None,
                })
                .collect()
        })
        .unwrap_or_default();
    let active = s.get("activeParameter").or(v.get("activeParameter")).and_then(Value::as_u64).map(|a| a as usize);
    Some(Signature { label, params, active, doc: doc_text(&s["documentation"]), overloads, current })
}

fn edits(v: &Value) -> Vec<Edit> {
    v.as_array()
        .into_iter()
        .flatten()
        .map(|e| Edit { start: pos(&e["range"]["start"]), end: pos(&e["range"]["end"]), content: e["newText"].as_str().unwrap_or("").to_string() })
        .collect()
}

pub fn parse_workspace_edit(v: &Value) -> Vec<FileEdits> {
    let mut by_file: Vec<FileEdits> = Vec::new();
    let mut add = |uri: &str, list: &Value| {
        let Some(path) = uri_to_path(uri) else { return };
        by_file.push((path, edits(list)));
    };
    if let Some(ch) = v.get("changes").and_then(Value::as_object) {
        for (uri, edits) in ch {
            add(uri, edits);
        }
    }
    for dc in v.get("documentChanges").and_then(Value::as_array).into_iter().flatten() {
        if let Some(uri) = dc.pointer("/textDocument/uri").and_then(Value::as_str) {
            add(uri, &dc["edits"]);
        }
    }
    by_file.sort_by(|a, b| a.0.cmp(&b.0));
    by_file
}

/// `textDocument/codeAction` reply: actions that carry an edit (commands are skipped).
pub fn parse_code_actions(v: &Value) -> Vec<CodeAction> {
    v.as_array()
        .into_iter()
        .flatten()
        .filter_map(|a| {
            let edits = parse_workspace_edit(a.get("edit")?);
            (!edits.is_empty()).then(|| CodeAction {
                title: a["title"].as_str().unwrap_or("").to_string(),
                preferred: a["isPreferred"].as_bool().unwrap_or(false),
                edits,
            })
        })
        .collect()
}

/// A diagnostic as the server reported it, for a code action request's context.
fn lsp_diagnostic(d: &Diagnostic) -> Value {
    let severity = match d.severity {
        Severity::Error => 1,
        Severity::Warning => 2,
        Severity::Unused => 4,
    };
    json!({"range": {"start": lsp_pos(d.start), "end": lsp_pos(d.end)}, "code": d.code, "message": d.message, "severity": severity, "source": "ty"})
}

/// Hover markdown split into what the documentation popup draws.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum DocBlock {
    Code(String),
    Text(String),
    Rule,
}

pub fn doc_blocks(md: &str) -> Vec<DocBlock> {
    let mut out = Vec::new();
    let mut lines = md.lines().peekable();
    let mut text = String::new();
    let flush = |text: &mut String, out: &mut Vec<DocBlock>| {
        let t = text.trim().to_string();
        if !t.is_empty() {
            out.push(DocBlock::Text(t));
        }
        text.clear();
    };
    while let Some(l) = lines.next() {
        if l.trim_start().starts_with("```") {
            flush(&mut text, &mut out);
            let mut code = Vec::new();
            for l in lines.by_ref() {
                if l.trim_start().starts_with("```") {
                    break;
                }
                code.push(l);
            }
            out.push(DocBlock::Code(code.join("\n")));
        } else if l.trim() == "---" || l.trim() == "***" {
            flush(&mut text, &mut out);
            out.push(DocBlock::Rule);
        } else {
            text.push_str(l);
            text.push('\n');
        }
    }
    flush(&mut text, &mut out);
    out
}

// --- Servers -------------------------------------------------------------------------

/// A language server for one project.
pub struct Server {
    client: Arc<Client>,
    /// For messages: "ty", "TypeScript".
    pub name: &'static str,
    enc: Encoding,
    /// Open documents' texts, for UTF-16 conversion.
    docs: encoding::Docs,
}

/// The Python server; the name the app grew up with.
pub type Ty = Server;

/// The LSP language id of a file.
pub fn language_id(path: &Path) -> &'static str {
    match path.extension().and_then(|e| e.to_str()).map(str::to_ascii_lowercase).as_deref() {
        Some("py" | "pyi" | "pyw") => "python",
        Some("ts" | "mts" | "cts") => "typescript",
        Some("tsx") => "typescriptreact",
        Some("js" | "mjs" | "cjs") => "javascript",
        Some("jsx") => "javascriptreact",
        _ => "plaintext",
    }
}

/// ty's settings for the folders marked in the project: extra import roots (source and test
/// roots) and excluded folders, relative to the project root.
pub fn ty_settings(extra_paths: &[std::path::PathBuf], exclude: &[String]) -> Value {
    if extra_paths.is_empty() && exclude.is_empty() {
        return Value::Null;
    }
    let mut conf = serde_json::Map::new();
    if !extra_paths.is_empty() {
        conf.insert("environment".into(), json!({"extra-paths": extra_paths.iter().map(|p| p.display().to_string()).collect::<Vec<_>>()}));
    }
    if !exclude.is_empty() {
        conf.insert("src".into(), json!({"exclude": exclude.iter().map(|d| format!("{d}/")).collect::<Vec<_>>()}));
    }
    json!({"configuration": conf})
}

fn exe(name: &str) -> String {
    brisk_platform::python::Layout::host().exe(name)
}

/// A tool installed next to a Python interpreter (`.venv\Scripts\ty.exe`); with `None`, on
/// PATH, else the copy bundled with Brisk.
pub fn find_tool(name: &str, python: Option<&Path>) -> Option<PathBuf> {
    if let Some(dir) = python.and_then(Path::parent) {
        if let Some(p) = brisk_platform::python::Layout::host().tool_dirs(dir).into_iter().map(|d| d.join(exe(name))).find(|p| p.is_file()) {
            return Some(p);
        }
    }
    if python.is_some() {
        return None;
    }
    let on_path = std::env::var_os("PATH").and_then(|path| std::env::split_paths(&path).map(|d| d.join(exe(name))).find(|p| p.is_file()));
    on_path.or_else(|| brisk_lint::bundled_tool(name))
}

fn text_doc(path: &Path) -> Value {
    json!({"uri": path_to_uri(path)})
}

fn at(path: &Path, p: Pos) -> Value {
    json!({"textDocument": text_doc(path), "position": lsp_pos(p)})
}

fn rename_params(renames: &[(PathBuf, PathBuf)]) -> Value {
    let files: Vec<Value> = renames.iter().map(|(a, b)| json!({"oldUri": path_to_uri(a), "newUri": path_to_uri(b)})).collect();
    json!({"files": files})
}

impl Server {
    /// Start `ty server` for the project and initialize it (blocks for the handshake, about
    /// 0.1 s: call it off the UI thread). `python` selects the environment ty resolves
    /// imports from.
    /// `settings` answers ty's `workspace/configuration` (see [`ty_settings`]).
    pub fn start(ty: &Path, root: &Path, python: Option<&Path>, settings: Value) -> Result<Ty, String> {
        let mut env = Vec::new();
        if let Some(py) = python {
            if let Some(dir) = py.parent() {
                let sep = if cfg!(windows) { ";" } else { ":" };
                env.push(("PATH".into(), format!("{}{sep}{}", dir.display(), std::env::var("PATH").unwrap_or_default())));
                if dir.parent().is_some_and(|v| v.join("pyvenv.cfg").is_file()) {
                    env.push(("VIRTUAL_ENV".into(), dir.parent().unwrap().display().to_string()));
                }
            }
        }
        let s = Self::launch("ty", ty, &["server"], root, &env, settings)?;
        if s.enc != Encoding::Utf32 {
            return Err("ty uses UTF-16 positions; Brisk needs utf-32 (update ty)".into());
        }
        Ok(s)
    }

    /// TypeScript 7's native server (`tsc --lsp --stdio`) for JS and TS.
    pub fn start_typescript(tsc: &Path, root: &Path) -> Result<Server, String> {
        Self::launch("TypeScript", tsc, &["--lsp", "--stdio"], root, &[], Value::Null)
    }

    /// Start a server and initialize it (blocks for the handshake: call it off the UI thread).
    pub fn launch(name: &'static str, program: &Path, args: &[&str], root: &Path, env: &[(String, String)], settings: Value) -> Result<Server, String> {
        let client = Client::start(program, args, root, env).map_err(|e| format!("cannot start {name}: {e}"))?;
        client.set_config(settings);
        let caps = json!({
            "general": {"positionEncodings": ["utf-32", "utf-16"]},
            "textDocument": {
                "synchronization": {"didSave": false},
                "diagnostic": {"dynamicRegistration": false},
                "hover": {"contentFormat": ["markdown", "plaintext"]},
                "inlayHint": {},
                "completion": {"completionItem": {"snippetSupport": false, "documentationFormat": ["markdown", "plaintext"]}},
                "codeAction": {"codeActionLiteralSupport": {"codeActionKind": {"valueSet": ["", "quickfix", "source", "source.organizeImports", "source.fixAll"]}}, "isPreferredSupport": true},
                "signatureHelp": {"signatureInformation": {"parameterInformation": {"labelOffsetSupport": true}}},
                "definition": {"linkSupport": false},
                "references": {},
                "rename": {"prepareSupport": true}
            },
            "workspace": {"configuration": true, "workspaceEdit": {"documentChanges": true}, "fileOperations": {"willRename": true, "didRename": true}}
        });
        let rx = client.request(
            "initialize",
            json!({"processId": std::process::id(), "rootUri": path_to_uri(root), "workspaceFolders": [{"uri": path_to_uri(root), "name": "project"}], "capabilities": caps}),
        );
        let init = futures::executor::block_on(rx).map_err(|_| format!("{name} exited during startup"))??;
        let enc = match init.pointer("/capabilities/positionEncoding").and_then(Value::as_str).unwrap_or("utf-16") {
            "utf-32" => Encoding::Utf32,
            "utf-16" => Encoding::Utf16,
            other => return Err(format!("{name} uses {other} positions, which Brisk does not support")),
        };
        client.notify("initialized", json!({}));
        Ok(Server { client, name, enc, docs: Arc::default() })
    }

    /// Send a request (positions converted for a UTF-16 server) and parse the reply.
    fn call<T: Send + 'static>(
        &self,
        method: &str,
        mut params: Value,
        parse: impl FnOnce(&Value) -> T + Send + 'static,
    ) -> impl Future<Output = Result<T, String>> + Send + 'static {
        let conv = (self.enc == Encoding::Utf16).then(|| self.docs.clone());
        let uri = params.pointer("/textDocument/uri").and_then(Value::as_str).unwrap_or_default().to_string();
        if let Some(d) = &conv {
            encoding::convert(&mut params, &uri, true, d);
        }
        let rx = self.client.request(method, params);
        async move {
            let mut v = rx.await.map_err(|_| "the language server exited".to_string())??;
            if let Some(d) = conv {
                encoding::convert(&mut v, &uri, false, &d);
            }
            Ok(parse(&v))
        }
    }

    /// Remembers an open document's text (UTF-16 servers: what its positions count in).
    fn keep_text(&self, path: &Path, text: Option<&str>) {
        if self.enc == Encoding::Utf16 {
            let mut d = self.docs.lock().unwrap_or_else(|e| e.into_inner());
            match text {
                Some(t) => d.insert(path.to_path_buf(), t.to_string()),
                None => d.remove(path),
            };
        }
    }

    pub fn is_alive(&self) -> bool {
        self.client.is_alive()
    }

    pub fn did_open(&self, path: &Path, version: u64, text: &str) {
        self.client.notify(
            "textDocument/didOpen",
            json!({"textDocument": {"uri": path_to_uri(path), "languageId": language_id(path), "version": version, "text": text}}),
        );
        self.keep_text(path, Some(text));
    }

    pub fn did_change(&self, path: &Path, version: u64, text: &str) {
        self.client
            .notify("textDocument/didChange", json!({"textDocument": {"uri": path_to_uri(path), "version": version}, "contentChanges": [{"text": text}]}));
        self.keep_text(path, Some(text));
    }

    /// Files changed on disk behind ty's back (a rename edited closed files).
    pub fn files_changed(&self, paths: &[PathBuf]) {
        self.watched(paths, 2);
    }

    /// Files that appeared or disappeared on disk. ty does not watch the disk itself: without
    /// this it never sees new modules (imports of them stay unresolved, moves miss them).
    pub fn files_created_deleted(&self, created: &[PathBuf], deleted: &[PathBuf]) {
        self.watched(created, 1);
        self.watched(deleted, 3);
    }

    fn watched(&self, paths: &[PathBuf], kind: u8) {
        if paths.is_empty() {
            return;
        }
        let changes: Vec<Value> = paths.iter().map(|p| json!({"uri": path_to_uri(p), "type": kind})).collect();
        self.client.notify("workspace/didChangeWatchedFiles", json!({"changes": changes}));
    }

    pub fn did_close(&self, path: &Path) {
        self.client.notify("textDocument/didClose", json!({"textDocument": text_doc(path)}));
        self.keep_text(path, None);
    }

    /// Inlay hints for lines `lines` of the document.
    pub fn inlay_hints(&self, path: &Path, lines: std::ops::Range<usize>) -> impl Future<Output = Result<Vec<InlayHint>, String>> + Send + 'static {
        let range = json!({"start": {"line": lines.start, "character": 0}, "end": {"line": lines.end, "character": 0}});
        self.call("textDocument/inlayHint", json!({"textDocument": text_doc(path), "range": range}), parse_inlay_hints)
    }

    pub fn diagnostics(&self, path: &Path) -> impl Future<Output = Result<Vec<Diagnostic>, String>> + Send + 'static {
        let p = path.to_path_buf();
        self.call("textDocument/diagnostic", json!({"textDocument": text_doc(path)}), move |v| parse_diagnostics(&p, v))
    }

    pub fn definition(&self, path: &Path, p: Pos) -> impl Future<Output = Result<Vec<Location>, String>> + Send + 'static {
        self.call("textDocument/definition", at(path, p), parse_locations)
    }

    pub fn type_definition(&self, path: &Path, p: Pos) -> impl Future<Output = Result<Vec<Location>, String>> + Send + 'static {
        self.call("textDocument/typeDefinition", at(path, p), parse_locations)
    }

    pub fn hover(&self, path: &Path, p: Pos) -> impl Future<Output = Result<Option<String>, String>> + Send + 'static {
        self.call("textDocument/hover", at(path, p), |v| {
            let t = doc_text(&v["contents"]);
            (!t.trim().is_empty()).then_some(t)
        })
    }

    pub fn completion(&self, path: &Path, p: Pos) -> impl Future<Output = Result<Vec<CompletionItem>, String>> + Send + 'static {
        self.call("textDocument/completion", at(path, p), parse_completion)
    }

    /// Completion and whether the list is incomplete (ty caps it at 1000 items): then the
    /// client asks again as the prefix grows.
    pub fn completion_list(&self, path: &Path, p: Pos) -> impl Future<Output = Result<(Vec<CompletionItem>, bool), String>> + Send + 'static {
        self.call("textDocument/completion", at(path, p), |v| (parse_completion(v), v["isIncomplete"].as_bool().unwrap_or(false)))
    }

    pub fn signature_help(&self, path: &Path, p: Pos) -> impl Future<Output = Result<Option<Signature>, String>> + Send + 'static {
        self.call("textDocument/signatureHelp", at(path, p), parse_signature)
    }

    pub fn references(&self, path: &Path, p: Pos) -> impl Future<Output = Result<Vec<Location>, String>> + Send + 'static {
        let mut params = at(path, p);
        params["context"] = json!({"includeDeclaration": true});
        self.call("textDocument/references", params, parse_locations)
    }

    /// The range of the symbol to rename, or `None` when it can't be renamed.
    pub fn prepare_rename(&self, path: &Path, p: Pos) -> impl Future<Output = Result<Option<(Pos, Pos)>, String>> + Send + 'static {
        self.call("textDocument/prepareRename", at(path, p), |v| {
            let r = v.get("range").unwrap_or(v);
            r.get("start").map(|s| (pos(s), pos(&r["end"])))
        })
    }

    /// Quick fixes for ty's problems in a range (auto-import, qualify, ignore).
    pub fn code_actions(
        &self,
        path: &Path,
        start: Pos,
        end: Pos,
        diags: &[Diagnostic],
    ) -> impl Future<Output = Result<Vec<CodeAction>, String>> + Send + 'static {
        let ctx: Vec<Value> = diags.iter().map(lsp_diagnostic).collect();
        let params =
            json!({"textDocument": text_doc(path), "range": {"start": lsp_pos(start), "end": lsp_pos(end)}, "context": {"diagnostics": ctx, "triggerKind": 1}});
        self.call("textDocument/codeAction", params, parse_code_actions)
    }

    /// Import updates for files or folders about to be renamed or moved (old, new).
    pub fn will_rename_files(&self, renames: &[(PathBuf, PathBuf)]) -> impl Future<Output = Result<Vec<FileEdits>, String>> + Send + 'static {
        self.call("workspace/willRenameFiles", rename_params(renames), parse_workspace_edit)
    }

    /// Files were renamed (after `will_rename_files` and the move on disk).
    pub fn did_rename_files(&self, renames: &[(PathBuf, PathBuf)]) {
        self.client.notify("workspace/didRenameFiles", rename_params(renames));
    }

    pub fn rename(&self, path: &Path, p: Pos, new_name: &str) -> impl Future<Output = Result<Vec<FileEdits>, String>> + Send + 'static {
        let mut params = at(path, p);
        params["newName"] = json!(new_name);
        self.call("textDocument/rename", params, parse_workspace_edit)
    }
}

#[cfg(test)]
mod tests;

mod encoding;
mod markdown;
pub mod typescript;
pub use encoding::Encoding;
mod module_rename;
pub use markdown::{parse_doc_text, unescape_md, DocText, InlineStyle};
pub use module_rename::{module_name, rename_module_refs};
