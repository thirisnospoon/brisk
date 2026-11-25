//! File system operations whose best form differs between operating systems.

use std::path::Path;

#[cfg_attr(windows, path = "windows.rs")]
#[cfg_attr(not(windows), path = "unix.rs")]
mod sys;

/// Put the finished file `tmp` in place of `target`, in one step, so a reader sees either the
/// old or the new contents. An existing `target` keeps its metadata: on Windows
/// `ReplaceFileW` keeps its ACLs, attributes and creation time; elsewhere its permissions are
/// copied before the rename. On error `tmp` is left for the caller to remove.
pub fn replace_atomic(tmp: &Path, target: &Path) -> std::io::Result<()> {
    let meta = std::fs::metadata(target).ok();
    if meta.is_some() && sys::replace_file(target, tmp) {
        return Ok(());
    }
    // Unix mode bits (on Windows only the read-only flag).
    if let Some(m) = &meta {
        let _ = std::fs::set_permissions(tmp, m.permissions());
    }
    std::fs::rename(tmp, target)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn replace_atomic_round_trip() {
        let dir = std::env::temp_dir().join(format!("brisk-platform-fs-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        let target = dir.join("a.txt");
        let tmp = dir.join("a.txt.tmp");
        // A new file.
        std::fs::write(&tmp, b"first").unwrap();
        replace_atomic(&tmp, &target).unwrap();
        assert_eq!(std::fs::read(&target).unwrap(), b"first");
        assert!(!tmp.exists());
        // Over an existing file.
        std::fs::write(&tmp, b"second").unwrap();
        replace_atomic(&tmp, &target).unwrap();
        assert_eq!(std::fs::read(&target).unwrap(), b"second");
        assert!(!tmp.exists());
        assert_eq!(std::fs::read_dir(&dir).unwrap().count(), 1);
        let _ = std::fs::remove_dir_all(&dir);
    }
}
