//! The Python Console's process: the project interpreter running `py_console_boot.py`, with
//! JSON requests on stdin and output plus `\x1e<kind> <json>` events on stdout.

use std::io::{Read, Write};
use std::path::Path;
use std::process::{Child, ChildStdin, Stdio};
use std::sync::mpsc::{channel, Receiver, Sender};

const BOOT: &str = include_str!("py_console_boot.py");

#[derive(Clone, Debug, PartialEq)]
pub struct Var {
    pub name: String,
    pub ty: String,
    pub value: String,
    pub size: Option<usize>,
}

#[derive(Clone, Debug, PartialEq)]
pub enum Event {
    Ready(String),
    Out(String),
    Err(String),
    /// Answer to `check`: is the code a complete statement?
    Check(bool),
    /// `input(prompt)` waits for a line.
    Input(String),
    /// A `run` finished; `exit`: the code called `exit()`.
    Done { vars: Vec<Var>, exit: bool },
    /// The process ended (exit code).
    Exited(Option<i32>),
}

pub struct Console {
    child: Child,
    stdin: ChildStdin,
    rx: Receiver<Event>,
    exited: bool,
}

impl Console {
    pub fn start(python: &Path, cwd: &Path) -> std::io::Result<Self> {
        let mut cmd = std::process::Command::new(python);
        cmd.args(["-u", "-X", "utf8", "-c", BOOT])
            .current_dir(cwd)
            .env("PYTHONIOENCODING", "utf-8")
            .env("PYTHONUNBUFFERED", "1")
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped());
        #[cfg(windows)]
        {
            use std::os::windows::process::CommandExt;
            cmd.creation_flags(0x0800_0000);
        }
        let mut child = cmd.spawn()?;
        brisk_proc::adopt(&child);
        let stdin = child.stdin.take().expect("piped stdin");
        let stdout = child.stdout.take().expect("piped stdout");
        let stderr = child.stderr.take().expect("piped stderr");
        let (tx, rx) = channel();
        let tx2 = tx.clone();
        std::thread::spawn(move || read_stdout(stdout, tx));
        // C-level writes to the real stderr (extension modules, the interpreter itself).
        std::thread::spawn(move || {
            let mut r = stderr;
            let mut buf = [0u8; 4096];
            while let Ok(n) = r.read(&mut buf) {
                if n == 0 || tx2.send(Event::Err(String::from_utf8_lossy(&buf[..n]).into_owned())).is_err() {
                    break;
                }
            }
        });
        Ok(Self { child, stdin, rx, exited: false })
    }

    fn send(&mut self, v: serde_json::Value) -> std::io::Result<()> {
        let mut line = v.to_string();
        line.push('\n');
        self.stdin.write_all(line.as_bytes())?;
        self.stdin.flush()
    }

    pub fn run(&mut self, code: &str) -> std::io::Result<()> {
        self.send(serde_json::json!({ "op": "run", "code": code }))
    }

    pub fn check(&mut self, code: &str) -> std::io::Result<()> {
        self.send(serde_json::json!({ "op": "check", "code": code }))
    }

    pub fn input(&mut self, text: &str) -> std::io::Result<()> {
        self.send(serde_json::json!({ "op": "input", "text": text }))
    }

    /// KeyboardInterrupt in the running command; the namespace stays.
    pub fn interrupt(&mut self) -> std::io::Result<()> {
        self.send(serde_json::json!({ "op": "interrupt" }))
    }

    /// Events that arrived since the last call; `Exited` once the process is gone.
    pub fn events(&mut self) -> Vec<Event> {
        let mut v: Vec<Event> = self.rx.try_iter().collect();
        if !self.exited && matches!(self.child.try_wait(), Ok(Some(_))) {
            self.exited = true;
            // Output still in the pipe comes first.
            std::thread::sleep(std::time::Duration::from_millis(20));
            v.extend(self.rx.try_iter());
            let code = self.child.try_wait().ok().flatten().and_then(|s| s.code());
            v.push(Event::Exited(code));
        }
        v
    }

    pub fn kill(&mut self) {
        let _ = self.child.kill();
        let _ = self.child.wait();
    }
}

impl Drop for Console {
    fn drop(&mut self) {
        self.kill();
    }
}

fn read_stdout(mut r: impl Read, tx: Sender<Event>) {
    let mut pending: Vec<u8> = Vec::new();
    let mut buf = [0u8; 8192];
    loop {
        let n = match r.read(&mut buf) {
            Ok(0) | Err(_) => break,
            Ok(n) => n,
        };
        pending.extend_from_slice(&buf[..n]);
        for e in split_events(&mut pending) {
            if tx.send(e).is_err() {
                return;
            }
        }
    }
    if !pending.is_empty() {
        let _ = tx.send(Event::Out(String::from_utf8_lossy(&pending).into_owned()));
    }
}

/// Take the complete output and events off the front of `pending`; an unfinished event line
/// or a split UTF-8 character stays for the next read.
pub fn split_events(pending: &mut Vec<u8>) -> Vec<Event> {
    let mut out = Vec::new();
    loop {
        match pending.iter().position(|&b| b == 0x1e) {
            Some(0) => {
                let Some(nl) = pending.iter().position(|&b| b == b'\n') else { break };
                let line = String::from_utf8_lossy(&pending[1..nl]).into_owned();
                pending.drain(..=nl);
                if let Some(e) = parse_event(&line) {
                    out.push(e);
                }
            }
            Some(i) => {
                let text: Vec<u8> = pending.drain(..i).collect();
                out.push(Event::Out(String::from_utf8_lossy(&text).into_owned()));
            }
            None => {
                let valid = match std::str::from_utf8(pending) {
                    Ok(_) => pending.len(),
                    Err(e) if e.error_len().is_none() => e.valid_up_to(),
                    Err(_) => pending.len(),
                };
                if valid > 0 {
                    let text: Vec<u8> = pending.drain(..valid).collect();
                    out.push(Event::Out(String::from_utf8_lossy(&text).into_owned()));
                }
                break;
            }
        }
    }
    out
}

fn parse_event(line: &str) -> Option<Event> {
    let (kind, json) = line.split_once(' ')?;
    let v: serde_json::Value = serde_json::from_str(json).ok()?;
    Some(match kind {
        "ready" => Event::Ready(v["version"].as_str().unwrap_or("").to_string()),
        "err" => Event::Err(v.as_str().unwrap_or("").to_string()),
        "check" => Event::Check(v["complete"].as_bool().unwrap_or(true)),
        "input" => Event::Input(v.as_str().unwrap_or("").to_string()),
        "done" => Event::Done {
            exit: v["exit"].as_bool().unwrap_or(false),
            vars: v["vars"]
                .as_array()
                .map(|a| {
                    a.iter()
                        .filter_map(|x| {
                            Some(Var {
                                name: x[0].as_str()?.to_string(),
                                ty: x[1].as_str().unwrap_or("").to_string(),
                                value: x[2].as_str().unwrap_or("").to_string(),
                                size: x[3].as_u64().map(|n| n as usize),
                            })
                        })
                        .collect()
                })
                .unwrap_or_default(),
        },
        _ => return None,
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::time::{Duration, Instant};

    fn wait(c: &mut Console, until: impl Fn(&Event) -> bool) -> Vec<Event> {
        let start = Instant::now();
        let mut all = Vec::new();
        while start.elapsed() < Duration::from_secs(20) {
            let evs = c.events();
            let hit = evs.iter().any(&until);
            all.extend(evs);
            if hit {
                return all;
            }
            std::thread::sleep(Duration::from_millis(20));
        }
        panic!("timed out: {all:?}");
    }

    fn text(evs: &[Event], err: bool) -> String {
        evs.iter()
            .filter_map(|e| match e {
                Event::Out(s) if !err => Some(s.as_str()),
                Event::Err(s) if err => Some(s.as_str()),
                _ => None,
            })
            .collect()
    }

    #[test]
    fn events_are_split_from_output() {
        let mut p = b"hello \xd0".to_vec();
        assert_eq!(split_events(&mut p), vec![Event::Out("hello ".into())]);
        p.extend_from_slice(b"\xb6\n\x1echeck {\"complete\": false}\n\x1edone {\"vars\": [[\"x\", \"int\", \"1\", null]]}");
        assert_eq!(split_events(&mut p), vec![Event::Out("ж\n".into()), Event::Check(false)]);
        p.push(b'\n');
        let v = split_events(&mut p);
        assert_eq!(v, vec![Event::Done { vars: vec![Var { name: "x".into(), ty: "int".into(), value: "1".into(), size: None }], exit: false }]);
        assert!(p.is_empty());
    }

    /// Needs `python` on PATH; skipped otherwise.
    #[test]
    fn repl_roundtrip() {
        let dir = std::env::temp_dir().join(format!("brisk-console-{}", std::process::id()));
        let _ = std::fs::create_dir_all(&dir);
        // A project file shadowing a standard module must not break the console.
        std::fs::write(dir.join("types.py"), "raise SystemExit('shadowed')\n").unwrap();
        let Ok(mut c) = Console::start(Path::new("python"), &dir) else { return };
        wait(&mut c, |e| matches!(e, Event::Ready(_)));
        c.check("def f():").unwrap();
        assert!(wait(&mut c, |e| matches!(e, Event::Check(_))).contains(&Event::Check(false)));
        c.check("x = 41").unwrap();
        assert!(wait(&mut c, |e| matches!(e, Event::Check(_))).contains(&Event::Check(true)));
        c.run("x = 41\nprint('hi')\nx + 1").unwrap();
        let evs = wait(&mut c, |e| matches!(e, Event::Done { .. }));
        assert_eq!(text(&evs, false), "hi\n42\n");
        let Some(Event::Done { vars, .. }) = evs.last() else { panic!() };
        assert!(vars.iter().any(|v| v.name == "x" && v.ty == "int" && v.value == "41"), "{vars:?}");
        c.run("1/0").unwrap();
        let evs = wait(&mut c, |e| matches!(e, Event::Done { .. }));
        assert!(text(&evs, true).contains("ZeroDivisionError"), "{evs:?}");
        assert!(!text(&evs, true).contains("_run"), "the console's own frame is hidden: {evs:?}");
        c.run("name = input('who? ')").unwrap();
        let evs = wait(&mut c, |e| matches!(e, Event::Input(_)));
        assert!(evs.contains(&Event::Input("who? ".into())));
        c.input("Bob").unwrap();
        let evs = wait(&mut c, |e| matches!(e, Event::Done { .. }));
        let Some(Event::Done { vars, .. }) = evs.last() else { panic!() };
        assert!(vars.iter().any(|v| v.name == "name" && v.value == "'Bob'"), "{vars:?}");
        // Interrupt a sleeping loop: KeyboardInterrupt, and the session keeps its variables.
        c.run("import time\nfor k in range(100):\n    time.sleep(0.1)").unwrap();
        std::thread::sleep(Duration::from_millis(400));
        let t0 = Instant::now();
        c.interrupt().unwrap();
        let evs = wait(&mut c, |e| matches!(e, Event::Done { .. }));
        assert!(t0.elapsed() < Duration::from_secs(2), "interrupt took {:?}", t0.elapsed());
        assert!(text(&evs, true).contains("KeyboardInterrupt"), "{evs:?}");
        let Some(Event::Done { vars, .. }) = evs.last() else { panic!() };
        assert!(vars.iter().any(|v| v.name == "name"), "{vars:?}");
        // A blocking input() is interrupted too.
        c.run("input()").unwrap();
        wait(&mut c, |e| matches!(e, Event::Input(_)));
        c.interrupt().unwrap();
        let evs = wait(&mut c, |e| matches!(e, Event::Done { .. }));
        assert!(text(&evs, true).contains("KeyboardInterrupt"), "{evs:?}");
        c.run("x").unwrap();
        let evs = wait(&mut c, |e| matches!(e, Event::Done { .. }));
        assert_eq!(text(&evs, false), "41\n");
        c.run("exit()").unwrap();
        wait(&mut c, |e| matches!(e, Event::Exited(_)));
    }
}
