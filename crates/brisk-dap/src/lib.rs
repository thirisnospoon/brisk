//! Debug Adapter Protocol client, UI-free, for debugpy (`python -m debugpy.adapter`):
//! requests with replies, events on a channel, and typed helpers for the parts the Debug
//! window shows (threads, stack, scopes, variables, evaluate). Lines are 1-based on the wire
//! and 0-based here.

use futures::channel::{mpsc, oneshot};
use serde_json::{json, Value};
use std::collections::HashMap;
use std::io::{BufRead, BufReader, Write};
use std::path::{Path, PathBuf};
use std::process::{Child, ChildStdin, Command, Stdio};
use std::sync::atomic::{AtomicBool, AtomicI64, Ordering};
use std::sync::{Arc, Mutex};

pub type Reply = Result<Value, String>;
type Pending = Arc<Mutex<HashMap<i64, oneshot::Sender<Reply>>>>;

/// An event from the adapter (`stopped`, `output`, `terminated`, …), or `Exited` when the
/// adapter itself went away.
#[derive(Clone, Debug)]
pub enum Event {
    Dap { event: String, body: Value },
    AdapterExited,
}

pub struct Client {
    stdin: Mutex<ChildStdin>,
    pending: Pending,
    next: AtomicI64,
    child: Mutex<Child>,
    alive: Arc<AtomicBool>,
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
            if len.is_some() {
                break;
            }
            continue;
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
    /// Start the adapter. Events arrive on the returned receiver.
    pub fn start(program: &Path, args: &[String], cwd: &Path, env: &[(String, String)]) -> std::io::Result<(Arc<Client>, mpsc::UnboundedReceiver<Event>)> {
        let mut c = Command::new(program);
        c.args(args).current_dir(cwd).stdin(Stdio::piped()).stdout(Stdio::piped()).stderr(Stdio::null());
        for (k, v) in env {
            c.env(k, v);
        }
        #[cfg(windows)]
        {
            use std::os::windows::process::CommandExt;
            c.creation_flags(0x0800_0000); // CREATE_NO_WINDOW
        }
        let mut child = c.spawn()?;
        let stdout = child.stdout.take().ok_or_else(|| std::io::Error::other("no stdout"))?;
        let stdin = child.stdin.take().ok_or_else(|| std::io::Error::other("no stdin"))?;
        let client = Arc::new(Client { stdin: Mutex::new(stdin), pending: Arc::default(), next: AtomicI64::new(1), child: Mutex::new(child), alive: Arc::new(AtomicBool::new(true)) });
        let (tx, rx) = mpsc::unbounded();
        let weak = Arc::downgrade(&client);
        let (pending, alive) = (client.pending.clone(), client.alive.clone());
        std::thread::spawn(move || {
            let mut r = BufReader::new(stdout);
            while let Some(msg) = read_msg(&mut r) {
                match msg.get("type").and_then(Value::as_str) {
                    Some("response") => {
                        let Some(id) = msg.get("request_seq").and_then(Value::as_i64) else { continue };
                        let waiter = pending.lock().unwrap_or_else(|e| e.into_inner()).remove(&id);
                        if let Some(w) = waiter {
                            let ok = msg.get("success").and_then(Value::as_bool).unwrap_or(false);
                            let reply = if ok {
                                Ok(msg.get("body").cloned().unwrap_or(Value::Null))
                            } else {
                                let detail = msg.pointer("/body/error/format").and_then(Value::as_str);
                                Err(detail.or_else(|| msg.get("message").and_then(Value::as_str)).unwrap_or("request failed").to_string())
                            };
                            let _ = w.send(reply);
                        }
                    }
                    Some("event") => {
                        let event = msg.get("event").and_then(Value::as_str).unwrap_or("").to_string();
                        let body = msg.get("body").cloned().unwrap_or(Value::Null);
                        let _ = tx.unbounded_send(Event::Dap { event, body });
                    }
                    // Reverse requests (runInTerminal, startDebugging): not supported, say so.
                    Some("request") => {
                        if let (Some(c), Some(seq)) = (weak.upgrade(), msg.get("seq").and_then(Value::as_i64)) {
                            let command = msg.get("command").cloned().unwrap_or(Value::Null);
                            c.send_raw(json!({"type": "response", "request_seq": seq, "success": false, "command": command, "message": "not supported"}));
                        }
                    }
                    _ => {}
                }
            }
            alive.store(false, Ordering::SeqCst);
            for (_, w) in pending.lock().unwrap_or_else(|e| e.into_inner()).drain() {
                let _ = w.send(Err("the debugger exited".into()));
            }
            let _ = tx.unbounded_send(Event::AdapterExited);
        });
        Ok((client, rx))
    }

    fn send_raw(&self, mut v: Value) {
        v["seq"] = json!(self.next.fetch_add(1, Ordering::Relaxed));
        let mut w = self.stdin.lock().unwrap_or_else(|e| e.into_inner());
        if write_msg(&mut *w, &v).is_err() {
            self.alive.store(false, Ordering::SeqCst);
        }
    }

    pub fn is_alive(&self) -> bool {
        self.alive.load(Ordering::SeqCst)
    }

    pub fn request(&self, command: &str, arguments: Value) -> oneshot::Receiver<Reply> {
        let (tx, rx) = oneshot::channel();
        if !self.is_alive() {
            let _ = tx.send(Err("the debugger is not running".into()));
            return rx;
        }
        let seq = self.next.fetch_add(1, Ordering::Relaxed);
        self.pending.lock().unwrap_or_else(|e| e.into_inner()).insert(seq, tx);
        let v = json!({"seq": seq, "type": "request", "command": command, "arguments": arguments});
        let mut w = self.stdin.lock().unwrap_or_else(|e| e.into_inner());
        if write_msg(&mut *w, &v).is_err() {
            self.alive.store(false, Ordering::SeqCst);
            if let Some(tx) = self.pending.lock().unwrap_or_else(|e| e.into_inner()).remove(&seq) {
                let _ = tx.send(Err("the debugger is not running".into()));
            }
        }
        rx
    }

    pub fn kill(&self) {
        let _ = self.child.lock().unwrap_or_else(|e| e.into_inner()).kill();
    }
}

impl Drop for Client {
    fn drop(&mut self) {
        self.kill();
    }
}

// --- Finding debugpy -------------------------------------------------------------

/// How to start the adapter: `python [-c bootstrap] -m debugpy.adapter`, with a bundled
/// copy put on `sys.path` when the interpreter has none.
#[derive(Clone, Debug, PartialEq)]
pub struct AdapterCommand {
    pub program: PathBuf,
    pub args: Vec<String>,
    /// Where the bundled copy lives (None: the interpreter's own debugpy).
    pub bundled: Option<PathBuf>,
}

/// Does `python` import debugpy by itself?
pub fn has_debugpy(python: &Path) -> bool {
    let mut c = Command::new(python);
    c.args(["-c", "import debugpy"]).stdin(Stdio::null()).stdout(Stdio::null()).stderr(Stdio::null());
    #[cfg(windows)]
    {
        use std::os::windows::process::CommandExt;
        c.creation_flags(0x0800_0000);
    }
    c.status().is_ok_and(|s| s.success())
}

/// A folder that holds a `debugpy` package: `BRISK_DEBUGPY`, `<exe dir>/lib`, or
/// `vendor/debugpy-lib` above the executable (development builds).
pub fn bundled_debugpy() -> Option<PathBuf> {
    let ok = |d: &Path| d.join("debugpy").join("__init__.py").is_file();
    if let Some(d) = std::env::var_os("BRISK_DEBUGPY").map(PathBuf::from) {
        if ok(&d) {
            return Some(d);
        }
    }
    let exe = std::env::current_exe().ok()?;
    let dir = exe.parent()?;
    if ok(&dir.join("lib")) {
        return Some(dir.join("lib"));
    }
    dir.ancestors().map(|a| a.join("vendor").join("debugpy-lib")).find(|d| ok(d))
}

pub fn adapter_command(python: &Path) -> Result<AdapterCommand, String> {
    if std::env::var_os("BRISK_DEBUGPY").is_none() && has_debugpy(python) {
        return Ok(AdapterCommand { program: python.to_path_buf(), args: vec!["-m".into(), "debugpy.adapter".into()], bundled: None });
    }
    let lib = bundled_debugpy().ok_or_else(|| "debugpy was not found: run `pip install debugpy` in the project interpreter".to_string())?;
    // `-c` instead of PYTHONPATH, so the program being debugged keeps its own environment.
    let boot = format!(
        "import sys, runpy; sys.path.insert(0, {:?}); sys.argv = ['debugpy.adapter']; runpy.run_module('debugpy.adapter', run_name='__main__', alter_sys=True)",
        lib.to_string_lossy()
    );
    Ok(AdapterCommand { program: python.to_path_buf(), args: vec!["-c".into(), boot], bundled: Some(lib) })
}

// --- Requests ----------------------------------------------------------------------

pub fn initialize_args() -> Value {
    json!({
        "clientID": "brisk",
        "clientName": "Brisk",
        "adapterID": "debugpy",
        "pathFormat": "path",
        "linesStartAt1": true,
        "columnsStartAt1": true,
        "supportsVariableType": true,
        "supportsRunInTerminalRequest": false,
        "locale": "en"
    })
}

/// What to debug.
#[derive(Clone, Debug, Default)]
pub struct Launch {
    pub python: PathBuf,
    pub python_args: Vec<String>,
    /// A script path, or a module with `module = true`.
    pub target: String,
    pub module: bool,
    pub args: Vec<String>,
    pub cwd: PathBuf,
    pub env: Vec<(String, String)>,
    pub just_my_code: bool,
}

pub fn launch_args(l: &Launch) -> Value {
    let mut python = vec![l.python.to_string_lossy().to_string()];
    python.extend(l.python_args.iter().cloned());
    let env: serde_json::Map<String, Value> = l.env.iter().map(|(k, v)| (k.clone(), json!(v))).collect();
    let mut v = json!({
        "name": "Brisk",
        "type": "python",
        "request": "launch",
        "python": python,
        "args": l.args,
        "cwd": l.cwd.to_string_lossy(),
        "env": env,
        "console": "internalConsole",
        "redirectOutput": true,
        "justMyCode": l.just_my_code,
        "showReturnValue": true,
        "subProcess": false,
    });
    if l.module {
        v["module"] = json!(l.target);
    } else {
        v["program"] = json!(l.target);
    }
    v
}

/// Attach to a running Python process: debugpy injects itself (`debugpy --pid`).
pub fn attach_args(pid: u32, just_my_code: bool) -> Value {
    json!({
        "name": "Brisk",
        "type": "python",
        "request": "attach",
        "processId": pid,
        "justMyCode": just_my_code,
        "showReturnValue": true,
        "redirectOutput": true,
        "subProcess": false,
    })
}

/// Python processes in `ps`-like lines (`pid<TAB>command line`), without debugpy's own
/// (adapters, launchers, programs it started) and without `own`.
pub fn python_processes(lines: &str, own: u32) -> Vec<(u32, String)> {
    let mut out = Vec::new();
    for l in lines.lines() {
        let Some((pid, cmd)) = l.trim().split_once(char::is_whitespace) else { continue };
        let Ok(pid) = pid.trim().parse::<u32>() else { continue };
        let cmd = cmd.trim();
        let exe = match cmd.strip_prefix('"') {
            Some(q) => q.split('"').next().unwrap_or(""),
            None => cmd.split(char::is_whitespace).next().unwrap_or(""),
        };
        let name = exe.rsplit(['/', '\\']).next().unwrap_or("").to_ascii_lowercase();
        if pid == own || !name.starts_with("python") || cmd.contains("debugpy") {
            continue;
        }
        out.push((pid, cmd.to_string()));
    }
    out
}

/// One line breakpoint: 0-based line and an optional condition.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct SourceBreakpoint {
    pub line: usize,
    pub condition: Option<String>,
}

pub fn set_breakpoints_args(path: &Path, bps: &[SourceBreakpoint]) -> Value {
    let list: Vec<Value> = bps
        .iter()
        .map(|b| {
            let mut v = json!({"line": b.line + 1});
            if let Some(c) = b.condition.as_ref().filter(|c| !c.trim().is_empty()) {
                v["condition"] = json!(c);
            }
            v
        })
        .collect();
    json!({"source": {"path": path.to_string_lossy()}, "breakpoints": list})
}

/// Verified lines from a `setBreakpoints` reply (0-based; None = not verified).
pub fn parse_breakpoints(v: &Value) -> Vec<Option<usize>> {
    v.get("breakpoints")
        .and_then(Value::as_array)
        .map(|a| {
            a.iter()
                .map(|b| {
                    let ok = b.get("verified").and_then(Value::as_bool).unwrap_or(false);
                    ok.then(|| b.get("line").and_then(Value::as_u64).map(|l| l.saturating_sub(1) as usize)).flatten()
                })
                .collect()
        })
        .unwrap_or_default()
}

#[derive(Clone, Debug, PartialEq)]
pub struct Frame {
    pub id: i64,
    pub name: String,
    pub path: Option<PathBuf>,
    /// 0-based.
    pub line: usize,
}

pub fn parse_frames(v: &Value) -> Vec<Frame> {
    v.get("stackFrames")
        .and_then(Value::as_array)
        .map(|a| {
            a.iter()
                .map(|f| Frame {
                    id: f.get("id").and_then(Value::as_i64).unwrap_or(0),
                    name: f.get("name").and_then(Value::as_str).unwrap_or("?").to_string(),
                    path: f.pointer("/source/path").and_then(Value::as_str).map(PathBuf::from),
                    line: f.get("line").and_then(Value::as_u64).unwrap_or(1).saturating_sub(1) as usize,
                })
                .collect()
        })
        .unwrap_or_default()
}

#[derive(Clone, Debug, PartialEq)]
pub struct Variable {
    pub name: String,
    pub value: String,
    pub type_name: String,
    /// > 0: has children (`variables` request with this reference).
    pub reference: i64,
}

/// Scopes (`scopes` reply) as variables with children, or a `variables` reply.
pub fn parse_scopes(v: &Value) -> Vec<Variable> {
    v.get("scopes")
        .and_then(Value::as_array)
        .map(|a| {
            a.iter()
                .map(|s| Variable {
                    name: s.get("name").and_then(Value::as_str).unwrap_or("?").to_string(),
                    value: String::new(),
                    type_name: String::new(),
                    reference: s.get("variablesReference").and_then(Value::as_i64).unwrap_or(0),
                })
                .collect()
        })
        .unwrap_or_default()
}

pub fn parse_variables(v: &Value) -> Vec<Variable> {
    v.get("variables")
        .and_then(Value::as_array)
        .map(|a| {
            a.iter()
                .map(|x| Variable {
                    name: x.get("name").and_then(Value::as_str).unwrap_or("?").to_string(),
                    value: x.get("value").and_then(Value::as_str).unwrap_or("").to_string(),
                    type_name: x.get("type").and_then(Value::as_str).unwrap_or("").to_string(),
                    reference: x.get("variablesReference").and_then(Value::as_i64).unwrap_or(0),
                })
                // debugpy groups "special variables" / "function variables": keep them, last.
                .collect()
        })
        .unwrap_or_default()
}

/// `evaluate` reply: (result, children reference).
pub fn parse_evaluate(v: &Value) -> (String, i64) {
    (v.get("result").and_then(Value::as_str).unwrap_or("").to_string(), v.get("variablesReference").and_then(Value::as_i64).unwrap_or(0))
}

#[cfg(test)]
mod tests;
