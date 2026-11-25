//! The Run console's output when there is no terminal emulation (pipes, or a saved run): a
//! compact log instead of a terminal grid. The text is one `String` with line starts, colours
//! are style runs over it and OSC 8 hyperlinks sit in a side table, so a line costs its bytes
//! plus a few words, not 24 bytes per column of the window. Rows exist only when drawn: lines
//! are wrapped at the console's width on the fly.
//!
//! The parser is the terminal's (vte) and handles what program output needs: text, LF, CR (a
//! `\r` progress line is rewritten in place), BS, tab, SGR colours and attributes, OSC 8,
//! erase in line, the cursor's visibility and Clear All.

use crate::{CellView, Found, Palette, SelectKind, Snapshot, TermEvent, MAX_FOUND};
use alacritty_terminal::term::color::Colors;
use alacritty_terminal::term::SEMANTIC_ESCAPE_CHARS;
use alacritty_terminal::vte::ansi::{Attr, ClearMode, Color, Handler, Hyperlink, LineClearMode, NamedColor, NamedPrivateMode, PrivateMode, Processor, Rgb};
use std::collections::HashMap;
use std::sync::Arc;
use unicode_width::UnicodeWidthChar;

const TAB: usize = 8;

const BOLD: u8 = 1;
const DIM: u8 = 2;
const ITALIC: u8 = 4;
const UNDERLINE: u8 = 8;
const INVERSE: u8 = 16;
const HIDDEN: u8 = 32;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
struct Style {
    fg: Color,
    bg: Color,
    flags: u8,
}

const DEFAULT_STYLE: Style = Style { fg: Color::Named(NamedColor::Foreground), bg: Color::Named(NamedColor::Background), flags: 0 };

fn color_key(c: Color) -> u64 {
    match c {
        Color::Named(n) => (1 << 24) | n as u64,
        Color::Indexed(i) => (2 << 24) | i as u64,
        Color::Spec(Rgb { r, g, b }) => (3 << 24) | (r as u64) << 16 | (g as u64) << 8 | b as u64,
    }
}

impl Style {
    fn key(&self) -> u64 {
        color_key(self.fg) << 34 | color_key(self.bg) << 8 | self.flags as u64
    }
}

/// A character of a line being rewritten (after `\r` or Backspace).
#[derive(Clone, Copy)]
struct Cell {
    c: char,
    style: u16,
    /// 0: none, otherwise index into `uris` + 1.
    link: u32,
}

/// One end of a selection: a character of a line and which half of its cell was hit.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
struct Anchor {
    /// Absolute line number (counting lines dropped from the front).
    line: u64,
    /// Character index in the line; the line's length when past its end.
    ch: usize,
    right: bool,
    past_end: bool,
}

#[derive(Clone, Copy, Debug)]
struct Selection {
    kind: SelectKind,
    start: Anchor,
    end: Anchor,
}

/// Display width of a character that is not a tab.
fn width(c: char) -> usize {
    if (c as u32) < 0x7f {
        1
    } else {
        c.width().unwrap_or(0)
    }
}

/// A character laid out in rows of `cols` columns.
#[derive(Clone, Copy, Debug)]
struct Placed {
    byte: usize,
    c: char,
    row: usize,
    col: usize,
    /// Columns it takes: 0 for a zero-width character (drawn with the previous one).
    w: usize,
    /// A wide character that did not fit in the last column of the previous row.
    lead_spacer: bool,
}

/// Lays out `line[start..]`, where `start` begins row `row` at column 0, calling `f` per
/// character until it returns false. Returns (last row, column after the last character).
fn place_from(line: &str, cols: usize, start: usize, mut row: usize, mut f: impl FnMut(Placed) -> bool) -> (usize, usize) {
    let mut col = 0;
    for (b, c) in line[start..].char_indices() {
        let mut lead_spacer = false;
        let w = if c == '\t' {
            if col >= cols {
                row += 1;
                col = 0;
            }
            (TAB - col % TAB).min(cols - col)
        } else {
            let w = width(c);
            if w > 0 && col + w > cols {
                lead_spacer = w == 2 && col + 1 == cols;
                row += 1;
                col = 0;
            }
            w
        };
        if !f(Placed { byte: start + b, c, row, col, w, lead_spacer }) {
            break;
        }
        col += w;
    }
    (row, col)
}

fn simple(line: &str) -> bool {
    line.is_ascii() && !line.as_bytes().contains(&b'\t')
}

/// Rows a line takes at `cols` columns (at least one).
fn line_rows(line: &str, cols: usize) -> usize {
    if simple(line) {
        return line.len().div_ceil(cols).max(1);
    }
    place_from(line, cols, 0, 0, |_| true).0 + 1
}

pub(crate) struct Log {
    parser: Processor,
    buf: Buf,
}

/// Everything but the parser, so that the parser can drive it as a [`Handler`].
pub(crate) struct Buf {
    text: String,
    /// Line starts in `text`; `starts[first..]` are the kept lines, the last one is the line
    /// being written (it has no newline yet). Lines carry no separators.
    starts: Vec<u32>,
    /// Rows of the lines before each line at the current width (same length as `starts`).
    rows_before: Vec<u32>,
    /// Lines dropped from the front but not yet removed from the buffers.
    first: usize,
    /// Lines ever dropped: absolute line numbers stay valid as the front goes.
    dropped: u64,
    /// Style runs: (byte offset, style id), each until the next one.
    runs: Vec<(u32, u16)>,
    /// OSC 8 hyperlinks: (start, end, index into `uris`), never across lines.
    links: Vec<(u32, u32, u32)>,
    uris: Vec<String>,
    styles: Vec<Style>,
    style_ids: HashMap<u64, u16>,
    pen: Style,
    pen_id: u16,
    /// The active OSC 8 hyperlink (index into `uris`).
    link: Option<u32>,
    /// Character index of the cursor in the last line.
    cursor: usize,
    /// Characters of the last line while it is in `text`.
    open_chars: usize,
    /// The last line while it is being rewritten (the cursor went back into it).
    open: Option<Vec<Cell>>,
    cursor_visible: bool,
    colors: Colors,
    cols: usize,
    rows: usize,
    /// Rows scrolled back from the bottom.
    offset: usize,
    /// Rows at the last settle: the view stays put while output arrives under it.
    prev_total: usize,
    /// Rows dropped from the front since the last settle.
    rows_dropped: usize,
    selection: Option<Selection>,
    /// Lines kept above the screen and bytes kept at most.
    max_lines: usize,
    max_bytes: usize,
    on_event: Option<crate::Callback>,
}

impl Log {
    pub(crate) fn new(cols: usize, rows: usize, max_lines: usize, max_bytes: usize, on_event: Option<crate::Callback>) -> Self {
        let mut buf = Buf {
            text: String::new(),
            starts: vec![0],
            rows_before: vec![0],
            first: 0,
            dropped: 0,
            runs: Vec::new(),
            links: Vec::new(),
            uris: Vec::new(),
            styles: Vec::new(),
            style_ids: HashMap::new(),
            pen: DEFAULT_STYLE,
            pen_id: 0,
            link: None,
            cursor: 0,
            open_chars: 0,
            open: None,
            cursor_visible: true,
            colors: Colors::default(),
            cols: cols.max(2),
            rows: rows.max(1),
            offset: 0,
            prev_total: 1,
            rows_dropped: 0,
            selection: None,
            max_lines,
            max_bytes: max_bytes.max(1024),
            on_event,
        };
        buf.pen_id = buf.intern(DEFAULT_STYLE);
        Self { parser: Processor::new(), buf }
    }

    /// Program output (and the echo of typed input).
    pub(crate) fn feed(&mut self, bytes: &[u8]) {
        self.parser.advance(&mut self.buf, bytes);
        self.buf.settle();
    }

    /// IDE text ("Process finished…"), parsed on its own so that an escape sequence the
    /// program left unfinished cannot swallow it.
    pub(crate) fn inject(&mut self, bytes: &[u8]) {
        let mut p: Processor = Processor::new();
        p.advance(&mut self.buf, bytes);
        self.buf.settle();
        self.buf.offset = 0;
    }
}

impl std::ops::Deref for Log {
    type Target = Buf;
    fn deref(&self) -> &Buf {
        &self.buf
    }
}

impl std::ops::DerefMut for Log {
    fn deref_mut(&mut self) -> &mut Buf {
        &mut self.buf
    }
}

// --- Writing --------------------------------------------------------------------

impl Buf {
    fn intern(&mut self, s: Style) -> u16 {
        if let Some(&id) = self.style_ids.get(&s.key()) {
            return id;
        }
        if self.styles.len() > u16::MAX as usize {
            // Out of ids (a truecolor gradient of 65k colours): default look from here on.
            return 0;
        }
        let id = self.styles.len() as u16;
        self.styles.push(s);
        self.style_ids.insert(s.key(), id);
        id
    }

    fn open_start(&self) -> usize {
        *self.starts.last().unwrap_or(&0) as usize
    }

    fn open_len(&self) -> usize {
        self.open.as_ref().map_or(self.open_chars, |c| c.len())
    }

    /// Append a character to the last line.
    fn push(&mut self, c: char, style: u16, link: u32) {
        let at = self.text.len() as u32;
        if self.runs.last().map(|r| r.1) != Some(style) {
            self.runs.push((at, style));
        }
        self.text.push(c);
        if link != 0 {
            let end = self.text.len() as u32;
            let line_start = self.open_start() as u32;
            match self.links.last_mut() {
                Some(l) if l.1 == at && l.2 == link - 1 && at > line_start => l.1 = end,
                _ => self.links.push((at, end, link - 1)),
            }
        }
        self.open_chars += 1;
    }

    /// The cursor went back into the last line: take it out of `text` as cells.
    fn detach(&mut self) {
        if self.open.is_some() {
            return;
        }
        let ls = self.open_start();
        let mut cells = Vec::with_capacity(self.open_chars);
        let mut ri = self.runs.partition_point(|r| r.0 as usize <= ls).saturating_sub(1);
        let mut li = self.links.partition_point(|l| l.1 as usize <= ls);
        for (b, c) in self.text[ls..].char_indices() {
            let b = (ls + b) as u32;
            while ri + 1 < self.runs.len() && self.runs[ri + 1].0 <= b {
                ri += 1;
            }
            while li < self.links.len() && self.links[li].1 <= b {
                li += 1;
            }
            let style = self.runs.get(ri).filter(|r| r.0 <= b).map_or(0, |r| r.1);
            let link = self.links.get(li).filter(|l| l.0 <= b).map_or(0, |l| l.2 + 1);
            cells.push(Cell { c, style, link });
        }
        self.text.truncate(ls);
        let keep = self.runs.partition_point(|r| (r.0 as usize) < ls);
        self.runs.truncate(keep);
        let keep = self.links.partition_point(|l| (l.0 as usize) < ls);
        self.links.truncate(keep);
        self.open_chars = 0;
        self.open = Some(cells);
    }

    /// Put a rewritten last line back into `text`.
    fn attach(&mut self) {
        let Some(cells) = self.open.take() else { return };
        self.open_chars = 0;
        for cell in cells {
            self.push(cell.c, cell.style, cell.link);
        }
    }

    fn blank() -> Cell {
        Cell { c: ' ', style: 0, link: 0 }
    }

    fn put(&mut self, c: char) {
        let link = self.link.map_or(0, |l| l + 1);
        if self.open.is_none() && self.cursor >= self.open_chars {
            while self.open_chars < self.cursor {
                self.push(' ', 0, 0);
            }
            self.push(c, self.pen_id, link);
            self.cursor += 1;
            return;
        }
        self.detach();
        let cursor = self.cursor;
        let cell = Cell { c, style: self.pen_id, link };
        let cells = self.open.as_mut().expect("detached");
        while cells.len() < cursor {
            cells.push(Self::blank());
        }
        if cursor == cells.len() {
            cells.push(cell);
        } else if width(c) == 0 {
            cells.insert(cursor, cell);
        } else {
            cells[cursor] = cell;
        }
        self.cursor += 1;
    }

    fn new_line(&mut self) {
        self.attach();
        let start = self.open_start();
        let rows = line_rows(&self.text[start..], self.cols) as u32;
        let before = *self.rows_before.last().unwrap_or(&0);
        self.starts.push(self.text.len() as u32);
        self.rows_before.push(before + rows);
        self.open_chars = 0;
        self.cursor = 0;
    }

    /// Drop everything (Clear All).
    fn clear_all(&mut self) {
        self.dropped += (self.starts.len() - self.first) as u64;
        self.open = None;
        self.text.clear();
        self.starts.clear();
        self.starts.push(0);
        self.rows_before.clear();
        self.rows_before.push(0);
        self.first = 0;
        self.runs.clear();
        self.links.clear();
        self.uris.clear();
        self.link = None;
        self.open_chars = 0;
        self.cursor = 0;
        self.offset = 0;
        self.selection = None;
        self.rows_dropped = 0;
        self.prev_total = 1;
    }

    /// After a batch of output: the last line back in place, the limits applied, the view
    /// kept where it was when scrolled back.
    fn settle(&mut self) {
        self.attach();
        self.enforce_limits();
        let total = self.total_rows();
        if self.offset > 0 {
            let added = (total + self.rows_dropped).saturating_sub(self.prev_total);
            self.offset += added;
        }
        self.offset = self.offset.min(self.history());
        self.prev_total = total;
        self.rows_dropped = 0;
    }

    fn enforce_limits(&mut self) {
        let n = self.n_lines();
        let mut drop = n.saturating_sub(self.max_lines + self.rows);
        let len = self.text.len();
        if len - self.starts[self.first] as usize > self.max_bytes {
            let min_start = (len - self.max_bytes) as u32;
            let p = self.starts[self.first..].partition_point(|&s| s < min_start);
            drop = drop.max(p);
        }
        let drop = drop.min(n - 1);
        if drop > 0 {
            let base = self.rows_before[self.first];
            self.rows_dropped += (self.rows_before[self.first + drop] - base) as usize;
            self.first += drop;
            self.dropped += drop as u64;
            if let Some(s) = self.selection.as_mut() {
                let first = self.dropped;
                if s.start.line < first && s.end.line < first {
                    self.selection = None;
                } else {
                    for a in [&mut s.start, &mut s.end] {
                        if a.line < first {
                            *a = Anchor { line: first, ch: 0, right: false, past_end: false };
                        }
                    }
                }
            }
        }
        let live_start = self.starts[self.first] as usize;
        let dead = live_start;
        if self.n_lines() == 1 && len - live_start > self.max_bytes {
            // One line longer than the limit: keep its end, as a terminal keeps the last rows.
            let mut cut = len - self.max_bytes / 2;
            while !self.text.is_char_boundary(cut) {
                cut += 1;
            }
            let removed = self.text[live_start..cut].chars().count();
            let rows_gone = self.total_rows();
            self.compact(cut);
            self.open_chars -= removed;
            self.cursor = self.cursor.saturating_sub(removed);
            self.selection = None;
            self.rows_dropped += rows_gone.saturating_sub(self.total_rows());
        } else if self.first > 0 && (self.first >= self.n_lines() || dead >= len - live_start) {
            self.compact(live_start);
        }
    }

    /// Remove the dropped lines (and, for one long line, its first `cut` bytes) from the
    /// buffers.
    fn compact(&mut self, cut: usize) {
        let cut32 = cut as u32;
        self.text.drain(..cut);
        self.starts.drain(..self.first);
        for s in &mut self.starts {
            *s = s.saturating_sub(cut32);
        }
        self.rows_before.drain(..self.first);
        let base = self.rows_before[0];
        for r in &mut self.rows_before {
            *r -= base;
        }
        if self.starts.len() == 1 {
            // A cut inside the only line changes its rows.
            self.rows_before[0] = 0;
        }
        self.first = 0;
        let keep_from = self.runs.partition_point(|r| r.0 <= cut32).saturating_sub(1);
        self.runs.drain(..keep_from);
        if let Some(r) = self.runs.first_mut() {
            r.0 = r.0.max(cut32);
        }
        for r in &mut self.runs {
            r.0 -= cut32;
        }
        self.links.retain(|l| l.1 > cut32);
        for l in &mut self.links {
            l.0 = l.0.max(cut32) - cut32;
            l.1 -= cut32;
        }
        if self.uris.len() > 64 {
            // Forget the URIs no kept link uses.
            let mut map: HashMap<u32, u32> = HashMap::new();
            let mut uris = Vec::new();
            let old = std::mem::take(&mut self.uris);
            for id in self.links.iter().map(|l| l.2).chain(self.link) {
                map.entry(id).or_insert_with(|| {
                    uris.push(old[id as usize].clone());
                    uris.len() as u32 - 1
                });
            }
            for l in &mut self.links {
                l.2 = map[&l.2];
            }
            self.link = self.link.map(|l| map[&l]);
            self.uris = uris;
        }
        if self.styles.len() > 0x8000 {
            let mut map: HashMap<u16, u16> = HashMap::new();
            let old = std::mem::take(&mut self.styles);
            self.style_ids.clear();
            self.intern(DEFAULT_STYLE);
            let used: Vec<u16> = self.runs.iter().map(|r| r.1).chain([self.pen_id]).collect();
            for id in used {
                if let std::collections::hash_map::Entry::Vacant(e) = map.entry(id) {
                    e.insert(self.intern(old[id as usize]));
                }
            }
            for r in &mut self.runs {
                r.1 = map[&r.1];
            }
            self.pen_id = map[&self.pen_id];
        }
    }

    fn intern_uri(&mut self, uri: String) -> u32 {
        if let Some(i) = self.uris.iter().rev().take(16).position(|u| *u == uri) {
            return (self.uris.len() - 1 - i) as u32;
        }
        self.uris.push(uri);
        self.uris.len() as u32 - 1
    }
}

impl Handler for Buf {
    fn input(&mut self, c: char) {
        self.put(c);
    }

    fn put_tab(&mut self, count: u16) {
        for _ in 0..count {
            if self.open.is_none() && self.cursor >= self.open_chars {
                while self.open_chars < self.cursor {
                    self.push(' ', 0, 0);
                }
                self.push('\t', 0, 0);
            }
            self.cursor += 1;
        }
    }

    fn backspace(&mut self) {
        self.cursor = self.cursor.saturating_sub(1);
    }

    fn carriage_return(&mut self) {
        self.cursor = 0;
    }

    fn linefeed(&mut self) {
        self.new_line();
    }

    fn newline(&mut self) {
        self.new_line();
    }

    fn set_title(&mut self, title: Option<String>) {
        if let Some(cb) = &self.on_event {
            cb(TermEvent::Title(title.unwrap_or_default()));
        }
    }

    fn clear_line(&mut self, mode: LineClearMode) {
        if self.open_len() == 0 {
            return;
        }
        self.detach();
        let cursor = self.cursor;
        let cells = self.open.as_mut().expect("detached");
        match mode {
            LineClearMode::Right => cells.truncate(cursor),
            LineClearMode::All => cells.clear(),
            LineClearMode::Left => {
                let end = (cursor + 1).min(cells.len());
                cells[..end].fill(Self::blank());
            }
        }
    }

    fn clear_screen(&mut self, mode: ClearMode) {
        match mode {
            // The screen goes into the scrollback, new output starts on a line of its own.
            ClearMode::All => {
                if self.open_len() > 0 {
                    self.new_line();
                }
            }
            // The scrollback goes (Clear All sends 2J then 3J).
            ClearMode::Saved => {
                if self.open_len() == 0 {
                    self.clear_all();
                }
            }
            ClearMode::Above | ClearMode::Below => {}
        }
    }

    fn reset_state(&mut self) {
        self.pen = DEFAULT_STYLE;
        self.pen_id = self.intern(DEFAULT_STYLE);
        self.link = None;
        self.cursor_visible = true;
        self.colors = Colors::default();
    }

    fn terminal_attribute(&mut self, attr: Attr) {
        let p = &mut self.pen;
        match attr {
            Attr::Foreground(c) => p.fg = c,
            Attr::Background(c) => p.bg = c,
            Attr::Reset => *p = DEFAULT_STYLE,
            Attr::Reverse => p.flags |= INVERSE,
            Attr::CancelReverse => p.flags &= !INVERSE,
            Attr::Bold => p.flags |= BOLD,
            Attr::CancelBold => p.flags &= !BOLD,
            Attr::Dim => p.flags |= DIM,
            Attr::CancelBoldDim => p.flags &= !(BOLD | DIM),
            Attr::Italic => p.flags |= ITALIC,
            Attr::CancelItalic => p.flags &= !ITALIC,
            Attr::Underline | Attr::DoubleUnderline | Attr::Undercurl => p.flags |= UNDERLINE,
            // Drawn without a line, as in the terminal.
            Attr::DottedUnderline | Attr::DashedUnderline | Attr::CancelUnderline => p.flags &= !UNDERLINE,
            Attr::Hidden => p.flags |= HIDDEN,
            Attr::CancelHidden => p.flags &= !HIDDEN,
            _ => return,
        }
        let pen = self.pen;
        self.pen_id = self.intern(pen);
    }

    fn set_private_mode(&mut self, mode: PrivateMode) {
        if mode == PrivateMode::Named(NamedPrivateMode::ShowCursor) {
            self.cursor_visible = true;
        }
    }

    fn unset_private_mode(&mut self, mode: PrivateMode) {
        if mode == PrivateMode::Named(NamedPrivateMode::ShowCursor) {
            self.cursor_visible = false;
        }
    }

    fn set_color(&mut self, index: usize, c: Rgb) {
        if index < alacritty_terminal::term::color::COUNT {
            self.colors[index] = Some(c);
        }
    }

    fn reset_color(&mut self, index: usize) {
        if index < alacritty_terminal::term::color::COUNT {
            self.colors[index] = None;
        }
    }

    fn set_hyperlink(&mut self, link: Option<Hyperlink>) {
        self.link = link.map(|h| self.intern_uri(h.uri));
    }
}

// --- Reading ---------------------------------------------------------------------

impl Buf {
    pub(crate) fn n_lines(&self) -> usize {
        self.starts.len() - self.first
    }

    /// Line `i` of the kept ones.
    fn line(&self, i: usize) -> &str {
        let p = self.first + i;
        let start = self.starts[p] as usize;
        let end = self.starts.get(p + 1).map_or(self.text.len(), |&e| e as usize);
        &self.text[start..end]
    }

    fn line_start(&self, i: usize) -> usize {
        self.starts[self.first + i] as usize
    }

    /// First row of line `i`, counted from the first kept row.
    fn row_of(&self, i: usize) -> usize {
        (self.rows_before[self.first + i] - self.rows_before[self.first]) as usize
    }

    pub(crate) fn total_rows(&self) -> usize {
        let n = self.n_lines();
        self.row_of(n - 1) + line_rows(self.line(n - 1), self.cols)
    }

    /// Rows above the screen.
    pub(crate) fn history(&self) -> usize {
        self.total_rows().saturating_sub(self.rows)
    }

    /// Row of the screen's top line.
    fn top(&self) -> usize {
        self.history() - self.offset.min(self.history())
    }

    /// The line holding row `v` and the row within that line.
    fn locate(&self, v: usize) -> (usize, usize) {
        let base = self.rows_before[self.first];
        let i = self.rows_before[self.first..].partition_point(|&r| ((r - base) as usize) <= v).saturating_sub(1);
        (i, v - self.row_of(i))
    }

    pub(crate) fn resize(&mut self, cols: usize, rows: usize) {
        let (cols, rows) = (cols.max(2), rows.max(1));
        if cols != self.cols {
            self.cols = cols;
            for p in self.first..self.starts.len() - 1 {
                let i = p - self.first;
                let r = line_rows(self.line(i), cols) as u32;
                self.rows_before[p + 1] = self.rows_before[p] + r;
            }
        }
        self.rows = rows;
        self.offset = self.offset.min(self.history());
        self.prev_total = self.total_rows();
    }

    pub(crate) fn scroll(&mut self, delta: i32) {
        self.offset = (self.offset as i64 + delta as i64).clamp(0, self.history() as i64) as usize;
    }

    /// (lines scrolled back, screen lines, lines above the screen).
    pub(crate) fn scroll_pos(&self) -> (usize, usize, usize) {
        let h = self.history();
        (self.offset.min(h), self.rows, h)
    }

    /// Scroll so that `line` (a row relative to the screen at the bottom) is visible,
    /// centered when it was off screen.
    pub(crate) fn scroll_to(&mut self, line: i32) {
        let (offset, rows, history) = self.scroll_pos();
        let (offset, rows, history) = (offset as i32, rows as i32, history as i32);
        if line >= -offset && line < rows - offset {
            return;
        }
        self.offset = (rows / 2 - line).clamp(0, history) as usize;
    }

    /// Visual position (row from the first kept row, column) of character `ch` of line `i`;
    /// the end of the line for `ch` past it.
    fn pos(&self, i: usize, ch: usize) -> (usize, usize) {
        let line = self.line(i);
        let row0 = self.row_of(i);
        if simple(line) {
            let k = ch.min(line.len());
            if k == line.len() && k > 0 && k.is_multiple_of(self.cols) {
                return (row0 + k / self.cols - 1, self.cols);
            }
            return (row0 + k / self.cols, k % self.cols);
        }
        let mut found = None;
        let mut n = 0;
        let end = place_from(line, self.cols, 0, 0, |p| {
            if n == ch {
                found = Some((p.row, p.col));
                return false;
            }
            n += 1;
            true
        });
        let (r, c) = found.unwrap_or(end);
        (row0 + r, c)
    }

    /// The character under column `col` of row `v`: (line, char index, past the row's text).
    fn hit(&self, v: usize, col: usize) -> Option<(usize, usize, bool)> {
        if v >= self.total_rows() {
            let i = self.n_lines() - 1;
            return Some((i, self.line(i).chars().count(), true));
        }
        let (i, r) = self.locate(v);
        let line = self.line(i);
        let mut n = 0;
        let mut out = None;
        let mut after_row = None;
        place_from(line, self.cols, 0, 0, |p| {
            if p.row > r {
                after_row = Some(n);
                return false;
            }
            if p.row == r && p.w > 0 && col >= p.col && col < p.col + p.w {
                out = Some(n);
                return false;
            }
            n += 1;
            true
        });
        Some(match out {
            Some(k) => (i, k, false),
            None => (i, after_row.unwrap_or(n), true),
        })
    }

    fn style_at(&self, ri: &mut usize, b: u32) -> u16 {
        while *ri + 1 < self.runs.len() && self.runs[*ri + 1].0 <= b {
            *ri += 1;
        }
        self.runs.get(*ri).filter(|r| r.0 <= b).map_or(0, |r| r.1)
    }

    fn link_at(&self, li: &mut usize, b: u32) -> Option<u32> {
        while *li < self.links.len() && self.links[*li].1 <= b {
            *li += 1;
        }
        self.links.get(*li).filter(|l| l.0 <= b).map(|l| l.2)
    }

    pub(crate) fn hyperlink_at(&self, row: usize, col: usize) -> Option<String> {
        if row >= self.rows || col >= self.cols {
            return None;
        }
        let (i, k, past) = self.hit(self.top() + row, col)?;
        if past {
            return None;
        }
        let line = self.line(i);
        let b = (self.line_start(i) + line.char_indices().nth(k)?.0) as u32;
        let mut li = self.links.partition_point(|l| l.1 <= b);
        self.link_at(&mut li, b).map(|u| self.uris[u as usize].clone())
    }

    pub(crate) fn snapshot(&self) -> Snapshot {
        let (cols, rows) = (self.cols, self.rows);
        let palette = Palette { overrides: &self.colors };
        let blank = CellView {
            c: ' ',
            fg: palette.named(NamedColor::Foreground),
            bg: None,
            bold: false,
            italic: false,
            underline: false,
            wide: false,
            selected: false,
            link: false,
        };
        let mut cells = vec![blank; cols * rows];
        let top = self.top();
        let total = self.total_rows();
        let n = self.n_lines();
        if top < total {
            let (mut i, mut r0) = self.locate(top);
            let mut sr0 = 0usize;
            while sr0 < rows && i < n {
                let line = self.line(i);
                let ls = self.line_start(i);
                let (start, row) = if r0 > 0 && simple(line) { ((r0 * cols).min(line.len()), r0) } else { (0, 0) };
                let mut ri = self.runs.partition_point(|r| r.0 as usize <= ls + start).saturating_sub(1);
                let mut li = self.links.partition_point(|l| l.1 as usize <= ls + start);
                place_from(line, cols, start, row, |p| {
                    if p.row < r0 {
                        return true;
                    }
                    let sr = sr0 + p.row - r0;
                    if sr >= rows {
                        return false;
                    }
                    if p.lead_spacer && sr > 0 {
                        cells[(sr - 1) * cols + cols - 1].c = '\0';
                    }
                    if p.w == 0 || p.c == '\t' {
                        return true;
                    }
                    let b = (ls + p.byte) as u32;
                    let st = self.styles[self.style_at(&mut ri, b) as usize];
                    let link = self.link_at(&mut li, b).is_some();
                    let f = st.flags;
                    let mut fg = palette.resolve(st.fg, f & BOLD != 0, f & DIM != 0);
                    let mut bg = match st.bg {
                        Color::Named(NamedColor::Background) => None,
                        c => Some(palette.resolve(c, false, false)),
                    };
                    if f & INVERSE != 0 {
                        let b = bg.unwrap_or(palette.named(NamedColor::Background));
                        bg = Some(fg);
                        fg = b;
                    }
                    let cell = CellView {
                        c: if f & HIDDEN != 0 { ' ' } else { p.c },
                        fg,
                        bg,
                        bold: f & BOLD != 0,
                        italic: f & ITALIC != 0,
                        underline: f & UNDERLINE != 0,
                        wide: p.w == 2,
                        selected: false,
                        link,
                    };
                    cells[sr * cols + p.col] = cell;
                    if p.w == 2 && p.col + 1 < cols {
                        cells[sr * cols + p.col + 1] = CellView { c: '\0', wide: false, ..cell };
                    }
                    true
                });
                sr0 += line_rows(line, cols) - r0;
                r0 = 0;
                i += 1;
            }
        }
        if let Some((s, e)) = self.selection_span() {
            for sr in 0..rows {
                let v = top + sr;
                if v < s.0 || v > e.0 {
                    continue;
                }
                for c in 0..cols {
                    if (v, c) >= s && (v, c) <= e {
                        cells[sr * cols + c].selected = true;
                    }
                }
            }
        }
        let cursor = self.cursor_visible.then(|| {
            let i = n - 1;
            let len = self.open_len();
            let (r, c) = self.pos(i, self.cursor.min(len));
            let c = (c + self.cursor.saturating_sub(len)).min(cols - 1);
            (r, c)
        });
        let cursor = cursor.and_then(|(r, c)| (r >= top && r - top < rows).then(|| (r - top, c)));
        Snapshot { cols, rows, cells, cursor, display_offset: self.offset.min(self.history()), alt_screen: false }
    }

    /// Plain text of everything kept, trailing blanks of each line trimmed.
    pub(crate) fn text(&self) -> String {
        let mut out = String::with_capacity(self.text.len() - self.line_start(0) + self.n_lines());
        for i in 0..self.n_lines() {
            out.push_str(self.line(i).trim_end());
            out.push('\n');
        }
        out.truncate(out.trim_end().len());
        out
    }

    /// Matches of `needle` (case-insensitive unless `case`), top to bottom, at most
    /// [`MAX_FOUND`]. Lines are `0..rows` on the screen at the bottom, negative above.
    pub(crate) fn find(&self, needle: &str, case: bool) -> Vec<Found> {
        let fold = |c: char| match c {
            _ if case => c,
            _ if c.is_ascii() => c.to_ascii_lowercase(),
            _ => c.to_lowercase().next().unwrap_or(c),
        };
        let pat: Vec<char> = needle.chars().map(fold).collect();
        let mut out = Vec::new();
        if pat.is_empty() {
            return out;
        }
        let pat_str: String = pat.iter().collect();
        let history = self.history() as i64;
        let grid_line = |v: usize| (v as i64 - history) as i32;
        let mut lower = String::new();
        for i in 0..self.n_lines() {
            if out.len() >= MAX_FOUND {
                break;
            }
            let full = self.line(i);
            // Trailing blanks never take part in a match.
            let line = full.trim_end_matches(' ');
            // Matches as char ranges.
            let mut hits: Vec<(usize, usize)> = Vec::new();
            if line.is_ascii() {
                if !pat_str.is_ascii() {
                    continue;
                }
                let hay = if case {
                    line
                } else {
                    lower.clear();
                    lower.push_str(line);
                    lower.make_ascii_lowercase();
                    lower.as_str()
                };
                let mut at = 0;
                while let Some(k) = hay[at..].find(&pat_str) {
                    hits.push((at + k, at + k + pat.len()));
                    at += k + pat_str.len();
                }
            } else {
                // Zero-width characters are not in the cells, so they are not searched.
                let chars: Vec<(usize, char)> = line.chars().enumerate().filter(|&(_, c)| width(c) > 0 || c == '\t').map(|(k, c)| (k, fold(c))).collect();
                let mut j = 0;
                while j + pat.len() <= chars.len() {
                    if chars[j..j + pat.len()].iter().map(|x| x.1).eq(pat.iter().copied()) {
                        hits.push((chars[j].0, chars[j + pat.len() - 1].0 + 1));
                        j += pat.len();
                    } else {
                        j += 1;
                    }
                }
            }
            if hits.is_empty() {
                continue;
            }
            let row0 = self.row_of(i);
            // (row, column) of every character of the line.
            let cells: Vec<(usize, usize, usize)> = if simple(line) {
                Vec::new()
            } else {
                let mut v = Vec::new();
                place_from(line, self.cols, 0, 0, |p| {
                    v.push((p.row, p.col, p.w));
                    true
                });
                v
            };
            let at = |k: usize| if cells.is_empty() { (k / self.cols, k % self.cols, 1) } else { cells[k] };
            let text: Arc<str> = Arc::from(full.trim_end());
            for (a, b) in hits {
                if out.len() >= MAX_FOUND {
                    break;
                }
                let mut parts: Vec<(i32, std::ops::Range<usize>)> = Vec::new();
                for k in a..b {
                    let (r, c, w) = at(k);
                    if w == 0 {
                        continue;
                    }
                    let l = grid_line(row0 + r);
                    match parts.last_mut() {
                        Some((pl, rg)) if *pl == l => rg.end = c + 1,
                        _ => parts.push((l, c..c + 1)),
                    }
                }
                if parts.is_empty() {
                    continue;
                }
                let (line, cols) = parts.remove(0);
                out.push(Found { line, cols, more: parts, row: grid_line(row0), text: text.clone() });
            }
        }
        out
    }
}

// --- Selection --------------------------------------------------------------------

impl Buf {
    fn anchor(&self, row: usize, col: usize, right: bool) -> Option<Anchor> {
        let (i, ch, past_end) = self.hit(self.top() + row, col.min(self.cols - 1))?;
        Some(Anchor { line: self.dropped + i as u64, ch, right, past_end })
    }

    pub(crate) fn start_selection(&mut self, row: usize, col: usize, kind: SelectKind) {
        self.selection = self.anchor(row, col, false).map(|a| Selection { kind, start: a, end: a });
    }

    pub(crate) fn update_selection(&mut self, row: usize, col: usize, right: bool) {
        let Some(a) = self.anchor(row, col, right) else { return };
        if let Some(s) = self.selection.as_mut() {
            s.end = a;
        }
    }

    pub(crate) fn clear_selection(&mut self) {
        self.selection = None;
    }

    /// Kept line index of an absolute line.
    fn idx(&self, line: u64) -> usize {
        (line.saturating_sub(self.dropped) as usize).min(self.n_lines() - 1)
    }

    fn sep(c: Option<char>) -> bool {
        c.is_none_or(|c| SEMANTIC_ESCAPE_CHARS.contains(c))
    }

    /// The selection's ends in text order.
    fn ordered(&self) -> Option<(SelectKind, Anchor, Anchor)> {
        let s = self.selection?;
        let (a, b) = (s.start, s.end);
        Some(if (b.line, b.ch, b.right) < (a.line, a.ch, a.right) { (s.kind, b, a) } else { (s.kind, a, b) })
    }

    /// The selected text as (line, char) boundaries: start inclusive, end exclusive.
    fn selection_range(&self) -> Option<((usize, usize), (usize, usize))> {
        let (kind, a, b) = self.ordered()?;
        let (la, lb) = (self.idx(a.line), self.idx(b.line));
        let len = |i: usize| self.line(i).chars().count();
        match kind {
            SelectKind::Simple => {
                let ka = if a.right && !a.past_end { a.ch + 1 } else { a.ch }.min(len(la));
                let kb = if b.right && !b.past_end { b.ch + 1 } else { b.ch }.min(len(lb));
                ((la, ka) < (lb, kb)).then_some(((la, ka), (lb, kb)))
            }
            SelectKind::Word => {
                let chars_a: Vec<char> = self.line(la).chars().collect();
                let mut ka = a.ch.min(chars_a.len());
                if !Self::sep(chars_a.get(ka).copied()) {
                    while ka > 0 && !Self::sep(Some(chars_a[ka - 1])) {
                        ka -= 1;
                    }
                }
                let chars_b: Vec<char> = self.line(lb).chars().collect();
                let mut kb = b.ch.min(chars_b.len());
                if kb < chars_b.len() {
                    if Self::sep(Some(chars_b[kb])) {
                        kb += 1;
                    } else {
                        while kb < chars_b.len() && !Self::sep(Some(chars_b[kb])) {
                            kb += 1;
                        }
                    }
                }
                ((la, ka) < (lb, kb)).then_some(((la, ka), (lb, kb)))
            }
            SelectKind::Line => Some(((la, 0), (lb, len(lb)))),
        }
    }

    /// The selection in visual coordinates (row, column), both ends inclusive.
    fn selection_span(&self) -> Option<((usize, usize), (usize, usize))> {
        let (kind, _, end_anchor) = self.ordered()?;
        let ((la, ka), (lb, kb)) = self.selection_range()?;
        let start = self.pos(la, ka);
        let start = if start.1 >= self.cols { (start.0 + 1, 0) } else { start };
        let len_b = self.line(lb).chars().count();
        let to_edge = kb == len_b && (kind == SelectKind::Line || end_anchor.past_end);
        let end = if to_edge {
            let last = self.row_of(lb) + line_rows(self.line(lb), self.cols) - 1;
            (last, self.cols - 1)
        } else if kb == 0 {
            // Ends before the first character of a line: the end of the line before.
            let p = lb - 1;
            (self.row_of(p) + line_rows(self.line(p), self.cols) - 1, self.cols - 1)
        } else {
            let (r, c) = self.pos(lb, kb - 1);
            let w = self.line(lb).chars().nth(kb - 1).map_or(1, |ch| if ch == '\t' { 1 } else { width(ch).max(1) });
            (r, (c + w - 1).min(self.cols - 1))
        };
        Some((start, end))
    }

    pub(crate) fn selection_text(&self) -> Option<String> {
        let kind = self.selection?.kind;
        let ((la, ka), (lb, kb)) = self.selection_range()?;
        let mut out = String::new();
        for i in la..=lb {
            let line = self.line(i);
            let from = if i == la { ka } else { 0 };
            let to = if i == lb { kb } else { usize::MAX };
            let part: String = line.chars().skip(from).take(to.saturating_sub(from)).collect();
            // A part that reaches the end of its line loses the line's trailing blanks.
            let reaches_end = i < lb || kb >= line.chars().count();
            out.push_str(if reaches_end { part.trim_end_matches(' ') } else { &part });
            if i < lb {
                out.push('\n');
            }
        }
        if kind == SelectKind::Line {
            out.push('\n');
        }
        Some(out)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn log(cols: usize, rows: usize) -> Log {
        Log::new(cols, rows, 1000, 1 << 20, None)
    }

    fn feed(l: &mut Log, s: &str) {
        l.feed(s.as_bytes());
    }

    fn screen(l: &Log) -> Vec<String> {
        let s = l.snapshot();
        (0..s.rows)
            .map(|r| s.cells[r * s.cols..(r + 1) * s.cols].iter().filter(|c| c.c != '\0').map(|c| c.c).collect::<String>().trim_end().to_string())
            .collect()
    }

    #[test]
    fn carriage_return_rewrites_the_line() {
        let mut l = log(40, 5);
        feed(&mut l, "cmd\r\n 10%\r 50%\r");
        feed(&mut l, "100%\ndone\n");
        assert_eq!(l.text(), "cmd\n100%\ndone");
        // Overwriting keeps the rest of a longer line, as a terminal does.
        feed(&mut l, "abcdef\rXY");
        assert_eq!(l.text().lines().last(), Some("XYcdef"));
        assert_eq!(l.snapshot().cursor, Some((3, 2)));
        // Backspace echo of the Run console: "\x08 \x08".
        feed(&mut l, "\nab\x08 \x08c\n");
        assert_eq!(l.text().lines().last(), Some("ac"));
        // Erase in line.
        feed(&mut l, "progress 99%\r\x1b[Kok\n");
        assert_eq!(l.text().lines().last(), Some("ok"));
    }

    #[test]
    fn sgr_runs() {
        let mut l = log(60, 3);
        feed(&mut l, "\x1b[31mred\x1b[0m n \x1b[38;5;196mi\x1b[38;2;1;2;3mt\x1b[1;3;4mB\x1b[0;7mv\x1b[0m\n");
        let s = l.snapshot();
        let c = &s.cells;
        assert_eq!(c[0].fg, crate::ANSI[1]);
        assert_eq!((c[0].c, c[2].c), ('r', 'd'));
        assert_eq!(c[4].fg, crate::FG);
        assert_eq!(c[6].fg, [255, 0, 0]);
        assert_eq!(c[7].fg, [1, 2, 3]);
        assert!(c[8].bold && c[8].italic && c[8].underline);
        // Bold keeps the truecolor colour.
        assert_eq!(c[8].fg, [1, 2, 3]);
        // Inverse: background takes the foreground.
        assert_eq!((c[9].bg, c[9].fg), (Some(crate::FG), crate::BG));
        assert!(!c[10].bold && c[10].bg.is_none());
        // Few runs: one per change, not per character.
        assert!(l.runs.len() <= 8, "{:?}", l.runs);
        // A red stderr line stays red after its line is rewritten.
        feed(&mut l, "\x1b[31merr 1\rE\x1b[39m\n");
        let s = l.snapshot();
        let row = &s.cells[s.cols..2 * s.cols];
        assert_eq!((row[0].c, row[0].fg, row[1].fg), ('E', crate::ANSI[1], crate::ANSI[1]));
    }

    #[test]
    fn line_limit_drops_the_oldest() {
        let mut l = Log::new(20, 4, 100, 1 << 20, None);
        for i in 0..1000 {
            feed(&mut l, &format!("line {i}\r\n"));
        }
        // 100 lines above a 4-line screen.
        assert_eq!(l.history(), 100);
        assert_eq!(l.n_lines(), 104);
        let text = l.text();
        assert_eq!(text.lines().next(), Some("line 897"));
        assert_eq!(text.lines().last(), Some("line 999"));
        // The dropped front is really freed now and then.
        assert!(l.starts.len() <= 2 * 104 + 1, "{}", l.starts.len());
        assert!(l.text.len() < 2 * 104 * 9);
        let f = l.find("line 950", true);
        assert_eq!(f.len(), 1);
        assert_eq!(&*f[0].text, "line 950");
        // A line scrolled back to stays in view while output arrives.
        l.scroll(10);
        let before = screen(&l);
        feed(&mut l, "more\r\nmore\r\n");
        assert_eq!(screen(&l), before);
    }

    #[test]
    fn byte_limit() {
        let mut l = Log::new(80, 4, 1000, 4096, None);
        for i in 0..100 {
            feed(&mut l, &format!("{i:03}{}\n", "x".repeat(200)));
        }
        assert!(l.text.len() <= 2 * 4096 + 256, "{}", l.text.len());
        assert!(l.text().ends_with(&format!("099{}", "x".repeat(200))));
        // One endless line keeps its end.
        let mut l = Log::new(80, 4, 1000, 4096, None);
        for i in 0..100 {
            feed(&mut l, &format!("{i:03}{}", "y".repeat(200)));
        }
        assert!(l.text.len() <= 4096 + 256, "{}", l.text.len());
        assert!(l.text().ends_with(&format!("099{}", "y".repeat(200))));
        feed(&mut l, "\nend\n");
        assert!(l.text().ends_with("\nend"));
    }

    #[test]
    fn osc8_hyperlinks() {
        let mut l = log(40, 3);
        feed(&mut l, "see \x1b]8;;file:///C:/x.py#L3\x1b\\x.py\x1b]8;;\x1b\\ and \x1b]8;;https://e.com\x07site\x1b]8;;\x07\n");
        let s = l.snapshot();
        let links: Vec<bool> = s.cells[..s.cols].iter().map(|c| c.link).collect();
        assert_eq!(links.iter().filter(|&&b| b).count(), 8);
        assert!(!links[3] && links[4] && links[7] && !links[8]);
        assert_eq!(l.hyperlink_at(0, 4).as_deref(), Some("file:///C:/x.py#L3"));
        assert_eq!(l.hyperlink_at(0, 14).as_deref(), Some("https://e.com"));
        assert_eq!(l.hyperlink_at(0, 9), None);
        assert_eq!(l.text(), "see x.py and site");
    }

    #[test]
    fn wide_and_unicode() {
        let mut l = log(4, 8);
        feed(&mut l, "日本語abc\nпривет\ne\u{301}!\n");
        // 日本 | 語ab | c
        assert_eq!(line_rows("日本語abc", 4), 3);
        let s = l.snapshot();
        assert_eq!((s.cells[0].c, s.cells[0].wide, s.cells[1].c, s.cells[2].c), ('日', true, '\0', '本'));
        assert_eq!(&screen(&l)[..5], ["日本", "語ab", "c", "прив", "ет"]);
        // A wide character that does not fit in the last column goes to the next row.
        let mut l = log(5, 3);
        feed(&mut l, "abc日本\n");
        let s = l.snapshot();
        assert_eq!((s.cells[3].c, s.cells[4].c, s.cells[5].c), ('日', '\0', '本'));
        assert_eq!(l.text(), "abc日本");
        let mut l = log(4, 3);
        feed(&mut l, "abc日\n");
        let s = l.snapshot();
        assert_eq!((s.cells[2].c, s.cells[3].c, s.cells[4].c, s.cells[4].wide), ('c', '\0', '日', true));
        // Combining marks stay in the text; search finds across the wrap.
        let mut l = log(4, 6);
        feed(&mut l, "e\u{301}!\nxxпривет\n");
        assert_eq!(l.text(), "e\u{301}!\nxxпривет");
        let f = l.find("ПРИ", false);
        assert_eq!(f.len(), 1);
        assert_eq!((f[0].cols.clone(), f[0].more.clone()), (2..4, vec![(f[0].line + 1, 0..1)]));
    }

    #[test]
    fn find_rows_and_limit() {
        let mut l = log(10, 3);
        feed(&mut l, "header\nxxxxxxxnee\x1b[31mdle\x1b[0m yy\nerror Error\n");
        let f = l.find("needle", false);
        assert_eq!(f.len(), 1);
        // 6 rows: header, xxxxxxxnee, dle yy, error Erro, r and the cursor's; the screen shows the last 3.
        assert_eq!(l.total_rows(), 6);
        assert_eq!((f[0].line, f[0].cols.clone(), f[0].more.clone(), f[0].row), (-2, 7..10, vec![(-1, 0..3)], -2));
        assert_eq!(&*f[0].text, "xxxxxxxneedle yy");
        assert_eq!(l.find("error", false).len(), 2);
        assert_eq!(l.find("Error", true).len(), 1);
        let mut l = Log::new(80, 3, 100_000, 1 << 24, None);
        feed(&mut l, &"a a a a a\n".repeat(3000));
        let f = l.find("a", true);
        assert_eq!(f.len(), MAX_FOUND);
        // One text per line, shared by its matches.
        assert!(Arc::ptr_eq(&f[0].text, &f[4].text));
    }

    #[test]
    fn selection_and_copy() {
        let mut l = log(20, 4);
        feed(&mut l, "hello world  \nsecond (line)\n");
        l.start_selection(0, 1, SelectKind::Simple);
        l.update_selection(1, 2, true);
        assert_eq!(l.selection_text().as_deref(), Some("ello world\nsec"));
        let s = l.snapshot();
        assert!(s.cells[1].selected && s.cells[19].selected && s.cells[20 + 2].selected && !s.cells[20 + 3].selected && !s.cells[0].selected);
        l.start_selection(1, 9, SelectKind::Word);
        l.update_selection(1, 9, true);
        assert_eq!(l.selection_text().as_deref(), Some("line"));
        l.start_selection(0, 3, SelectKind::Line);
        l.update_selection(0, 3, true);
        assert_eq!(l.selection_text().as_deref(), Some("hello world\n"));
        l.clear_selection();
        assert_eq!(l.selection_text(), None);
    }

    #[test]
    fn tabs_cursor_and_clear() {
        let mut l = log(20, 4);
        feed(&mut l, "a\tb\n");
        assert_eq!(l.text(), "a\tb");
        assert_eq!(screen(&l)[0], "a       b");
        let mut l = log(20, 4);
        feed(&mut l, "name? ");
        assert_eq!(l.snapshot().cursor, Some((0, 6)));
        feed(&mut l, "\x1b[?25l");
        assert_eq!(l.snapshot().cursor, None);
        l.inject(b"\x1b[0m\x1b[2J\x1b[3J\x1b[H");
        assert_eq!(l.text(), "");
        assert_eq!(l.total_rows(), 1);
        feed(&mut l, "after\n");
        assert_eq!(l.text(), "after");
    }
}
