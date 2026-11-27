//! The work behind the screens: install, change and uninstall. Each step reports a fraction
//! and a line for the progress bar; they run on a background thread.

use brisk_platform::install::payload;
use brisk_platform::install::{self, Options};
use std::path::{Path, PathBuf};

pub const VERSION: &str = env!("CARGO_PKG_VERSION");

pub type Report<'a> = &'a (dyn Fn(f32, &str) + Sync);

/// Installs the zip this exe carries into `dir` (a new folder or an existing install, which
/// is updated in place) and registers it with `options`.
pub fn install(dir: &Path, options: &Options, report: Report) -> Result<(), String> {
    let exe = std::env::current_exe().map_err(|e| e.to_string())?;
    let at = payload::find(&exe).ok_or("this setup has no Brisk inside it (it is the uninstaller)")?;
    report(0.04, "Preparing the folder");
    std::fs::create_dir_all(dir).map_err(|e| format!("cannot create {}: {e}", dir.display()))?;
    let zip = dir.join(".brisk-setup.zip");
    report(0.10, "Unpacking Brisk");
    payload::extract(&exe, at, &zip).map_err(|e| format!("cannot write {}: {e}", zip.display()))?;
    report(0.30, "Copying files");
    let res = brisk_update::install(&zip, dir);
    let _ = std::fs::remove_file(&zip);
    res?;
    brisk_update::clean_old(dir);
    report(0.80, "Adding the uninstaller");
    let un = dir.join("uninstall.exe");
    if un.exists() {
        // An older uninstaller may be the file running a modify: move it aside first.
        let old = dir.join("uninstall.exe.brisk-old");
        let _ = std::fs::remove_file(&old);
        let _ = std::fs::rename(&un, &old);
    }
    payload::copy_head(&exe, at.0, &un).map_err(|e| format!("cannot write the uninstaller: {e}"))?;
    report(0.88, "Registering with Windows");
    install::register(dir, VERSION, options)?;
    report(1.0, "Done");
    Ok(())
}

/// Applies new options to the installed Brisk (Apps & features › Modify).
pub fn modify(dir: &Path, version: &str, options: &Options, report: Report) -> Result<(), String> {
    report(0.3, "Updating the Windows integration");
    install::register(dir, version, options)?;
    report(1.0, "Done");
    Ok(())
}

/// Removes the install: what it registered, then its folder; with `settings`, the user's
/// settings, consoles and saved passwords too.
pub fn uninstall(dir: &Path, settings: bool, report: Report) -> Result<(), String> {
    report(0.1, "Removing the Windows integration");
    install::unregister()?;
    report(0.35, "Deleting files");
    let left = remove_dir(dir);
    if settings {
        report(0.85, "Deleting settings");
        let cfg = brisk_platform::dirs::config_dir();
        // Only a folder named Brisk, never something the environment pointed elsewhere.
        if cfg.file_name().is_some_and(|n| n.eq_ignore_ascii_case("Brisk")) {
            let _ = remove_dir(&cfg);
        }
    }
    report(1.0, "Done");
    if left > 0 {
        return Err(format!("{left} file(s) were in use; Windows removes them at the next restart"));
    }
    Ok(())
}

/// Deletes `dir`; what cannot go now goes at the next restart. Returns how many were left.
fn remove_dir(dir: &Path) -> usize {
    if std::fs::remove_dir_all(dir).is_ok() || !dir.exists() {
        return 0;
    }
    let mut files = Vec::new();
    let mut dirs = vec![dir.to_path_buf()];
    let mut i = 0;
    while i < dirs.len() {
        for e in std::fs::read_dir(&dirs[i]).into_iter().flatten().flatten() {
            let p = e.path();
            if p.is_dir() {
                dirs.push(p);
            } else {
                files.push(p);
            }
        }
        i += 1;
    }
    let mut left = 0;
    for f in files {
        if std::fs::remove_file(&f).is_err() {
            install::delete_on_reboot(&f);
            left += 1;
        }
    }
    // Deepest first.
    for d in dirs.iter().rev() {
        if std::fs::remove_dir(d).is_err() {
            install::delete_on_reboot(d);
        }
    }
    left
}

/// An uninstaller running from the folder it deletes copies itself to the temp folder and
/// runs from there. Returns the copy to start, when that is needed.
pub fn relaunch_copy(dir: &Path) -> Option<PathBuf> {
    let exe = std::env::current_exe().ok()?;
    let inside = exe.parent().is_some_and(|p| p.to_string_lossy().to_lowercase().starts_with(&dir.to_string_lossy().to_lowercase()));
    if !inside {
        return None;
    }
    let tmp = std::env::temp_dir().join(format!("brisk-uninstall-{}.exe", std::process::id()));
    std::fs::copy(&exe, &tmp).ok()?;
    Some(tmp)
}

/// Starts the installed Brisk.
pub fn launch(dir: &Path) {
    // From the home folder: Brisk opens its current folder as a project when it has no
    // recent one, and the install folder is not a project.
    let home = std::env::var_os("USERPROFILE").map(std::path::PathBuf::from).unwrap_or_else(|| dir.to_path_buf());
    let _ = std::process::Command::new(dir.join("brisk.exe")).current_dir(home).spawn();
}
