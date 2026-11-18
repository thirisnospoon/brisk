//! Terminal sessions: PTY (ConPTY on Windows, openpty on Unix) + VT emulation via
//! `alacritty_terminal`. UI-free: the app takes [`Snapshot`]s and sends input bytes.

mod keys;
mod pipe;

pub use keys::{encode_key, KeyMods};

use alacritty_terminal::event::{Event, EventListener, WindowSize};
use alacritty_terminal::event_loop::{EventLoop, EventLoopSender, Msg};
use alacritty_terminal::grid::{Dimensions, Scroll};
use alacritty_terminal::index::{Column, Line, Point, Side};
use alacritty_terminal::selection::{Selection, SelectionType};
use alacritty_terminal::sync::FairMutex;
use alacritty_terminal::term::cell::Flags;
use alacritty_terminal::term::{Config, TermMode};
use alacritty_terminal::tty::{self, Options, Shell};
use alacritty_terminal::vte::ansi::{Color, NamedColor, Rgb};
use alacritty_terminal::Term;
use std::borrow::Cow;
use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::sync::{Arc, OnceLock};

/// Events the UI cares about.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum TermEvent {
    /// New output; redraw.
    Wakeup,
    Title(String),
    Bell,
    /// The child process exited with this code (sent before [`TermEvent::Exit`]).
    ChildExit(Option<i32>),
    /// The PTY is closed and all output has been read.
    Exit,
}

type Callback = Arc<dyn Fn(TermEvent) + Send + Sync>;

#[derive(Clone)]
pub struct Listener {
    callback: Callback,
    // Set right after the event loop is created; used to answer terminal queries (DSR etc.).
    sender: Arc<OnceLock<EventLoopSender>>,
}

impl EventListener for Listener {
    fn send_event(&self, event: Event) {
        match event {
            Event::Wakeup => (self.callback)(TermEvent::Wakeup),
            Event::Title(t) => (self.callback)(TermEvent::Title(t)),
            Event::ResetTitle => (self.callback)(TermEvent::Title(String::new())),
            Event::Bell => (self.callback)(TermEvent::Bell),
            Event::ChildExit(status) => (self.callback)(TermEvent::ChildExit(status.code())),
            Event::Exit => (self.callback)(TermEvent::Exit),
            Event::PtyWrite(s) => {
                if let Some(tx) = self.sender.get() {
                    let _ = tx.send(Msg::Input(Cow::Owned(s.into_bytes())));
                }
            }
            _ => {}
        }
    }
}

#[derive(Clone, Copy, Debug)]
struct Size {
    cols: usize,
    rows: usize,
}

impl Dimensions for Size {
    fn total_lines(&self) -> usize {
        self.rows
    }
    fn screen_lines(&self) -> usize {
        self.rows
    }
    fn columns(&self) -> usize {
        self.cols
    }
}

#[derive(Clone, Debug, Default)]
pub struct SpawnOptions {
    /// Program and args; `None` = platform default (PowerShell on Windows, $SHELL on Unix).
    pub shell: Option<(String, Vec<String>)>,
    pub cwd: Option<PathBuf>,
    pub env: HashMap<String, String>,
}

impl SpawnOptions {
    /// PyCharm-like defaults: start in the project root and activate its virtualenv
    /// by putting `.venv` first on PATH (works for every shell, no activate script needed).
    pub fn for_project(root: &Path) -> Self {
        let mut env = HashMap::new();
        env.insert("TERM".into(), "xterm-256color".into());
        env.insert("COLORTERM".into(), "truecolor".into());
        env.insert("TERMINAL_EMULATOR".into(), "Brisk".into());
        for name in [".venv", "venv"] {
            let venv = root.join(name);
            let bin = if cfg!(windows) { venv.join("Scripts") } else { venv.join("bin") };
            if bin.is_dir() {
                let sep = if cfg!(windows) { ";" } else { ":" };
                let old = std::env::var("PATH").unwrap_or_default();
                env.insert("PATH".into(), format!("{}{sep}{old}", bin.display()));
                env.insert("VIRTUAL_ENV".into(), venv.display().to_string());
                break;
            }
        }
        Self { shell: default_shell(), cwd: Some(root.to_path_buf()), env }
    }
}

pub fn default_shell() -> Option<(String, Vec<String>)> {
    if cfg!(windows) {
        Some(("powershell.exe".into(), vec!["-NoLogo".into()]))
    } else {
        None // alacritty uses $SHELL / passwd entry
    }
}

enum Io {
    Pty(EventLoopSender),
    /// Plain pipes (PyCharm's Run console without "Emulate terminal"): exact output, stderr
    /// in red, line-buffered input echoed locally.
    Pipe(pipe::PipeIo),
    /// No process: a saved run shown again (input goes nowhere).
    None,
}

/// How many lines of a Run console's output are kept: beyond it the oldest lines are dropped
/// (the buffer is a ring), so a chatty process cannot grow memory without bound.
/// Lines a Run console keeps above the screen. A line costs about 24 bytes per column
/// (2.7 KB at 110 columns), so 100k lines took ~275 MB per tab; 20k is ~55 MB.
pub const RUN_HISTORY_LINES: usize = 20_000;

/// One terminal tab or one running process.
pub struct Session {
    term: Arc<FairMutex<Term<Listener>>>,
    io: Io,
    size: Size,
    pid: Option<u32>,
}

/// A cell ready for painting. Colors are resolved to RGB.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct CellView {
    pub c: char,
    pub fg: [u8; 3],
    pub bg: Option<[u8; 3]>,
    pub bold: bool,
    pub italic: bool,
    pub underline: bool,
    pub wide: bool,
    pub selected: bool,
    /// Part of an OSC 8 hyperlink (see [`Session::hyperlink_at`]).
    pub link: bool,
}

/// A search match. Matching goes over logical lines (rows joined where the output was
/// soft-wrapped), so a match may continue on the next rows.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Found {
    /// Grid line of the match's start: negative in the history, `0..rows` on the screen (as
    /// `display_offset` 0).
    pub line: i32,
    /// Columns of the match on `line`.
    pub cols: std::ops::Range<usize>,
    /// The rest of a match that continues on the following rows: `(line, columns)`.
    pub more: Vec<(i32, std::ops::Range<usize>)>,
    /// First row of the logical line.
    pub row: i32,
    /// The whole logical line, trailing blanks trimmed.
    pub text: String,
}

impl Found {
    /// Every row piece of the match: `(line, columns)`.
    pub fn parts(&self) -> impl Iterator<Item = (i32, std::ops::Range<usize>)> + '_ {
        std::iter::once((self.line, self.cols.clone())).chain(self.more.iter().cloned())
    }
}

/// One grid row for [`find_in_rows`]: its line, its `(column, char)` cells and whether it
/// continues on the next row (soft wrap).
pub struct SearchRow {
    pub line: i32,
    pub cells: Vec<(usize, char)>,
    pub wrapped: bool,
}

/// Matches of `needle` (case-insensitive unless `case`) in rows joined into logical lines.
pub fn find_in_rows(rows: impl IntoIterator<Item = SearchRow>, needle: &str, case: bool) -> Vec<Found> {
    let fold = |c: char| if case { c } else { c.to_lowercase().next().unwrap_or(c) };
    let pat: Vec<char> = needle.chars().map(fold).collect();
    let mut out = Vec::new();
    if pat.is_empty() {
        return out;
    }
    // (line, column, char) of the logical line being collected.
    let mut logical: Vec<(i32, usize, char)> = Vec::new();
    let mut search = |logical: &mut Vec<(i32, usize, char)>| {
        if logical.is_empty() {
            return;
        }
        let chars: Vec<char> = logical.iter().map(|&(_, _, c)| fold(c)).collect();
        let text: String = logical.iter().map(|&(_, _, c)| c).collect::<String>().trim_end().to_string();
        let row = logical[0].0;
        let mut i = 0;
        while i + pat.len() <= chars.len() {
            if chars[i..i + pat.len()] != pat[..] {
                i += 1;
                continue;
            }
            // Split the match into row pieces.
            let mut parts: Vec<(i32, std::ops::Range<usize>)> = Vec::new();
            for &(l, c, _) in &logical[i..i + pat.len()] {
                match parts.last_mut() {
                    Some((pl, r)) if *pl == l => r.end = c + 1,
                    _ => parts.push((l, c..c + 1)),
                }
            }
            let (line, cols) = parts.remove(0);
            out.push(Found { line, cols, more: parts, row, text: text.clone() });
            i += pat.len();
        }
        logical.clear();
    };
    for r in rows {
        logical.extend(r.cells.iter().map(|&(c, ch)| (r.line, c, ch)));
        if !r.wrapped {
            search(&mut logical);
        }
    }
    search(&mut logical);
    out
}

#[derive(Clone, Debug)]
pub struct Snapshot {
    pub cols: usize,
    pub rows: usize,
    /// Row-major, `rows * cols`. Spacer cells of wide characters are `'\0'`.
    pub cells: Vec<CellView>,
    /// (row, col) on screen, if visible.
    pub cursor: Option<(usize, usize)>,
    pub display_offset: usize,
    pub alt_screen: bool,
}

impl Session {
    pub fn spawn(
        opts: SpawnOptions,
        cols: usize,
        rows: usize,
        cell_w: u16,
        cell_h: u16,
        on_event: impl Fn(TermEvent) + Send + Sync + 'static,
    ) -> std::io::Result<Self> {
        Self::spawn_pty(opts, cols, rows, cell_w, cell_h, 10_000, "", on_event)
    }

    /// A Run process through a pseudo-terminal (PyCharm's "Emulate terminal in output
    /// console"): like [`Session::spawn_process`], `header` (the command line) comes first.
    pub fn spawn_process_pty(opts: SpawnOptions, header: &str, cols: usize, rows: usize, on_event: impl Fn(TermEvent) + Send + Sync + 'static) -> std::io::Result<Self> {
        Self::spawn_pty(opts, cols, rows, 8, 16, RUN_HISTORY_LINES, header, on_event)
    }

    #[allow(clippy::too_many_arguments)]
    fn spawn_pty(
        opts: SpawnOptions,
        cols: usize,
        rows: usize,
        cell_w: u16,
        cell_h: u16,
        history: usize,
        header: &str,
        on_event: impl Fn(TermEvent) + Send + Sync + 'static,
    ) -> std::io::Result<Self> {
        let size = Size { cols: cols.max(2), rows: rows.max(1) };
        let listener = Listener { callback: Arc::new(on_event), sender: Arc::new(OnceLock::new()) };
        let config = Config { scrolling_history: history, ..Config::default() };
        let term = Arc::new(FairMutex::new(Term::new(config, &size, listener.clone())));
        if !header.is_empty() {
            let mut p: alacritty_terminal::vte::ansi::Processor = alacritty_terminal::vte::ansi::Processor::new();
            p.advance(&mut *term.lock(), format!("{header}\r\n").as_bytes());
        }

        let pty_opts = Options {
            shell: opts.shell.map(|(p, a)| Shell::new(p, a)),
            working_directory: opts.cwd,
            drain_on_exit: true,
            env: opts.env,
            #[cfg(windows)]
            escape_args: true,
        };
        let ws = WindowSize { num_lines: size.rows as u16, num_cols: size.cols as u16, cell_width: cell_w, cell_height: cell_h };
        let pty = tty::new(&pty_opts, ws, 0)?;
        #[cfg(windows)]
        let pid = pty.child_watcher().pid().map(|p| p.get());
        #[cfg(windows)]
        if let Some(pid) = pid {
            brisk_proc::adopt_pid(pid);
        }
        #[cfg(unix)]
        let pid = Some(pty.child().id());
        let event_loop = EventLoop::new(term.clone(), listener.clone(), pty, true, false)?;
        let tx = event_loop.channel();
        let _ = listener.sender.set(tx.clone());
        event_loop.spawn();
        Ok(Self { term, io: Io::Pty(tx), size, pid })
    }

    pub fn write(&self, bytes: impl Into<Cow<'static, [u8]>>) {
        let bytes = bytes.into();
        if bytes.is_empty() {
            return;
        }
        // Typing jumps back to the live screen, like every terminal.
        self.term.lock().scroll_display(Scroll::Bottom);
        match &self.io {
            Io::Pty(tx) => {
                let _ = tx.send(Msg::Input(bytes));
            }
            Io::Pipe(p) => p.input(&self.term, &bytes),
            Io::None => {}
        }
    }

    /// Paste text, honoring bracketed paste mode.
    pub fn paste(&self, text: &str) {
        let bracketed = self.term.lock().mode().contains(TermMode::BRACKETED_PASTE);
        let text = text.replace("\r\n", "\r").replace('\n', "\r");
        if bracketed {
            let clean = text.replace('\x1b', "");
            self.write(format!("\x1b[200~{clean}\x1b[201~").into_bytes());
        } else {
            self.write(text.into_bytes());
        }
    }

    pub fn resize(&mut self, cols: usize, rows: usize, cell_w: u16, cell_h: u16) {
        let cols = cols.max(2);
        let rows = rows.max(1);
        if cols == self.size.cols && rows == self.size.rows {
            return;
        }
        self.size = Size { cols, rows };
        self.term.lock().resize(self.size);
        if let Io::Pty(tx) = &self.io {
            let _ = tx.send(Msg::Resize(WindowSize { num_lines: rows as u16, num_cols: cols as u16, cell_width: cell_w, cell_height: cell_h }));
        }
    }

    pub fn size(&self) -> (usize, usize) {
        (self.size.cols, self.size.rows)
    }

    pub fn scroll(&self, lines: i32) {
        self.term.lock().scroll_display(Scroll::Delta(lines));
    }

    pub fn app_cursor(&self) -> bool {
        self.term.lock().mode().contains(TermMode::APP_CURSOR)
    }

    pub fn alt_screen(&self) -> bool {
        self.term.lock().mode().contains(TermMode::ALT_SCREEN)
    }

    fn grid_point(&self, row: usize, col: usize) -> Point {
        let offset = self.term.lock().grid().display_offset() as i32;
        Point::new(Line(row as i32 - offset), Column(col.min(self.size.cols - 1)))
    }

    pub fn start_selection(&self, row: usize, col: usize, kind: SelectKind) {
        let p = self.grid_point(row, col);
        let ty = match kind {
            SelectKind::Simple => SelectionType::Simple,
            SelectKind::Word => SelectionType::Semantic,
            SelectKind::Line => SelectionType::Lines,
        };
        self.term.lock().selection = Some(Selection::new(ty, p, Side::Left));
    }

    pub fn update_selection(&self, row: usize, col: usize, right_half: bool) {
        let p = self.grid_point(row, col);
        if let Some(s) = self.term.lock().selection.as_mut() {
            s.update(p, if right_half { Side::Right } else { Side::Left });
        }
    }

    pub fn clear_selection(&self) {
        self.term.lock().selection = None;
    }

    pub fn selection_text(&self) -> Option<String> {
        self.term.lock().selection_to_string().filter(|s| !s.is_empty())
    }

    /// Process id of the child.
    pub fn pid(&self) -> Option<u32> {
        self.pid
    }

    /// Kill the child and its whole process tree (Stop in the Run window).
    pub fn kill(&self) {
        let Some(pid) = self.pid else { return };
        // Never signal a finished process: its pid may already belong to someone else.
        if let Io::Pipe(p) = &self.io {
            if p.exited() {
                return;
            }
        }
        #[cfg(windows)]
        {
            use std::os::windows::process::CommandExt;
            let _ = std::process::Command::new("taskkill")
                .args(["/PID", &pid.to_string(), "/T", "/F"])
                .stdout(std::process::Stdio::null())
                .stderr(std::process::Stdio::null())
                .creation_flags(0x0800_0000) // CREATE_NO_WINDOW
                .status();
        }
        #[cfg(unix)]
        {
            // The child is a session leader on its PTY: signal the whole group.
            let _ = std::process::Command::new("kill").args(["-TERM", "--", &format!("-{pid}")]).status();
        }
    }

    /// Start a program with pipes instead of a PTY and render its output in an emulator.
    /// `header` is printed first (PyCharm shows the command line there).
    pub fn spawn_process(
        cmd: std::process::Command,
        header: &str,
        cols: usize,
        rows: usize,
        on_event: impl Fn(TermEvent) + Send + Sync + 'static,
    ) -> std::io::Result<Self> {
        let size = Size { cols: cols.max(2), rows: rows.max(1) };
        let listener = Listener { callback: Arc::new(on_event), sender: Arc::new(OnceLock::new()) };
        let config = Config { scrolling_history: RUN_HISTORY_LINES, ..Config::default() };
        let term = Arc::new(FairMutex::new(Term::new(config, &size, listener.clone())));
        let (io, pid) = pipe::PipeIo::spawn(cmd, header, term.clone(), listener.callback.clone())?;
        Ok(Self { term, io: Io::Pipe(io), size, pid: Some(pid) })
    }

    /// A console with `text` and no process behind it (a run from the Test History).
    pub fn offline(text: &str, cols: usize, rows: usize) -> Self {
        let size = Size { cols: cols.max(2), rows: rows.max(1) };
        let listener = Listener { callback: Arc::new(|_| {}), sender: Arc::new(OnceLock::new()) };
        let config = Config { scrolling_history: RUN_HISTORY_LINES, ..Config::default() };
        let term = Arc::new(FairMutex::new(Term::new(config, &size, listener)));
        let s = Self { term, io: Io::None, size, pid: None };
        s.inject(format!("{}\x1b[?25l", text.replace('\n', "\r\n")).as_bytes());
        s
    }

    /// Feed bytes to the emulator as if the program printed them (IDE messages such as
    /// "Process finished with exit code 0"). Use only after the child has exited.
    pub fn inject(&self, bytes: &[u8]) {
        let mut term = self.term.lock();
        let mut p: alacritty_terminal::vte::ansi::Processor = alacritty_terminal::vte::ansi::Processor::new();
        p.advance(&mut *term, bytes);
        term.scroll_display(Scroll::Bottom);
    }

    /// (lines scrolled back from the bottom, screen lines, lines above the screen).
    pub fn scroll_pos(&self) -> (usize, usize, usize) {
        let term = self.term.lock();
        let g = term.grid();
        (g.display_offset(), g.screen_lines(), g.history_size())
    }

    /// Lines kept above the screen; at [`RUN_HISTORY_LINES`] the oldest output is being dropped.
    pub fn history_lines(&self) -> usize {
        self.term.lock().grid().history_size()
    }

    /// Plain text of the whole buffer (history and screen), trailing blanks trimmed.
    pub fn text(&self) -> String {
        let term = self.term.lock();
        let grid = term.grid();
        let top = -(grid.history_size() as i32);
        let bottom = grid.screen_lines() as i32;
        let mut out = String::new();
        for l in top..bottom {
            let row = &grid[Line(l)];
            let mut line = String::new();
            for c in 0..grid.columns() {
                let cell = &row[Column(c)];
                if !cell.flags.intersects(Flags::WIDE_CHAR_SPACER | Flags::LEADING_WIDE_CHAR_SPACER) {
                    line.push(cell.c);
                }
            }
            out.push_str(line.trim_end());
            if !row[Column(grid.columns() - 1)].flags.contains(Flags::WRAPLINE) {
                out.push('\n');
            }
        }
        out.trim_end().to_string()
    }

    /// Matches of `needle` in the whole buffer (history and screen), top to bottom.
    pub fn find(&self, needle: &str, case: bool) -> Vec<Found> {
        let term = self.term.lock();
        let grid = term.grid();
        let top = -(grid.history_size() as i32);
        let rows = (top..grid.screen_lines() as i32).map(|l| {
            let row = &grid[Line(l)];
            let cells = (0..grid.columns()).filter_map(|c| {
                let cell = &row[Column(c)];
                (!cell.flags.intersects(Flags::WIDE_CHAR_SPACER | Flags::LEADING_WIDE_CHAR_SPACER)).then_some((c, cell.c))
            });
            let wrapped = row[Column(grid.columns() - 1)].flags.contains(Flags::WRAPLINE);
            SearchRow { line: l, cells: cells.collect(), wrapped }
        });
        find_in_rows(rows, needle, case)
    }

    /// Scroll so that grid `line` is visible (centered when it was off screen).
    pub fn scroll_to(&self, line: i32) {
        let mut term = self.term.lock();
        let (offset, rows, history) = {
            let g = term.grid();
            (g.display_offset() as i32, g.screen_lines() as i32, g.history_size() as i32)
        };
        if line >= -offset && line < rows - offset {
            return;
        }
        let target = (rows / 2 - line).clamp(0, history);
        term.scroll_display(Scroll::Delta(target - offset));
    }

    pub fn shutdown(&self) {
        match &self.io {
            Io::Pty(tx) => {
                let _ = tx.send(Msg::Shutdown);
            }
            Io::Pipe(_) => self.kill(),
            Io::None => {}
        }
    }

    /// The OSC 8 hyperlink (`ESC ]8;;uri ESC \\ text ESC ]8;; ESC \\`) under a screen cell.
    pub fn hyperlink_at(&self, row: usize, col: usize) -> Option<String> {
        let term = self.term.lock();
        let grid = term.grid();
        let line = Line(row as i32 - grid.display_offset() as i32);
        if col >= term.columns() || row >= term.screen_lines() {
            return None;
        }
        grid[Point::new(line, Column(col))].hyperlink().map(|h| h.uri().to_string())
    }

    /// Copy the visible screen. Holds the lock only for the copy.
    pub fn snapshot(&self) -> Snapshot {
        let term = self.term.lock();
        let content = term.renderable_content();
        let cols = term.columns();
        let rows = term.screen_lines();
        let offset = content.display_offset as i32;
        let palette = Palette { overrides: content.colors };
        let blank = CellView { c: ' ', fg: palette.named(NamedColor::Foreground), bg: None, bold: false, italic: false, underline: false, wide: false, selected: false, link: false };
        let mut cells = vec![blank; cols * rows];
        let sel = content.selection;
        for ind in content.display_iter {
            let row = ind.point.line.0 + offset;
            if row < 0 || row as usize >= rows {
                continue;
            }
            let (row, col) = (row as usize, ind.point.column.0);
            if col >= cols {
                continue;
            }
            let cell = ind.cell;
            let f = cell.flags;
            let mut fg = palette.resolve(cell.fg, f.contains(Flags::BOLD), f.contains(Flags::DIM));
            let mut bg = match cell.bg {
                Color::Named(NamedColor::Background) => None,
                c => Some(palette.resolve(c, false, false)),
            };
            if f.contains(Flags::INVERSE) {
                let b = bg.unwrap_or(palette.named(NamedColor::Background));
                bg = Some(fg);
                fg = b;
            }
            let selected = sel.is_some_and(|s| {
                let p = ind.point;
                if s.is_block {
                    p.line >= s.start.line && p.line <= s.end.line && p.column >= s.start.column && p.column <= s.end.column
                } else {
                    p >= s.start && p <= s.end
                }
            });
            let spacer = f.intersects(Flags::WIDE_CHAR_SPACER | Flags::LEADING_WIDE_CHAR_SPACER);
            cells[row * cols + col] = CellView {
                c: if spacer { '\0' } else if f.contains(Flags::HIDDEN) { ' ' } else { cell.c },
                fg,
                bg,
                bold: f.contains(Flags::BOLD),
                italic: f.contains(Flags::ITALIC),
                underline: f.intersects(Flags::UNDERLINE | Flags::DOUBLE_UNDERLINE | Flags::UNDERCURL),
                wide: f.contains(Flags::WIDE_CHAR),
                selected,
                link: cell.hyperlink().is_some(),
            };
        }
        let cur = content.cursor.point;
        let crow = cur.line.0 + offset;
        let cursor = (crow >= 0 && (crow as usize) < rows && !content.mode.contains(TermMode::VI) && content.mode.contains(TermMode::SHOW_CURSOR))
            .then(|| (crow as usize, cur.column.0.min(cols.saturating_sub(1))));
        Snapshot { cols, rows, cells, cursor, display_offset: content.display_offset, alt_screen: content.mode.contains(TermMode::ALT_SCREEN) }
    }
}

impl Drop for Session {
    fn drop(&mut self) {
        self.shutdown();
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum SelectKind {
    Simple,
    Word,
    Line,
}

/// Our own dark ANSI palette (readable on #1e1f22), overridable by OSC 4/10/11.
struct Palette<'a> {
    overrides: &'a alacritty_terminal::term::color::Colors,
}

const ANSI: [[u8; 3]; 16] = [
    [0x1e, 0x1f, 0x22], // black
    [0xf0, 0x5d, 0x5d], // red
    [0x5f, 0xb8, 0x65], // green
    [0xe0, 0xa8, 0x5a], // yellow
    [0x56, 0x8a, 0xf2], // blue
    [0xc7, 0x7d, 0xbb], // magenta
    [0x2a, 0xac, 0xb8], // cyan
    [0xbc, 0xbe, 0xc4], // white
    [0x6f, 0x73, 0x7a], // bright black
    [0xff, 0x7b, 0x72], // bright red
    [0x7e, 0xd3, 0x84], // bright green
    [0xf2, 0xc5, 0x5c], // bright yellow
    [0x7c, 0xa8, 0xff], // bright blue
    [0xe0, 0x9e, 0xd6], // bright magenta
    [0x5c, 0xd2, 0xdc], // bright cyan
    [0xf0, 0xf1, 0xf3], // bright white
];
const FG: [u8; 3] = [0xdf, 0xe1, 0xe5];
const BG: [u8; 3] = [0x1e, 0x1f, 0x22];

fn rgb(c: Rgb) -> [u8; 3] {
    [c.r, c.g, c.b]
}

fn dim(c: [u8; 3]) -> [u8; 3] {
    [c[0] / 3 * 2, c[1] / 3 * 2, c[2] / 3 * 2]
}

impl Palette<'_> {
    fn named(&self, n: NamedColor) -> [u8; 3] {
        if let Some(c) = self.overrides[n] {
            return rgb(c);
        }
        let i = n as usize;
        match n {
            NamedColor::Foreground | NamedColor::BrightForeground => FG,
            NamedColor::Background => BG,
            NamedColor::Cursor => FG,
            NamedColor::DimForeground => dim(FG),
            _ if i < 16 => ANSI[i],
            // Dim variants (DimBlack..DimWhite) follow Cursor in the enum.
            _ => {
                let base = i.saturating_sub(NamedColor::DimBlack as usize).min(7);
                dim(ANSI[base])
            }
        }
    }

    fn indexed(&self, i: u8) -> [u8; 3] {
        if let Some(c) = self.overrides[i as usize] {
            return rgb(c);
        }
        match i {
            0..=15 => ANSI[i as usize],
            16..=231 => {
                let i = i - 16;
                let level = |v: u8| if v == 0 { 0 } else { 55 + v * 40 };
                [level(i / 36), level((i / 6) % 6), level(i % 6)]
            }
            _ => {
                let v = 8 + (i - 232) * 10;
                [v, v, v]
            }
        }
    }

    fn resolve(&self, c: Color, bold: bool, is_dim: bool) -> [u8; 3] {
        let out = match c {
            Color::Spec(rgb_) => rgb(rgb_),
            Color::Indexed(i) => self.indexed(if bold && i < 8 { i + 8 } else { i }),
            Color::Named(n) => {
                let n = if bold && (n as usize) < 8 { unsafe_bright(n) } else { n };
                self.named(n)
            }
        };
        if is_dim {
            dim(out)
        } else {
            out
        }
    }
}

/// Bold + normal ANSI color renders as the bright variant (xterm behavior).
fn unsafe_bright(n: NamedColor) -> NamedColor {
    use NamedColor::*;
    match n {
        Black => BrightBlack,
        Red => BrightRed,
        Green => BrightGreen,
        Yellow => BrightYellow,
        Blue => BrightBlue,
        Magenta => BrightMagenta,
        Cyan => BrightCyan,
        White => BrightWhite,
        other => other,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::mpsc;
    use std::time::{Duration, Instant};

    #[test]
    fn finds_in_rows() {
        let row = |l: i32, t: &str, wrapped: bool| SearchRow { line: l, cells: t.chars().enumerate().collect(), wrapped };
        // A wide char: its spacer column is missing from the row.
        let wide = SearchRow { line: 2, cells: vec![(0, '日'), (2, 'E'), (3, 'r'), (4, 'r')], wrapped: false };
        let rows = || vec![row(-1, "Error: x  error", false), row(0, "ok", false), wide_clone(&wide)];
        fn wide_clone(w: &SearchRow) -> SearchRow {
            SearchRow { line: w.line, cells: w.cells.clone(), wrapped: w.wrapped }
        }
        let f = find_in_rows(rows(), "error", false);
        assert_eq!(f.iter().map(|f| (f.line, f.cols.clone())).collect::<Vec<_>>(), vec![(-1, 0..5), (-1, 10..15)]);
        assert_eq!(f[0].text, "Error: x  error");
        assert_eq!(find_in_rows(rows(), "Error", true).len(), 1);
        assert_eq!(find_in_rows(rows(), "err", false).last().map(|f| (f.line, f.cols.clone())), Some((2, 2..5)));
        assert!(find_in_rows(rows(), "", false).is_empty());
        // A match broken by soft wrap: "needle" over rows 5 and 6.
        let wrapped = vec![row(5, "xxxxxxxnee", true), row(6, "dle yy", false), row(7, "dle", false)];
        let f = find_in_rows(wrapped, "needle", false);
        assert_eq!(f.len(), 1);
        assert_eq!((f[0].line, f[0].cols.clone(), f[0].more.clone(), f[0].row), (5, 7..10, vec![(6, 0..3)], 5));
        assert_eq!(f[0].text, "xxxxxxxneedle yy");
    }

    /// Spawns a real shell, runs a command and reads it back from the grid.
    #[test]
    fn echo_roundtrip() {
        let (tx, rx) = mpsc::channel();
        let opts = SpawnOptions { cwd: Some(std::env::temp_dir()), ..SpawnOptions::default() };
        let opts = if cfg!(windows) {
            SpawnOptions { shell: Some(("cmd.exe".into(), vec![])), ..opts }
        } else {
            SpawnOptions { shell: Some(("/bin/sh".into(), vec![])), ..opts }
        };
        let s = Session::spawn(opts, 80, 24, 8, 16, move |e| {
            let _ = tx.send(e);
        })
        .expect("spawn");
        s.write(b"echo brisk-ok-42\r".to_vec());
        let deadline = Instant::now() + Duration::from_secs(10);
        loop {
            let _ = rx.recv_timeout(Duration::from_millis(200));
            let snap = s.snapshot();
            let text: String = snap.cells.iter().map(|c| c.c).collect();
            // The echoed command and its output: the marker appears at least twice.
            if text.matches("brisk-ok-42").count() >= 2 {
                break;
            }
            assert!(Instant::now() < deadline, "no output; screen:\n{}", text);
        }
    }
}
