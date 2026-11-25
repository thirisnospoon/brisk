//! Local History, like PyCharm's: a copy of a file at every save, when it is opened with
//! contents we have not seen, after an external change, before a revert, a refactoring, a
//! reformat or a delete. Kept for 5 days (at most 200 revisions per file, 200 MB in all) in
//! `<config dir>/LocalHistory/<hash of the path>/` as gzip (`<ms>-<label>.txt.gz`; older
//! plain `.txt` revisions are still read); user labels ("Put Label") in `labels.tsv` there.

use std::path::{Path, PathBuf};
use std::time::{Duration, SystemTime, UNIX_EPOCH};

const KEEP: Duration = Duration::from_secs(5 * 24 * 3600);
const MAX_REVISIONS: usize = 200;
const MAX_BYTES: usize = 2 * 1024 * 1024;
/// All revisions together: beyond it the oldest go first.
const MAX_TOTAL: u64 = 200 * 1024 * 1024;
/// Files and bytes kept when a folder is deleted.
const MAX_TREE_FILES: usize = 2000;
const MAX_TREE_BYTES: u64 = 32 * 1024 * 1024;
/// Files and folders [`record_path`] looks at in all, kept or not.
const MAX_TREE_ENTRIES: usize = 20_000;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Label {
    Saved,
    Opened,
    External,
    BeforeRevert,
    BeforeRefactor,
    BeforeReformat,
    Deleted,
}

impl Label {
    fn slug(self) -> &'static str {
        match self {
            Label::Saved => "saved",
            Label::Opened => "opened",
            Label::External => "external",
            Label::BeforeRevert => "before-revert",
            Label::BeforeRefactor => "before-refactor",
            Label::BeforeReformat => "before-reformat",
            Label::Deleted => "deleted",
        }
    }
    fn from_slug(s: &str) -> Self {
        match s {
            "opened" => Label::Opened,
            "external" => Label::External,
            "before-revert" => Label::BeforeRevert,
            "before-refactor" => Label::BeforeRefactor,
            "before-reformat" => Label::BeforeReformat,
            "deleted" => Label::Deleted,
            _ => Label::Saved,
        }
    }
    pub fn title(self) -> &'static str {
        match self {
            Label::Saved => "Saved",
            Label::Opened => "Opened",
            Label::External => "External change",
            Label::BeforeRevert => "Before revert",
            Label::BeforeRefactor => "Before refactoring",
            Label::BeforeReformat => "Before reformat",
            Label::Deleted => "Deleted",
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
        let Ok(bytes) = std::fs::read(&self.file) else { return String::new() };
        if self.file.extension().is_some_and(|e| e == "gz") {
            gunzip(&bytes).unwrap_or_default()
        } else {
            String::from_utf8_lossy(&bytes).into_owned()
        }
    }

    /// "just now", "5 min ago", "3 h ago", "2 days ago".
    pub fn age(&self) -> String {
        age(self.at)
    }
}

/// "just now", "5 min ago", "3 h ago", "2 days ago" for a time in ms since the epoch.
pub fn age(at: u64) -> String {
    let secs = now_ms().saturating_sub(at) / 1000;
    match secs {
        0..=44 => "just now".into(),
        45..=3599 => format!("{} min ago", (secs + 30) / 60),
        3600..=86_399 => format!("{} h ago", secs / 3600),
        _ if secs < 2 * 86_400 => "yesterday".into(),
        _ => format!("{} days ago", secs / 86_400),
    }
}

fn gzip(text: &str) -> std::io::Result<Vec<u8>> {
    use std::io::Write;
    let mut e = flate2::write::GzEncoder::new(Vec::new(), flate2::Compression::default());
    e.write_all(text.as_bytes())?;
    e.finish()
}

fn gunzip(bytes: &[u8]) -> Option<String> {
    use std::io::Read;
    let mut s = String::new();
    flate2::read::GzDecoder::new(bytes).read_to_string(&mut s).ok()?;
    Some(s)
}

fn now_ms() -> u64 {
    SystemTime::now().duration_since(UNIX_EPOCH).map(|d| d.as_millis() as u64).unwrap_or(0)
}

fn root() -> PathBuf {
    brisk_db::config_dir().join("LocalHistory")
}

/// A path as a comparable key: forward slashes, lowercase (Windows paths).
fn key(file: &Path) -> String {
    file.to_string_lossy().replace('\\', "/").to_lowercase()
}

/// FNV-1a of the lowercased path: one folder per file.
fn dir_for(file: &Path) -> PathBuf {
    let key = key(file);
    let mut h: u64 = 0xcbf29ce484222325;
    for b in key.bytes() {
        h ^= b as u64;
        h = h.wrapping_mul(0x100000001b3);
    }
    root().join(format!("{h:016x}"))
}

/// Revisions of `file`, newest first.
pub fn revisions(file: &Path) -> Vec<Revision> {
    revisions_in(&dir_for(file))
}

fn revisions_in(dir: &Path) -> Vec<Revision> {
    let Ok(rd) = std::fs::read_dir(dir) else { return Vec::new() };
    let mut v: Vec<Revision> = rd
        .flatten()
        .filter_map(|e| {
            let name = e.file_name().to_string_lossy().to_string();
            let stem = name.strip_suffix(".txt.gz").or_else(|| name.strip_suffix(".txt"))?;
            let (at, label) = stem.split_once('-')?;
            Some(Revision { at: at.parse().ok()?, label: Label::from_slug(label), file: e.path() })
        })
        .collect();
    v.sort_by(|a, b| b.at.cmp(&a.at));
    v
}

/// [`record`] on a worker thread, for callers on the UI thread without a `Context` (save,
/// reload): it reads the newest revision back and gzips. One thread keeps the order of
/// revisions of a file.
pub fn record_later(file: PathBuf, text: String, label: Label) {
    use std::sync::{mpsc, OnceLock};
    static TX: OnceLock<mpsc::Sender<(PathBuf, String, Label)>> = OnceLock::new();
    let tx = TX.get_or_init(|| {
        let (tx, rx) = mpsc::channel::<(PathBuf, String, Label)>();
        // If the thread cannot start, `rx` is dropped and every send fails into the fallback.
        let _ = std::thread::Builder::new().name("local-history".into()).spawn(move || {
            for (f, t, l) in rx {
                record(&f, &t, l);
            }
        });
        tx
    });
    if let Err(mpsc::SendError((f, t, l))) = tx.send((file, text, label)) {
        record(&f, &t, l);
    }
}

/// Keep `text` (with '\n' line ends) as a revision of `file`, unless it equals the newest one.
pub fn record(file: &Path, text: &str, label: Label) {
    if text.len() > MAX_BYTES || std::env::var_os("BRISK_NO_LOCAL_HISTORY").is_some() {
        return;
    }
    let revs = revisions(file);
    if revs.first().is_some_and(|r| r.text() == text) {
        // Same text: a delete still marks it, so the folder history shows the file as gone.
        if label != Label::Deleted {
            return;
        }
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
    // Gzip: source text shrinks 3-5 times, so 200 revisions of a file cost little disk.
    match gzip(text) {
        Ok(gz) => {
            let _ = std::fs::write(dir.join(format!("{at}-{}.txt.gz", label.slug())), gz);
        }
        Err(_) => {
            let _ = std::fs::write(dir.join(format!("{at}-{}.txt", label.slug())), text);
        }
    }
    // Old revisions go (always keeping the newest).
    let cutoff = now_ms().saturating_sub(KEEP.as_millis() as u64);
    for (i, r) in revs.iter().enumerate() {
        if i + 1 >= MAX_REVISIONS || r.at < cutoff {
            let _ = std::fs::remove_file(&r.file);
        }
    }
}

/// Keep `file` (or every text file under it, for a folder) as it is on disk, before a delete,
/// move or rename. Folders: at most [`MAX_TREE_FILES`] files, without VCS and venv folders.
/// Symlinks and junctions are not followed (a delete removes the link, not its target, and a
/// junction loop would never end), and the walk stops after [`MAX_TREE_ENTRIES`] entries so a
/// folder of 50k images does not freeze the UI thread it runs on.
pub fn record_path(path: &Path, label: Label) {
    let (mut n, mut bytes, mut seen) = (0, 0u64, 0usize);
    let mut stack = vec![path.to_path_buf()];
    while let Some(p) = stack.pop() {
        seen += 1;
        if seen > MAX_TREE_ENTRIES {
            return;
        }
        let Ok(meta) = std::fs::symlink_metadata(&p) else { continue };
        if meta.is_dir() {
            let Ok(rd) = std::fs::read_dir(&p) else { continue };
            for e in rd.flatten() {
                let name = e.file_name().to_string_lossy().to_string();
                if !matches!(name.as_str(), ".git" | ".venv" | "venv" | "node_modules" | "__pycache__" | ".idea" | ".brisk") {
                    stack.push(e.path());
                }
            }
        } else if meta.is_file() && meta.len() as usize <= MAX_BYTES {
            if n >= MAX_TREE_FILES || bytes + meta.len() > MAX_TREE_BYTES {
                return;
            }
            if let Ok(text) = std::fs::read_to_string(&p) {
                record(&p, &text.replace("\r\n", "\n"), label);
                n += 1;
                bytes += meta.len();
            }
        }
    }
}

/// A revision of some file under a folder (Show History on a folder).
#[derive(Clone, Debug)]
pub struct Change {
    pub path: PathBuf,
    pub rev: Revision,
}

/// The revisions of every file under `dir` (deleted ones too), newest first, at most `limit`.
pub fn history_under(dir: &Path, limit: usize) -> Vec<Change> {
    let prefix = key(dir);
    let prefix = if prefix.ends_with('/') { prefix } else { format!("{prefix}/") };
    let Ok(rd) = std::fs::read_dir(root()) else { return Vec::new() };
    let mut v = Vec::new();
    for e in rd.flatten() {
        let Ok(path) = std::fs::read_to_string(e.path().join("path.txt")) else { continue };
        let path = PathBuf::from(path.trim());
        if !key(&path).starts_with(&prefix) {
            continue;
        }
        for rev in revisions_in(&e.path()) {
            v.push(Change { path: path.clone(), rev });
        }
    }
    v.sort_by(|a, b| b.rev.at.cmp(&a.rev.at));
    v.truncate(limit);
    v
}

/// A named point in time of a project ("Put Label").
#[derive(Clone, Debug, PartialEq)]
pub struct UserLabel {
    pub at: u64,
    pub name: String,
}

fn labels_file() -> PathBuf {
    root().join("labels.tsv")
}

/// Put a label on the project at `project` now.
pub fn put_label(project: &Path, name: &str) -> std::io::Result<()> {
    use std::io::Write;
    std::fs::create_dir_all(root())?;
    let name = name.replace(['\t', '\n', '\r'], " ");
    let mut f = std::fs::OpenOptions::new().create(true).append(true).open(labels_file())?;
    writeln!(f, "{}\t{}\t{}", now_ms(), key(project), name.trim())
}

/// The labels of the project that contains `path` (or is `path`), newest first.
pub fn labels_for(path: &Path) -> Vec<UserLabel> {
    let Ok(text) = std::fs::read_to_string(labels_file()) else { return Vec::new() };
    parse_labels(&text, &key(path), now_ms().saturating_sub(KEEP.as_millis() as u64))
}

fn parse_labels(text: &str, k: &str, cutoff: u64) -> Vec<UserLabel> {
    let mut v: Vec<UserLabel> = text
        .lines()
        .filter_map(|l| {
            let mut it = l.splitn(3, '\t');
            let at: u64 = it.next()?.parse().ok()?;
            let project = it.next()?.trim_end_matches('/');
            let name = it.next()?.to_string();
            let inside = k == project || k.starts_with(&format!("{project}/"));
            (inside && at >= cutoff).then_some(UserLabel { at, name })
        })
        .collect();
    v.sort_by(|a, b| b.at.cmp(&a.at));
    v
}

/// Drop revisions older than [`KEEP`] (the newest of a file stays), then the oldest ones
/// until all fit in [`MAX_TOTAL`]; empty folders and old labels go too. Returns (files
/// removed, bytes kept). Run once at startup, off the UI thread.
pub fn cleanup() -> (usize, u64) {
    cleanup_in(&root(), now_ms().saturating_sub(KEEP.as_millis() as u64), MAX_TOTAL)
}

fn cleanup_in(root: &Path, cutoff: u64, max_total: u64) -> (usize, u64) {
    let Ok(rd) = std::fs::read_dir(root) else { return (0, 0) };
    let mut removed = 0;
    // (at, size, file)
    let mut all: Vec<(u64, u64, PathBuf)> = Vec::new();
    for e in rd.flatten() {
        if !e.file_type().is_ok_and(|t| t.is_dir()) {
            continue;
        }
        let revs = revisions_in(&e.path());
        if revs.is_empty() {
            let _ = std::fs::remove_dir_all(e.path());
            continue;
        }
        for (i, r) in revs.iter().enumerate() {
            if i > 0 && r.at < cutoff {
                if std::fs::remove_file(&r.file).is_ok() {
                    removed += 1;
                }
                continue;
            }
            let size = std::fs::metadata(&r.file).map(|m| m.len()).unwrap_or(0);
            all.push((r.at, size, r.file.clone()));
        }
    }
    let mut total: u64 = all.iter().map(|a| a.1).sum();
    if total > max_total {
        all.sort_by_key(|a| a.0);
        for (_, size, file) in &all {
            if total <= max_total {
                break;
            }
            if std::fs::remove_file(file).is_ok() {
                removed += 1;
                total -= size;
                if let Some(dir) = file.parent() {
                    if revisions_in(dir).is_empty() {
                        let _ = std::fs::remove_dir_all(dir);
                    }
                }
            }
        }
    }
    let labels = root.join("labels.tsv");
    if let Ok(text) = std::fs::read_to_string(&labels) {
        let keep: Vec<&str> = text.lines().filter(|l| l.split('\t').next().and_then(|a| a.parse::<u64>().ok()).is_some_and(|a| a >= cutoff)).collect();
        if keep.len() != text.lines().count() {
            let _ = std::fs::write(&labels, keep.iter().map(|l| format!("{l}\n")).collect::<String>());
        }
    }
    (removed, total)
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
    if add == 0 && del == 0 {
        "same as now".into()
    } else {
        format!("+{add} −{del} vs now")
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn rev(dir: &Path, at: u64, text: &str) {
        std::fs::create_dir_all(dir).unwrap();
        std::fs::write(dir.join(format!("{at}-saved.txt")), text).unwrap();
    }

    #[test]
    fn cleanup_keeps_newest_and_fits_budget() {
        let root = std::env::temp_dir().join(format!("brisk-lh-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&root);
        let a = root.join("a");
        let b = root.join("b");
        // a: two old revisions and a recent one; b: only an old one (the newest stays).
        rev(&a, 100, "old1");
        rev(&a, 200, "old2");
        rev(&a, 5000, "new");
        rev(&b, 150, "b-old");
        std::fs::write(root.join("labels.tsv"), "50\tc:/p\told\n6000\tc:/p\tnew\n").unwrap();
        let (removed, total) = cleanup_in(&root, 1000, u64::MAX);
        assert_eq!((removed, total), (2, 3 + 5));
        assert_eq!(revisions_in(&a).iter().map(|r| r.at).collect::<Vec<_>>(), vec![5000]);
        assert_eq!(revisions_in(&b).len(), 1);
        assert_eq!(std::fs::read_to_string(root.join("labels.tsv")).unwrap(), "6000\tc:/p\tnew\n");
        // A 4-byte budget drops the oldest ("b-old", 5 bytes) and its folder, keeps "new".
        let (removed, total) = cleanup_in(&root, 0, 4);
        assert_eq!((removed, total), (1, 3));
        assert!(!b.exists());
        let _ = std::fs::remove_dir_all(&root);
    }

    #[test]
    fn gzip_round_trip() {
        let dir = std::env::temp_dir().join(format!("brisk-lh-gz-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let text = "def f():\n    return 'привет'\n".repeat(50);
        std::fs::write(dir.join("7-saved.txt.gz"), gzip(&text).unwrap()).unwrap();
        std::fs::write(dir.join("5-opened.txt"), "plain").unwrap();
        let revs = revisions_in(&dir);
        assert_eq!(revs.iter().map(|r| (r.at, r.label)).collect::<Vec<_>>(), vec![(7, Label::Saved), (5, Label::Opened)]);
        assert_eq!(revs[0].text(), text);
        assert_eq!(revs[1].text(), "plain");
        assert!(std::fs::metadata(dir.join("7-saved.txt.gz")).unwrap().len() < text.len() as u64 / 5);
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn labels_belong_to_their_project() {
        let text = "100\tc:/work/app\tbefore upgrade\n200\tc:/work/other\tx\n300\tc:/work/app\tfixed\n";
        let v = parse_labels(text, "c:/work/app/src/main.py", 0);
        assert_eq!(v.iter().map(|l| l.name.as_str()).collect::<Vec<_>>(), vec!["fixed", "before upgrade"]);
        assert!(parse_labels(text, "c:/work/application/x.py", 0).is_empty());
        assert_eq!(parse_labels(text, "c:/work/app", 150).len(), 1);
    }
}
