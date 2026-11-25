//! The folder picker's model: a typed path split into the folder to list and the name being
//! typed, the subfolders that match it, and where a path can start (drives, home). Plain
//! filesystem work; listing a folder may be slow (network drives), so callers run
//! [`list_dirs`] off the UI thread.

use std::path::{Path, PathBuf};

/// Files and folders whose presence marks a folder as a project.
const PROJECT_MARKERS: &[&str] = &[".git", "pyproject.toml", "setup.py", "requirements.txt", ".idea", ".brisk"];

/// System folders nobody opens as a project; shown only when typed.
const SYSTEM: &[&str] = &["$recycle.bin", "system volume information", "$windows.~ws", "$winreagent", "config.msi", "recovery"];

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Folder {
    pub name: String,
    pub path: PathBuf,
    /// Looks like a project: has `.git`, `pyproject.toml` and the like.
    pub project: bool,
}

fn is_sep(c: char) -> bool {
    c == '/' || c == '\\'
}

/// The user's home folder.
pub fn home() -> Option<PathBuf> {
    std::env::var_os(if cfg!(windows) { "USERPROFILE" } else { "HOME" }).filter(|h| !h.is_empty()).map(PathBuf::from)
}

/// A typed path as the folder to list and the start of the name typed after it:
/// `C:\code\br` → (`C:\code\`, `br`); `C:\code\` → (`C:\code\`, ``). `~` stands for `home`.
/// `None` for the folder when nothing names one yet (empty input, `C:` without a separator).
pub fn split_query(q: &str, home: Option<&Path>) -> (Option<PathBuf>, String) {
    let expanded;
    let q = match (q.strip_prefix('~'), home) {
        (Some(rest), Some(h)) if rest.is_empty() || rest.starts_with(is_sep) => {
            expanded = format!("{}{}{rest}", h.display(), if rest.is_empty() { std::path::MAIN_SEPARATOR_STR } else { "" });
            expanded.as_str()
        }
        _ => q,
    };
    match q.rfind(is_sep) {
        Some(i) => (Some(PathBuf::from(&q[..=i])), q[i + 1..].to_string()),
        None => (None, q.to_string()),
    }
}

/// The folder's subfolders by name, case-insensitively. Each one is checked for project
/// markers, so this touches the disk once per subfolder: call it off the UI thread.
pub fn list_dirs(dir: &Path) -> std::io::Result<Vec<Folder>> {
    let mut v: Vec<Folder> = std::fs::read_dir(dir)?
        .flatten()
        // A link to a folder counts as one (`file_type` does not follow it, `is_dir` does).
        .filter(|e| e.file_type().is_ok_and(|t| t.is_dir()) || e.path().is_dir())
        .map(|e| {
            let path = e.path();
            let project = PROJECT_MARKERS.iter().any(|m| path.join(m).exists());
            Folder { name: e.file_name().to_string_lossy().into_owned(), path, project }
        })
        .collect();
    v.sort_by_cached_key(|f| f.name.to_lowercase());
    Ok(v)
}

/// The folders whose name matches `typed`: names that start with it first, then names that
/// contain it, each group in listing order. Hidden (`.name`) and system folders only when
/// the typed name asks for them.
pub fn matching<'a>(all: &'a [Folder], typed: &str) -> Vec<&'a Folder> {
    let t = typed.to_lowercase();
    let shown = |f: &&Folder| {
        let n = f.name.to_lowercase();
        let hidden = n.starts_with('.') || n.starts_with('$') || SYSTEM.contains(&n.as_str());
        !hidden || (!t.is_empty() && n.starts_with(&t))
    };
    let (mut prefix, mut inside) = (Vec::new(), Vec::new());
    for f in all.iter().filter(shown) {
        let n = f.name.to_lowercase();
        if n.starts_with(&t) {
            prefix.push(f);
        } else if n.contains(&t) {
            inside.push(f);
        }
    }
    prefix.extend(inside);
    prefix
}

/// Where a path can start: the drives on Windows, `/` elsewhere.
pub fn roots() -> Vec<PathBuf> {
    #[cfg(windows)]
    {
        // A bit mask of the drive letters; asks no drive anything, so a dead network drive
        // does not hang it.
        let mask = unsafe { windows_sys::Win32::Storage::FileSystem::GetLogicalDrives() };
        (0..26u8).filter(|i| mask & (1 << i) != 0).map(|i| PathBuf::from(format!("{}:\\", (b'A' + i) as char))).collect()
    }
    #[cfg(not(windows))]
    {
        vec![PathBuf::from("/")]
    }
}

/// `dir` as typed text that lists its contents: with a trailing separator.
pub fn as_query(dir: &Path) -> String {
    let s = dir.to_string_lossy().into_owned();
    if s.ends_with(is_sep) {
        s
    } else {
        format!("{s}{}", std::path::MAIN_SEPARATOR)
    }
}

/// A new folder's name is usable: not empty, no separators or characters Windows forbids,
/// not `.` / `..`, not ending in a dot or space.
pub fn valid_new_name(name: &str) -> bool {
    !name.is_empty()
        && name != "."
        && name != ".."
        && !name.ends_with(['.', ' '])
        && !name.chars().any(|c| c.is_control() || matches!(c, '<' | '>' | ':' | '"' | '/' | '\\' | '|' | '?' | '*'))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn folder(name: &str) -> Folder {
        Folder { name: name.into(), path: PathBuf::from(name), project: false }
    }

    #[test]
    fn split_query_into_folder_and_typed_name() {
        assert_eq!(split_query(r"C:\code\br", None), (Some(PathBuf::from(r"C:\code\")), "br".into()));
        assert_eq!(split_query(r"C:\code\", None), (Some(PathBuf::from(r"C:\code\")), String::new()));
        assert_eq!(split_query("/home/me/pro", None), (Some(PathBuf::from("/home/me/")), "pro".into()));
        assert_eq!(split_query("C:", None), (None, "C:".into()));
        assert_eq!(split_query("", None), (None, String::new()));
        // Multi-byte names split on characters, not bytes.
        assert_eq!(split_query("/проекты/жук", None), (Some(PathBuf::from("/проекты/")), "жук".into()));
    }

    #[test]
    fn tilde_is_home() {
        let h = Path::new("/home/me");
        let sep = std::path::MAIN_SEPARATOR;
        assert_eq!(split_query("~", Some(h)), (Some(PathBuf::from(format!("/home/me{sep}"))), String::new()));
        assert_eq!(split_query("~/code/x", Some(h)), (Some(PathBuf::from("/home/me/code/")), "x".into()));
        // Not a home reference: left as typed.
        assert_eq!(split_query("~x", Some(h)), (None, "~x".into()));
    }

    #[test]
    fn matching_puts_prefixes_first_and_hides_dot_folders() {
        let all: Vec<Folder> = ["$Recycle.Bin", ".cache", "brisk", "my-brisk", "Bristol", "notes"].into_iter().map(folder).collect();
        let names = |t: &str| matching(&all, t).into_iter().map(|f| f.name.clone()).collect::<Vec<_>>();
        assert_eq!(names(""), ["brisk", "my-brisk", "Bristol", "notes"]);
        assert_eq!(names("bris"), ["brisk", "Bristol", "my-brisk"]);
        assert_eq!(names("."), [".cache"]);
        assert_eq!(names("$r"), ["$Recycle.Bin"]);
    }

    #[test]
    fn lists_subfolders_and_marks_projects() {
        let d = std::env::temp_dir().join(format!("brisk-folders-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&d);
        std::fs::create_dir_all(d.join("b-app/.git")).unwrap();
        std::fs::create_dir_all(d.join("A-docs")).unwrap();
        std::fs::write(d.join("file.txt"), "").unwrap();
        let v = list_dirs(&d).unwrap();
        assert_eq!(v.iter().map(|f| (f.name.as_str(), f.project)).collect::<Vec<_>>(), [("A-docs", false), ("b-app", true)]);
        let _ = std::fs::remove_dir_all(&d);
    }

    #[test]
    fn new_names() {
        assert!(valid_new_name("my-project"));
        assert!(valid_new_name("проект"));
        for bad in ["", ".", "..", "a/b", r"a\b", "a:b", "x.", "x ", "a*"] {
            assert!(!valid_new_name(bad), "{bad:?}");
        }
    }

    #[test]
    fn roots_exist() {
        assert!(!roots().is_empty());
        assert!(as_query(Path::new("/x")).ends_with(std::path::MAIN_SEPARATOR));
    }
}
