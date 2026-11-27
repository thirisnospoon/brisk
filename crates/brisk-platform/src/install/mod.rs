//! An installed Brisk (as opposed to the portable zip): where it goes, what it registers with
//! the system and how that is undone. The installer writes it, the in-app updater keeps the
//! version current, the uninstaller removes it. On Windows everything is per user (`HKCU`,
//! `%LOCALAPPDATA%\Programs\Brisk`), so no step needs administrator rights; other systems
//! have no installer yet and get errors.

use std::path::{Path, PathBuf};

pub mod payload;
#[cfg_attr(windows, path = "windows.rs")]
#[cfg_attr(not(windows), path = "unix.rs")]
mod sys;

/// File types Brisk offers to open: extension and a name for the list.
pub const FILE_TYPES: &[(&str, &str)] = &[
    ("py", "Python"),
    ("pyi", "Python stub"),
    ("ipynb", "Jupyter notebook"),
    ("js", "JavaScript"),
    ("mjs", "JavaScript module"),
    ("cjs", "CommonJS module"),
    ("jsx", "JSX"),
    ("ts", "TypeScript"),
    ("tsx", "TSX"),
    ("json", "JSON"),
    ("jsonc", "JSON with comments"),
    ("html", "HTML"),
    ("css", "CSS"),
    ("scss", "SCSS"),
    ("less", "Less"),
    ("vue", "Vue"),
    ("md", "Markdown"),
    ("toml", "TOML"),
    ("yaml", "YAML"),
    ("yml", "YAML"),
    ("xml", "XML"),
    ("sql", "SQL"),
    ("ini", "INI"),
    ("cfg", "Config"),
    ("env", "Environment file"),
    ("txt", "Text"),
    ("log", "Log"),
    ("sh", "Shell script"),
    ("ps1", "PowerShell script"),
    ("bat", "Batch file"),
    ("csv", "CSV"),
];

/// The file types ticked by default: code and config, not text or logs that open better in
/// what the user has.
pub const DEFAULT_TYPES: &[&str] =
    &["py", "pyi", "js", "mjs", "cjs", "jsx", "ts", "tsx", "json", "jsonc", "css", "scss", "less", "vue", "md", "toml", "yaml", "yml", "sql"];

/// What an install sets up besides the files.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Options {
    /// A Start menu shortcut.
    pub start_menu: bool,
    pub desktop: bool,
    /// `brisk` on the user's PATH (`brisk .`, `brisk file.py`).
    pub path: bool,
    /// "Edit with Brisk" on files, "Open in Brisk" on folders and folder backgrounds.
    pub context_menu: bool,
    /// Extensions listed under "Open with" (Windows lets only the user make an app the default).
    pub file_types: Vec<String>,
}

impl Default for Options {
    fn default() -> Self {
        Options { start_menu: true, desktop: false, path: true, context_menu: true, file_types: DEFAULT_TYPES.iter().map(|s| s.to_string()).collect() }
    }
}

/// An install found on the system.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Installed {
    pub dir: PathBuf,
    pub version: String,
    pub options: Options,
}

/// Where a new install goes: `%LOCALAPPDATA%\Programs\Brisk`.
pub fn default_dir() -> Option<PathBuf> {
    sys::default_dir()
}

/// The install registered for this user, if any.
pub fn installed() -> Option<Installed> {
    sys::installed()
}

/// Whether the running exe is the registered install (not a portable copy or a build).
pub fn is_installed_exe() -> bool {
    let (Ok(exe), Some(i)) = (std::env::current_exe(), installed()) else { return false };
    exe.parent().is_some_and(|d| same_dir(d, &i.dir))
}

fn same_dir(a: &Path, b: &Path) -> bool {
    let norm = |p: &Path| p.to_string_lossy().trim_end_matches(['\\', '/']).to_lowercase();
    norm(a) == norm(b)
}

/// Registers the files in `dir` as Brisk `version`: the uninstall entry, the app path, the
/// shell commands and file types, PATH and shortcuts as `options` say. Run again to change
/// the options; what is no longer wanted is removed.
pub fn register(dir: &Path, version: &str, options: &Options) -> Result<(), String> {
    sys::register(dir, version, options, dir_size_kb(dir))
}

/// The version shown in Apps & features, after an in-app update.
pub fn set_version(version: &str) -> Result<(), String> {
    sys::set_version(version)
}

/// Removes everything `register` set up (the files stay; the uninstaller deletes them).
pub fn unregister() -> Result<(), String> {
    sys::unregister()
}

/// The size of the files under `dir` in KB, for Apps & features.
pub fn dir_size_kb(dir: &Path) -> u64 {
    let mut total = 0u64;
    let mut stack = vec![dir.to_path_buf()];
    while let Some(d) = stack.pop() {
        for e in std::fs::read_dir(&d).into_iter().flatten().flatten() {
            match e.metadata() {
                Ok(m) if m.is_dir() => stack.push(e.path()),
                Ok(m) => total += m.len(),
                Err(_) => {}
            }
        }
    }
    total / 1024
}

/// Brisk processes running from `dir` (they hold its files): what the installer and the
/// uninstaller wait for.
pub fn running_in(dir: &Path) -> Vec<u32> {
    let me = std::process::id();
    crate::process::processes()
        .into_iter()
        .filter(|p| p.pid != me && p.name.eq_ignore_ascii_case(if cfg!(windows) { "brisk.exe" } else { "brisk" }))
        .filter(|p| sys::exe_of(p.pid).is_some_and(|e| e.parent().is_some_and(|d| same_dir(d, dir))))
        .map(|p| p.pid)
        .collect()
}

/// Asks Windows to delete `path` (a file or an empty folder) at the next restart: what an
/// uninstaller cannot remove while it runs.
pub fn delete_on_reboot(path: &Path) {
    sys::delete_on_reboot(path);
}

/// Replaces the application manifest (resource 1) of `exe` with `xml`: the setup build
/// carries GPUI's manifest, and the release step swaps in one that also says "no
/// elevation", which Windows would otherwise guess wrong for a file named `setup`.
pub fn set_manifest(exe: &Path, xml: &str) -> Result<(), String> {
    sys::set_manifest(exe, xml)
}
