//! Running Brisk windows find each other, so "Edit with Brisk" on a file of an open project
//! opens it in that window instead of starting a second one. Every window registers its
//! project root (a small file per process in `<config>/instances`) and listens on a local
//! channel only its user can write to (a named pipe on Windows); a new process with a file
//! or folder argument asks the matching window and exits when it answers.

use std::path::{Path, PathBuf};

#[cfg_attr(windows, path = "windows.rs")]
#[cfg_attr(not(windows), path = "unix.rs")]
mod sys;

/// What a new process asks a running window to do.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Request {
    /// Open this file in an editor tab.
    Open(PathBuf),
    /// Come to the front (the project itself was opened again).
    Focus,
}

impl Request {
    fn encode(&self) -> String {
        match self {
            Request::Open(p) => format!("open\t{}", p.display()),
            Request::Focus => "focus".into(),
        }
    }

    fn decode(s: &str) -> Option<Request> {
        match s.trim_end_matches(['\r', '\n']).split_once('\t') {
            Some(("open", p)) => Some(Request::Open(PathBuf::from(p))),
            None if s.trim() == "focus" => Some(Request::Focus),
            _ => None,
        }
    }
}

fn registry() -> PathBuf {
    crate::dirs::config_dir().join("instances")
}

/// Starts listening for this process's window of `root`; every request goes to `on`, from a
/// background thread. Registers the window until the process ends.
pub fn serve(root: &Path, on: impl Fn(Request) + Send + 'static) {
    let dir = registry();
    let _ = std::fs::create_dir_all(&dir);
    let me = std::process::id();
    let _ = std::fs::write(dir.join(me.to_string()), root.display().to_string());
    sys::serve(me, move |s| {
        if let Some(r) = Request::decode(&s) {
            on(r);
        }
    });
}

/// The running windows: (process id, project root). Entries of processes that are gone are
/// removed on the way.
fn windows() -> Vec<(u32, PathBuf)> {
    let me = std::process::id();
    std::fs::read_dir(registry())
        .into_iter()
        .flatten()
        .flatten()
        .filter_map(|e| {
            let pid: u32 = e.file_name().to_str()?.parse().ok()?;
            if pid == me {
                return None;
            }
            if !sys::alive(pid) {
                let _ = std::fs::remove_file(e.path());
                return None;
            }
            Some((pid, PathBuf::from(std::fs::read_to_string(e.path()).ok()?.trim())))
        })
        .collect()
}

fn norm(p: &Path) -> String {
    p.to_string_lossy().trim_start_matches(r"\\?\").trim_end_matches(['\\', '/']).replace('/', "\\").to_lowercase()
}

/// Hands `target` (a file or a folder) to a running window: a file to the window whose
/// project holds it (the deepest one), a folder to the window of that project. True when a
/// window took it, and this process should exit.
pub fn hand_over(target: &Path) -> bool {
    let t = norm(target);
    let wins = windows();
    let pick = if target.is_dir() {
        wins.iter().find(|(_, r)| norm(r) == t).map(|(pid, _)| (*pid, Request::Focus))
    } else {
        wins.iter()
            .filter(|(_, r)| {
                let r = norm(r);
                t.starts_with(&r) && t.as_bytes().get(r.len()) == Some(&b'\\')
            })
            .max_by_key(|(_, r)| norm(r).len())
            .map(|(pid, _)| (*pid, Request::Open(target.to_path_buf())))
    };
    match pick {
        Some((pid, req)) => sys::send(pid, &req.encode()),
        None => false,
    }
}

/// The project folder for a file opened on its own: the nearest folder above it with a
/// project marker (`.git`, `pyproject.toml`, `package.json`, …), else the file's folder.
pub fn project_root_of(file: &Path) -> PathBuf {
    const MARKERS: &[&str] =
        &[".git", ".brisk", ".idea", "pyproject.toml", "setup.py", "setup.cfg", "requirements.txt", "package.json", "Cargo.toml", "go.mod"];
    let dir = file.parent().unwrap_or(file);
    let home = crate::dirs::home();
    dir.ancestors()
        // The home folder itself is not a project, even with a stray package.json.
        .take_while(|d| home.as_deref() != Some(*d) && d.parent().is_some())
        .find(|d| MARKERS.iter().any(|m| d.join(m).exists()))
        .unwrap_or(dir)
        .to_path_buf()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn requests_round_trip() {
        for r in [Request::Open(PathBuf::from(r"C:\a b\ф.py")), Request::Focus] {
            assert_eq!(Request::decode(&r.encode()), Some(r));
        }
        assert_eq!(Request::decode("rm -rf"), None);
    }

    #[test]
    fn finds_the_project_root() {
        let d = std::env::temp_dir().join(format!("brisk-root-{}", std::process::id()));
        std::fs::create_dir_all(d.join("p/src/deep")).unwrap();
        std::fs::write(d.join("p/pyproject.toml"), "").unwrap();
        std::fs::write(d.join("p/src/deep/a.py"), "").unwrap();
        assert_eq!(project_root_of(&d.join("p/src/deep/a.py")), d.join("p"));
        std::fs::create_dir_all(d.join("loose")).unwrap();
        // No marker anywhere up to the temp folder's own parents: the file's folder.
        let f = d.join("loose/x.txt");
        std::fs::write(&f, "").unwrap();
        let root = project_root_of(&f);
        assert!(root == d.join("loose") || f.starts_with(&root));
        let _ = std::fs::remove_dir_all(&d);
    }
}
