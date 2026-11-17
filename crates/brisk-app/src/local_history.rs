//! Local History, like PyCharm's: a copy of a file at every save, when it is opened with
//! contents we have not seen, after an external change and before a revert. Kept for 5 days
//! (at most 200 revisions per file) in `<config dir>/LocalHistory/<hash of the path>/`.

use std::path::{Path, PathBuf};
use std::time::{Duration, SystemTime, UNIX_EPOCH};

const KEEP: Duration = Duration::from_secs(5 * 24 * 3600);
const MAX_REVISIONS: usize = 200;
const MAX_BYTES: usize = 2 * 1024 * 1024;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Label {
    Saved,
    Opened,
    External,
    BeforeRevert,
}

impl Label {
    fn slug(self) -> &'static str {
        match self {
            Label::Saved => "saved",
            Label::Opened => "opened",
            Label::External => "external",
            Label::BeforeRevert => "before-revert",
        }
    }
    fn from_slug(s: &str) -> Self {
        match s {
            "opened" => Label::Opened,
            "external" => Label::External,
            "before-revert" => Label::BeforeRevert,
            _ => Label::Saved,
        }
    }
    pub fn title(self) -> &'static str {
        match self {
            Label::Saved => "Saved",
            Label::Opened => "Opened",
            Label::External => "External change",
            Label::BeforeRevert => "Before revert",
        }
    }
}

#[derive(Clone, Debug)]
pub struct Revision {
    /// Milliseconds since the Unix epoch.
    pub at: u64,
    pub label: Label,
    file: PathBuf,
}

impl Revision {
    pub fn text(&self) -> String {
        std::fs::read_to_string(&self.file).unwrap_or_default()
    }

    /// "just now", "5 min ago", "3 h ago", "2 days ago".
    pub fn age(&self) -> String {
        let secs = now_ms().saturating_sub(self.at) / 1000;
        match secs {
            0..=44 => "just now".into(),
            45..=3599 => format!("{} min ago", (secs + 30) / 60),
            3600..=86_399 => format!("{} h ago", secs / 3600),
            _ if secs < 2 * 86_400 => "yesterday".into(),
            _ => format!("{} days ago", secs / 86_400),
        }
    }
}

fn now_ms() -> u64 {
    SystemTime::now().duration_since(UNIX_EPOCH).map(|d| d.as_millis() as u64).unwrap_or(0)
}

fn root() -> PathBuf {
    brisk_db::config_dir().join("LocalHistory")
}

/// FNV-1a of the lowercased path: one folder per file.
fn dir_for(file: &Path) -> PathBuf {
    let key = file.to_string_lossy().replace('\\', "/").to_lowercase();
    let mut h: u64 = 0xcbf29ce484222325;
    for b in key.bytes() {
        h ^= b as u64;
        h = h.wrapping_mul(0x100000001b3);
    }
    root().join(format!("{h:016x}"))
}

/// Revisions of `file`, newest first.
pub fn revisions(file: &Path) -> Vec<Revision> {
    let Ok(rd) = std::fs::read_dir(dir_for(file)) else { return Vec::new() };
    let mut v: Vec<Revision> = rd
        .flatten()
        .filter_map(|e| {
            let name = e.file_name().to_string_lossy().to_string();
            let stem = name.strip_suffix(".txt")?;
            let (at, label) = stem.split_once('-')?;
            Some(Revision { at: at.parse().ok()?, label: Label::from_slug(label), file: e.path() })
        })
        .collect();
    v.sort_by(|a, b| b.at.cmp(&a.at));
    v
}

/// Keep `text` (with '\n' line ends) as a revision of `file`, unless it equals the newest one.
pub fn record(file: &Path, text: &str, label: Label) {
    if text.len() > MAX_BYTES || std::env::var_os("BRISK_NO_LOCAL_HISTORY").is_some() {
        return;
    }
    let revs = revisions(file);
    if revs.first().is_some_and(|r| r.text() == text) {
        return;
    }
    let dir = dir_for(file);
    if std::fs::create_dir_all(&dir).is_err() {
        return;
    }
    let _ = std::fs::write(dir.join("path.txt"), file.to_string_lossy().as_bytes());
    let mut at = now_ms();
    if let Some(r) = revs.first() {
        at = at.max(r.at + 1);
    }
    let _ = std::fs::write(dir.join(format!("{at}-{}.txt", label.slug())), text);
    // Old revisions go (always keeping the newest).
    let cutoff = now_ms().saturating_sub(KEEP.as_millis() as u64);
    for (i, r) in revs.iter().enumerate() {
        if i + 1 >= MAX_REVISIONS || r.at < cutoff {
            let _ = std::fs::remove_file(&r.file);
        }
    }
}

/// Lines added / removed going from `old` to `new` ("+3 −1").
pub fn change_summary(old: &str, new: &str) -> String {
    let rows = brisk_git::align(old, new);
    let (mut add, mut del) = (0, 0);
    for r in &rows {
        match r.kind {
            brisk_git::RowKind::Added => add += 1,
            brisk_git::RowKind::Deleted => del += 1,
            brisk_git::RowKind::Modified => {
                add += 1;
                del += 1;
            }
            _ => {}
        }
    }
    if add == 0 && del == 0 { "same as now".into() } else { format!("+{add} −{del} vs now") }
}
