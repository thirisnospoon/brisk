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

/// Replace `path`'s contents without a window where a crash or full disk leaves it truncated:
/// write a temp file next to it, flush it to disk, then swap it in. On Windows `ReplaceFileW`
/// keeps the original's ACLs, attributes and creation time; elsewhere the permissions are
/// copied. A read-only file fails with a clear error, as a plain write would.
pub fn write_atomic(path: &Path, bytes: &[u8]) -> std::io::Result<()> {
    use std::io::Write;
    // Through a symlink: replace the target, not the link.
    let path = match std::fs::symlink_metadata(path) {
        Ok(m) if m.file_type().is_symlink() => std::fs::canonicalize(path)?,
        _ => path.to_path_buf(),
    };
    let meta = std::fs::metadata(&path).ok();
    if meta.as_ref().is_some_and(|m| m.permissions().readonly()) {
        let name = path.file_name().map(|n| n.to_string_lossy().into_owned()).unwrap_or_default();
        return Err(std::io::Error::new(std::io::ErrorKind::PermissionDenied, format!("{name} is read-only")));
    }
    let dir = path.parent().filter(|d| !d.as_os_str().is_empty()).unwrap_or(Path::new("."));
    let name = path.file_name().map(|n| n.to_string_lossy().into_owned()).unwrap_or_default();
    static SEQ: std::sync::atomic::AtomicU32 = std::sync::atomic::AtomicU32::new(0);
    let n = SEQ.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
    let tmp = dir.join(format!(".{name}.{}-{n}.brisk-tmp", std::process::id()));
    let mut f = match std::fs::OpenOptions::new().write(true).create_new(true).open(&tmp) {
        Ok(f) => f,
        // A folder where only the file itself is writable: write in place rather than not at all.
        Err(e) if e.kind() == std::io::ErrorKind::PermissionDenied => return std::fs::write(&path, bytes),
        Err(e) => return Err(e),
    };
    if let Err(e) = f.write_all(bytes).and_then(|_| f.sync_all()) {
        // Disk full and the like: the original is untouched.
        drop(f);
        let _ = std::fs::remove_file(&tmp);
        return Err(e);
    }
    drop(f);
    match replace_atomic(&tmp, &path) {
        Ok(()) => Ok(()),
        Err(_) => {
            // E.g. another program holds the file open without delete sharing.
            let _ = std::fs::remove_file(&tmp);
            std::fs::write(&path, bytes)
        }
    }
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

    #[test]
    fn atomic_write_replaces_and_refuses_read_only() {
        let dir = std::env::temp_dir().join(format!("brisk-atomic-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        let f = dir.join("a.py");
        write_atomic(&f, b"new file").unwrap();
        assert_eq!(std::fs::read(&f).unwrap(), b"new file");
        write_atomic(&f, b"second").unwrap();
        assert_eq!(std::fs::read(&f).unwrap(), b"second");
        // No temp files left behind.
        assert_eq!(std::fs::read_dir(&dir).unwrap().count(), 1);
        let original = std::fs::metadata(&f).unwrap().permissions();
        let mut p = original.clone();
        p.set_readonly(true);
        std::fs::set_permissions(&f, p).unwrap();
        let e = write_atomic(&f, b"x").unwrap_err();
        assert_eq!(e.kind(), std::io::ErrorKind::PermissionDenied);
        assert!(e.to_string().contains("read-only"), "{e}");
        assert_eq!(std::fs::read(&f).unwrap(), b"second");
        std::fs::set_permissions(&f, original).unwrap();
        let _ = std::fs::remove_dir_all(&dir);
    }
}
