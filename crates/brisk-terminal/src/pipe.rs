//! Pipe backend for the Run console: the program's stdout and stderr go straight into the
//! log (no ConPTY, so nothing is lost when the process exits), stderr in red, input is
//! edited locally and sent a line at a time, like PyCharm's Run window.

use crate::log::Log;
use crate::{Callback, TermEvent};
use alacritty_terminal::sync::FairMutex;
use std::io::{Read, Write};
use std::process::{ChildStdin, Command, Stdio};
use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};
use std::sync::mpsc::Sender;
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

pub(crate) type SharedLog = Arc<FairMutex<Log>>;

/// Chunks of output (up to 16 KB each) on their way from the readers to the log: a process
/// that prints faster than the console takes it is held back by its pipe instead of filling
/// memory.
const IN_FLIGHT: usize = 64;

pub(crate) struct PipeIo {
    /// Lines for the stdin writer thread; `None` once input is closed (Ctrl+D or a dead pipe).
    stdin: Mutex<Option<Sender<Vec<u8>>>>,
    line: Mutex<String>,
    exited: Arc<AtomicBool>,
}

/// The log's lock is taken per 32 KB, so a backlog of output never keeps the UI (which
/// takes the same lock to draw) waiting for more than a moment.
fn feed(log: &SharedLog, bytes: &[u8]) {
    for chunk in bytes.chunks(32 * 1024) {
        log.lock().feed(chunk);
    }
}

/// Split off an incomplete UTF-8 sequence at the end so it is decoded with the next chunk.
fn complete_prefix(buf: &[u8]) -> usize {
    match std::str::from_utf8(buf) {
        Ok(_) => buf.len(),
        Err(e) if e.error_len().is_none() => e.valid_up_to(),
        Err(_) => buf.len(),
    }
}

/// Writes on its own thread: a program that does not read stdin fills the pipe, and a
/// blocking write on the UI thread would freeze the IDE. Dropping the sender closes stdin
/// once the queued lines are written.
fn stdin_writer(mut stdin: ChildStdin) -> Sender<Vec<u8>> {
    let (tx, rx) = std::sync::mpsc::channel::<Vec<u8>>();
    std::thread::spawn(move || {
        for data in rx {
            if stdin.write_all(&data).and_then(|_| stdin.flush()).is_err() {
                break;
            }
        }
    });
    tx
}

impl PipeIo {
    pub(crate) fn spawn(mut cmd: Command, header: &str, log: SharedLog, cb: Callback) -> std::io::Result<(Self, u32)> {
        cmd.stdin(Stdio::piped()).stdout(Stdio::piped()).stderr(Stdio::piped());
        brisk_platform::process::no_window(&mut cmd);
        #[cfg(unix)]
        {
            use std::os::unix::process::CommandExt;
            cmd.process_group(0);
        }
        feed(&log, format!("{header}\r\n").as_bytes());
        let mut child = cmd.spawn()?;
        brisk_platform::process::adopt(&child);
        let pid = child.id();
        // Readers only read; one writer thread orders output by whole lines, so a stderr line
        // never lands in the middle of a stdout line that `print()` wrote in pieces.
        let (chunk_tx, chunk_rx) = std::sync::mpsc::sync_channel::<(usize, Option<Vec<u8>>)>(IN_FLIGHT);
        let streams: [Option<Box<dyn Read + Send>>; 2] =
            [child.stdout.take().map(|s| Box::new(s) as Box<dyn Read + Send>), child.stderr.take().map(|s| Box::new(s) as Box<dyn Read + Send>)];
        let mut open = 0;
        // Streams still being read: once both are closed, the rest of the output is queued and
        // the exit waits for it to reach the console, however far behind the parser is.
        let reading = Arc::new(AtomicUsize::new(0));
        // Readers waiting for room in the channel: the console is behind, not the pipe idle.
        let sending = Arc::new(AtomicUsize::new(0));
        for (i, stream) in streams.into_iter().enumerate() {
            let Some(mut stream) = stream else { continue };
            open += 1;
            reading.fetch_add(1, Ordering::SeqCst);
            let tx = chunk_tx.clone();
            let (reading, sending) = (reading.clone(), sending.clone());
            std::thread::spawn(move || {
                let mut buf = vec![0u8; 16 * 1024];
                loop {
                    match stream.read(&mut buf) {
                        Ok(0) | Err(_) => break,
                        Ok(n) => {
                            sending.fetch_add(1, Ordering::SeqCst);
                            let sent = tx.send((i, Some(buf[..n].to_vec())));
                            sending.fetch_sub(1, Ordering::SeqCst);
                            if sent.is_err() {
                                return;
                            }
                        }
                    }
                }
                reading.fetch_sub(1, Ordering::SeqCst);
                let _ = tx.send((i, None));
            });
        }
        drop(chunk_tx);
        let (done_tx, done_rx) = std::sync::mpsc::channel::<()>();
        {
            let (log, cb) = (log.clone(), cb.clone());
            std::thread::spawn(move || {
                const PARTIAL: Duration = Duration::from_millis(30);
                let mut bufs: [Vec<u8>; 2] = [Vec::new(), Vec::new()];
                let mut since: [Option<Instant>; 2] = [None, None];
                let emit = |i: usize, bytes: &[u8]| {
                    if bytes.is_empty() {
                        return;
                    }
                    if i == 1 {
                        let mut v = b"\x1b[31m".to_vec();
                        v.extend_from_slice(bytes);
                        v.extend_from_slice(b"\x1b[39m");
                        feed(&log, &v);
                    } else {
                        feed(&log, bytes);
                    }
                    cb(TermEvent::Wakeup);
                };
                let mut eof = [false; 2];
                while open > 0 {
                    match chunk_rx.recv_timeout(PARTIAL) {
                        Ok((i, Some(b))) => {
                            if bufs[i].is_empty() {
                                since[i] = Some(Instant::now());
                            }
                            bufs[i].extend_from_slice(&b);
                        }
                        Ok((i, None)) => {
                            eof[i] = true;
                            open -= 1;
                        }
                        Err(std::sync::mpsc::RecvTimeoutError::Timeout) => {}
                        Err(std::sync::mpsc::RecvTimeoutError::Disconnected) => break,
                    }
                    // Emit in arrival order. A stream with an unfinished line (no newline yet,
                    // younger than PARTIAL) holds back the stream that started writing after
                    // it; a prompt without a newline or a `\r` progress bar shows after PARTIAL.
                    let mut order = [0usize, 1];
                    order.sort_by_key(|&i| since[i].unwrap_or_else(Instant::now));
                    for i in order {
                        if bufs[i].is_empty() {
                            continue;
                        }
                        if eof[i] {
                            let rest = std::mem::take(&mut bufs[i]);
                            emit(i, &rest);
                            since[i] = None;
                            continue;
                        }
                        if let Some(nl) = bufs[i].iter().rposition(|&c| c == b'\n') {
                            let lines: Vec<u8> = bufs[i].drain(..=nl).collect();
                            emit(i, &lines);
                            since[i] = (!bufs[i].is_empty()).then(Instant::now);
                        }
                        if bufs[i].is_empty() {
                            continue;
                        }
                        if since[i].is_some_and(|t| t.elapsed() >= PARTIAL) {
                            let k = complete_prefix(&bufs[i]);
                            let part: Vec<u8> = bufs[i].drain(..k).collect();
                            emit(i, &part);
                            since[i] = (!bufs[i].is_empty()).then(Instant::now);
                        } else {
                            break;
                        }
                    }
                }
                for (i, buf) in bufs.iter_mut().enumerate() {
                    let rest = std::mem::take(buf);
                    emit(i, &rest);
                }
                let _ = done_tx.send(());
            });
        }
        let exited = Arc::new(AtomicBool::new(false));
        let io = PipeIo { stdin: Mutex::new(child.stdin.take().map(stdin_writer)), line: Mutex::new(String::new()), exited: exited.clone() };
        std::thread::spawn(move || {
            let code = child.wait().ok().and_then(|s| s.code());
            exited.store(true, Ordering::SeqCst);
            // Normally the pipes close with the process. A detached grandchild may keep them
            // open: report the exit anyway after a second of the readers waiting on the pipes
            // (time they spend waiting for the console to catch up does not count) and let its
            // output keep coming.
            let mut idle = Duration::ZERO;
            let mut last = Instant::now();
            while reading.load(Ordering::SeqCst) > 0 && idle < Duration::from_secs(1) {
                std::thread::sleep(Duration::from_millis(5));
                let now = Instant::now();
                if sending.load(Ordering::SeqCst) == 0 {
                    idle += now - last;
                }
                last = now;
            }
            if reading.load(Ordering::SeqCst) == 0 {
                let _ = done_rx.recv();
            } else {
                let _ = done_rx.recv_timeout(Duration::from_millis(1));
            }
            cb(TermEvent::ChildExit(code));
            cb(TermEvent::Exit);
        });
        Ok((io, pid))
    }

    pub(crate) fn exited(&self) -> bool {
        self.exited.load(Ordering::SeqCst)
    }

    /// Keys from the console: printable text is echoed in green and kept until Enter.
    pub(crate) fn input(&self, log: &SharedLog, bytes: &[u8]) {
        let text = String::from_utf8_lossy(bytes);
        if text.starts_with('\x1b') {
            return; // arrows, function keys: no line editing beyond Backspace
        }
        let mut line = self.line.lock().unwrap_or_else(|e| e.into_inner());
        let mut echo = String::new();
        for c in text.chars() {
            match c {
                '\r' | '\n' => {
                    // Echo first, so the program's answer appears below the typed line.
                    echo.push_str("\r\n");
                    feed(log, std::mem::take(&mut echo).as_bytes());
                    let mut stdin = self.stdin.lock().unwrap_or_else(|e| e.into_inner());
                    if let Some(tx) = stdin.as_ref() {
                        if tx.send(format!("{line}\n").into_bytes()).is_err() {
                            *stdin = None;
                        }
                    }
                    line.clear();
                }
                '\x7f' | '\x08' => {
                    if line.pop().is_some() {
                        echo.push_str("\x08 \x08");
                    }
                }
                '\x04' => {
                    // Ctrl+D: end of input.
                    *self.stdin.lock().unwrap_or_else(|e| e.into_inner()) = None;
                }
                c if c == '\t' || !c.is_control() => {
                    line.push(c);
                    echo.push_str("\x1b[32m");
                    echo.push(c);
                    echo.push_str("\x1b[39m");
                }
                _ => {}
            }
        }
        drop(line);
        if !echo.is_empty() {
            feed(log, echo.as_bytes());
        }
    }
}
