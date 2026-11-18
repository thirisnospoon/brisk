//! Pipe backend for the Run console: the program's stdout and stderr go straight into the
//! emulator (no ConPTY, so nothing is lost when the process exits), stderr in red, input is
//! edited locally and sent a line at a time, like PyCharm's Run window.

use crate::{Callback, Listener, TermEvent};
use alacritty_terminal::sync::FairMutex;
use alacritty_terminal::vte::ansi::Processor;
use alacritty_terminal::Term;
use std::io::{Read, Write};
use std::process::{ChildStdin, Command, Stdio};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

type SharedTerm = Arc<FairMutex<Term<Listener>>>;

pub(crate) struct PipeIo {
    stdin: Mutex<Option<ChildStdin>>,
    line: Mutex<String>,
    parser: Arc<Mutex<Processor>>,
    exited: Arc<std::sync::atomic::AtomicBool>,
}

fn feed(term: &SharedTerm, parser: &Mutex<Processor>, bytes: &[u8]) {
    let mut p = parser.lock().unwrap_or_else(|e| e.into_inner());
    let mut t = term.lock();
    p.advance(&mut *t, bytes);
}

/// Program output: a bare LF also returns the carriage, as a tty would.
fn feed_output(term: &SharedTerm, parser: &Mutex<Processor>, bytes: &[u8], prev_cr: &mut bool) {
    let mut v = Vec::with_capacity(bytes.len() + 16);
    for &b in bytes {
        if b == b'\n' && !*prev_cr {
            v.push(b'\r');
        }
        *prev_cr = b == b'\r';
        v.push(b);
    }
    feed(term, parser, &v);
}

/// Split off an incomplete UTF-8 sequence at the end so it is decoded with the next chunk.
fn complete_prefix(buf: &[u8]) -> usize {
    match std::str::from_utf8(buf) {
        Ok(_) => buf.len(),
        Err(e) if e.error_len().is_none() => e.valid_up_to(),
        Err(_) => buf.len(),
    }
}

impl PipeIo {
    pub(crate) fn spawn(mut cmd: Command, header: &str, term: SharedTerm, cb: Callback) -> std::io::Result<(Self, u32)> {
        cmd.stdin(Stdio::piped()).stdout(Stdio::piped()).stderr(Stdio::piped());
        #[cfg(windows)]
        {
            use std::os::windows::process::CommandExt;
            cmd.creation_flags(0x0800_0000); // CREATE_NO_WINDOW
        }
        #[cfg(unix)]
        {
            use std::os::unix::process::CommandExt;
            cmd.process_group(0);
        }
        let parser = Arc::new(Mutex::new(Processor::new()));
        feed(&term, &parser, format!("{header}\r\n").as_bytes());
        let mut child = cmd.spawn()?;
        brisk_proc::adopt(&child);
        let pid = child.id();
        // Readers only read; one writer thread orders output by whole lines, so a stderr line
        // never lands in the middle of a stdout line that `print()` wrote in pieces.
        let (chunk_tx, chunk_rx) = std::sync::mpsc::channel::<(usize, Option<Vec<u8>>)>();
        let streams: [Option<Box<dyn Read + Send>>; 2] = [
            child.stdout.take().map(|s| Box::new(s) as Box<dyn Read + Send>),
            child.stderr.take().map(|s| Box::new(s) as Box<dyn Read + Send>),
        ];
        let mut open = 0;
        for (i, stream) in streams.into_iter().enumerate() {
            let Some(mut stream) = stream else { continue };
            open += 1;
            let tx = chunk_tx.clone();
            std::thread::spawn(move || {
                let mut buf = vec![0u8; 16 * 1024];
                loop {
                    match stream.read(&mut buf) {
                        Ok(0) | Err(_) => break,
                        Ok(n) => {
                            if tx.send((i, Some(buf[..n].to_vec()))).is_err() {
                                return;
                            }
                        }
                    }
                }
                let _ = tx.send((i, None));
            });
        }
        drop(chunk_tx);
        let (done_tx, done_rx) = std::sync::mpsc::channel::<()>();
        {
            let (term, parser, cb) = (term.clone(), parser.clone(), cb.clone());
            std::thread::spawn(move || {
                const PARTIAL: Duration = Duration::from_millis(30);
                let mut bufs: [Vec<u8>; 2] = [Vec::new(), Vec::new()];
                let mut since: [Option<Instant>; 2] = [None, None];
                let mut prev_cr = [false; 2];
                let emit = |i: usize, bytes: &[u8], prev_cr: &mut [bool; 2]| {
                    if bytes.is_empty() {
                        return;
                    }
                    if i == 1 {
                        let mut v = b"\x1b[31m".to_vec();
                        v.extend_from_slice(bytes);
                        v.extend_from_slice(b"\x1b[39m");
                        feed_output(&term, &parser, &v, &mut prev_cr[i]);
                    } else {
                        feed_output(&term, &parser, bytes, &mut prev_cr[i]);
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
                            emit(i, &rest, &mut prev_cr);
                            since[i] = None;
                            continue;
                        }
                        if let Some(nl) = bufs[i].iter().rposition(|&c| c == b'\n') {
                            let lines: Vec<u8> = bufs[i].drain(..=nl).collect();
                            emit(i, &lines, &mut prev_cr);
                            since[i] = (!bufs[i].is_empty()).then(Instant::now);
                        }
                        if bufs[i].is_empty() {
                            continue;
                        }
                        if since[i].is_some_and(|t| t.elapsed() >= PARTIAL) {
                            let k = complete_prefix(&bufs[i]);
                            let part: Vec<u8> = bufs[i].drain(..k).collect();
                            emit(i, &part, &mut prev_cr);
                            since[i] = (!bufs[i].is_empty()).then(Instant::now);
                        } else {
                            break;
                        }
                    }
                }
                for i in 0..2 {
                    let rest = std::mem::take(&mut bufs[i]);
                    emit(i, &rest, &mut prev_cr);
                }
                let _ = done_tx.send(());
            });
        }
        let exited = Arc::new(std::sync::atomic::AtomicBool::new(false));
        let io = PipeIo { stdin: Mutex::new(child.stdin.take()), line: Mutex::new(String::new()), parser, exited: exited.clone() };
        std::thread::spawn(move || {
            let code = child.wait().ok().and_then(|s| s.code());
            exited.store(true, std::sync::atomic::Ordering::SeqCst);
            // Normally the pipes close with the process. A detached grandchild may keep them
            // open: report the exit anyway after a moment and let its output keep coming.
            let _ = done_rx.recv_timeout(Duration::from_secs(1));
            cb(TermEvent::ChildExit(code));
            cb(TermEvent::Exit);
        });
        Ok((io, pid))
    }

    pub(crate) fn exited(&self) -> bool {
        self.exited.load(std::sync::atomic::Ordering::SeqCst)
    }

    /// Keys from the console: printable text is echoed in green and kept until Enter.
    pub(crate) fn input(&self, term: &SharedTerm, bytes: &[u8]) {
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
                    feed(term, &self.parser, std::mem::take(&mut echo).as_bytes());
                    let mut stdin = self.stdin.lock().unwrap_or_else(|e| e.into_inner());
                    if let Some(s) = stdin.as_mut() {
                        let data = format!("{line}\n");
                        if s.write_all(data.as_bytes()).and_then(|_| s.flush()).is_err() {
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
            feed(term, &self.parser, echo.as_bytes());
        }
    }
}
