//! Updates from GitHub Releases.
//!
//! - [`latest`] asks GitHub for the latest release and [`is_newer`] compares its version with
//!   the running one.
//! - [`download`] fetches the release's zip, reporting the bytes received so far.
//! - [`install`] unpacks it next to the running exe and swaps the files in: a running exe
//!   cannot be overwritten on Windows, but it can be renamed, so every replaced file is
//!   renamed to `<name>.brisk-old` first and the new one takes its name. The next start
//!   removes the old files ([`clean_old`]).
//!
//! Only a portable install (a `portable` file next to the exe, as in the release zip) is
//! updated in place; [`install_dir`] says where that is.
//!
//! HTTP goes through the system's `curl` and unpacking through its `tar` (both ship with
//! Windows 10 and 11), so the IDE carries no HTTP or TLS code for this. `BRISK_UPDATE_FEED`
//! replaces the GitHub URL, for tests: curl also reads `file://` URLs.

use std::path::{Path, PathBuf};
use std::process::Stdio;
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::time::{Duration, Instant};

/// The running version.
pub const CURRENT: &str = env!("CARGO_PKG_VERSION");
/// The latest release, as JSON.
pub const FEED: &str = "https://api.github.com/repos/thirisnospoon/brisk/releases/latest";
/// All releases, for people who update by hand.
pub const RELEASES_PAGE: &str = "https://github.com/thirisnospoon/brisk/releases";
/// What replaced files are renamed to until the next start.
pub const OLD_SUFFIX: &str = ".brisk-old";
/// How often the automatic check runs.
pub const CHECK_EVERY: Duration = Duration::from_secs(24 * 60 * 60);

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Asset {
    pub name: String,
    pub url: String,
    /// Bytes; 0 when GitHub did not say.
    pub size: u64,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Release {
    /// `0.5.0`, without the tag's `v`.
    pub version: String,
    /// The release title, e.g. `Brisk 0.5.0`.
    pub title: String,
    /// The release notes, Markdown.
    pub notes: String,
    /// `2026-11-26`, or empty.
    pub date: String,
    /// The release on GitHub.
    pub page: String,
    /// The package for this system, if the release has one.
    pub asset: Option<Asset>,
}

/// The name ending of this system's package.
fn asset_suffix() -> Option<&'static str> {
    if cfg!(all(windows, target_arch = "x86_64")) {
        Some("-windows-x64.zip")
    } else {
        None
    }
}

/// A release from GitHub's JSON (`/releases/latest` or `/releases/tags/<tag>`).
pub fn parse_release(json: &str) -> Result<Release, String> {
    let v: serde_json::Value = serde_json::from_str(json).map_err(|e| format!("unexpected answer from GitHub: {e}"))?;
    let s = |k: &str| v.get(k).and_then(|x| x.as_str()).unwrap_or("").to_string();
    let tag = s("tag_name");
    if tag.is_empty() {
        let msg = s("message");
        return Err(if msg.is_empty() { "GitHub sent no release".into() } else { format!("GitHub: {msg}") });
    }
    let version = tag.trim_start_matches(['v', 'V']).to_string();
    let suffix = asset_suffix();
    let asset = v.get("assets").and_then(|a| a.as_array()).and_then(|list| {
        list.iter().find_map(|a| {
            let name = a.get("name")?.as_str()?;
            if !suffix.is_some_and(|s| name.ends_with(s)) {
                return None;
            }
            Some(Asset {
                name: name.to_string(),
                url: a.get("browser_download_url")?.as_str()?.to_string(),
                size: a.get("size").and_then(|x| x.as_u64()).unwrap_or(0),
            })
        })
    });
    let title = match s("name") {
        t if t.is_empty() => format!("Brisk {version}"),
        t => t,
    };
    let date = s("published_at").get(..10).unwrap_or("").to_string();
    Ok(Release { version, title, notes: s("body").replace("\r\n", "\n"), date, page: s("html_url"), asset })
}

/// The numeric parts of a version (`v0.10.2-beta` → `[0, 10, 2]`) and whether it is a
/// pre-release (has a `-` part).
fn parts(v: &str) -> (Vec<u64>, bool) {
    let v = v.trim().trim_start_matches(['v', 'V']);
    let (core, pre) = match v.split_once('-') {
        Some((c, _)) => (c, true),
        None => (v, false),
    };
    (core.split('.').map(|p| p.trim().parse().unwrap_or(0)).collect(), pre)
}

/// Whether `candidate` is a later version than `current`. `0.5.0-beta` is earlier than
/// `0.5.0` and later than `0.4.9`.
pub fn is_newer(candidate: &str, current: &str) -> bool {
    let (mut a, a_pre) = parts(candidate);
    let (mut b, b_pre) = parts(current);
    let n = a.len().max(b.len());
    a.resize(n, 0);
    b.resize(n, 0);
    match a.cmp(&b) {
        std::cmp::Ordering::Equal => b_pre && !a_pre,
        o => o == std::cmp::Ordering::Greater,
    }
}

/// The feed to ask: `BRISK_UPDATE_FEED` if set, else GitHub.
pub fn feed_url() -> String {
    std::env::var("BRISK_UPDATE_FEED").ok().filter(|s| !s.is_empty()).unwrap_or_else(|| FEED.to_string())
}

/// Whether the feed was replaced (tests and smokes).
pub fn feed_overridden() -> bool {
    std::env::var_os("BRISK_UPDATE_FEED").is_some_and(|s| !s.is_empty())
}

/// A system tool: `%SystemRoot%\System32\<name>.exe` on Windows (not whatever is first on
/// PATH), `name` elsewhere.
fn system_tool(name: &str) -> PathBuf {
    if cfg!(windows) {
        if let Some(root) = std::env::var_os("SystemRoot") {
            let p = PathBuf::from(root).join("System32").join(format!("{name}.exe"));
            if p.is_file() {
                return p;
            }
        }
    }
    PathBuf::from(name)
}

fn curl(url: &str) -> std::process::Command {
    let mut c = brisk_platform::process::command(system_tool("curl"));
    c.args(["--silent", "--show-error", "--fail", "--location", "--connect-timeout", "10"])
        .args(["-H", &format!("User-Agent: Brisk/{CURRENT}")])
        .args(["-H", "Accept: application/vnd.github+json"])
        .arg(url)
        .stdin(Stdio::null());
    c
}

/// The error curl printed, or how it exited.
fn curl_error(stderr: &[u8], status: std::process::ExitStatus) -> String {
    let e = String::from_utf8_lossy(stderr);
    let e = e.trim().trim_start_matches("curl: ").trim();
    if e.is_empty() {
        format!("curl exited with {status}")
    } else {
        // "(6) Could not resolve host: api.github.com" reads better without the number.
        let e = e.strip_prefix('(').and_then(|r| r.split_once(") ")).map_or(e, |(_, m)| m);
        e.to_string()
    }
}

/// Where the release of `version` is described: GitHub's `/releases/tags/v<version>`, or the
/// replaced feed itself.
pub fn release_url(version: &str) -> String {
    if feed_overridden() {
        return feed_url();
    }
    FEED.replace("/latest", &format!("/tags/v{version}"))
}

/// The release of `version` (What's New for the running one). Blocks like [`latest`].
pub fn release(version: &str) -> Result<Release, String> {
    fetch(&release_url(version))
}

fn fetch(url: &str) -> Result<Release, String> {
    let out = curl(url).args(["--max-time", "30"]).output().map_err(|e| format!("cannot start curl: {e}"))?;
    if !out.status.success() {
        return Err(curl_error(&out.stderr, out.status));
    }
    parse_release(&String::from_utf8_lossy(&out.stdout))
}

/// The latest release, from [`feed_url`]. Blocks for up to about 30 s; call it off the UI
/// thread.
pub fn latest() -> Result<Release, String> {
    fetch(&feed_url())
}

/// Downloads `asset` to `dest`, keeping `got` at the bytes written so far. Stops with an
/// error when `cancel` is set. Blocks; call it off the UI thread.
pub fn download(asset: &Asset, dest: &Path, got: &AtomicU64, cancel: &AtomicBool) -> Result<(), String> {
    if let Some(dir) = dest.parent() {
        std::fs::create_dir_all(dir).map_err(|e| format!("cannot create {}: {e}", dir.display()))?;
    }
    let _ = std::fs::remove_file(dest);
    let mut child =
        curl(&asset.url).arg("--output").arg(dest).stdout(Stdio::null()).stderr(Stdio::piped()).spawn().map_err(|e| format!("cannot start curl: {e}"))?;
    let started = Instant::now();
    let status = loop {
        if cancel.load(Ordering::Relaxed) {
            let _ = child.kill();
            let _ = child.wait();
            let _ = std::fs::remove_file(dest);
            return Err("cancelled".into());
        }
        if let Some(s) = child.try_wait().map_err(|e| e.to_string())? {
            break s;
        }
        if started.elapsed() > Duration::from_secs(30 * 60) {
            let _ = child.kill();
            let _ = child.wait();
            return Err("the download took more than 30 minutes".into());
        }
        got.store(std::fs::metadata(dest).map(|m| m.len()).unwrap_or(0), Ordering::Relaxed);
        std::thread::sleep(Duration::from_millis(100));
    };
    let mut err = Vec::new();
    if let Some(mut e) = child.stderr.take() {
        let _ = std::io::Read::read_to_end(&mut e, &mut err);
    }
    if !status.success() {
        let _ = std::fs::remove_file(dest);
        return Err(curl_error(&err, status));
    }
    let len = std::fs::metadata(dest).map(|m| m.len()).unwrap_or(0);
    got.store(len, Ordering::Relaxed);
    if asset.size > 0 && len != asset.size {
        let _ = std::fs::remove_file(dest);
        return Err(format!("the download is incomplete ({len} of {} bytes)", asset.size));
    }
    Ok(())
}

/// The folder that can be updated in place: the running exe's folder when it has the
/// `portable` marker of the release zip, or when it is the installed Brisk (per user, so
/// writable without administrator rights).
pub fn install_dir() -> Option<PathBuf> {
    let exe = std::env::current_exe().ok()?;
    let dir = exe.parent()?;
    (dir.join("portable").is_file() || brisk_platform::install::is_installed_exe()).then(|| dir.to_path_buf())
}

/// Unpacks the release zip into `dir` and swaps its files in. Returns how many files were
/// replaced or added. On a failure the files already swapped are put back.
pub fn install(zip: &Path, dir: &Path) -> Result<usize, String> {
    let staging = dir.join(".brisk-update");
    let _ = std::fs::remove_dir_all(&staging);
    std::fs::create_dir_all(&staging).map_err(|e| format!("cannot write to {}: {e}", dir.display()))?;
    let out = brisk_platform::process::command(system_tool("tar"))
        .arg("-xf")
        .arg(zip)
        .arg("-C")
        .arg(&staging)
        .stdin(Stdio::null())
        .output()
        .map_err(|e| format!("cannot start tar: {e}"))?;
    if !out.status.success() {
        let _ = std::fs::remove_dir_all(&staging);
        return Err(format!("cannot unpack the update: {}", String::from_utf8_lossy(&out.stderr).trim()));
    }
    // An installed Brisk stays installed: the zip's `portable` marker is not copied in.
    let installed = !dir.join("portable").exists();
    let res = package_root(&staging).and_then(|root| {
        if installed {
            let _ = std::fs::remove_file(root.join("portable"));
        }
        let version = root.file_name().and_then(|n| n.to_str()).and_then(|n| n.strip_prefix("Brisk-")).and_then(|n| n.split('-').next()).map(str::to_string);
        let n = swap(&root, dir)?;
        // Apps & features shows the new version.
        if let (true, Some(v)) = (installed && brisk_platform::install::is_installed_exe(), version) {
            let _ = brisk_platform::install::set_version(&v);
        }
        Ok(n)
    });
    let _ = std::fs::remove_dir_all(&staging);
    res
}

/// The folder inside the unpacked zip that holds `brisk.exe` (the zip has one top folder,
/// `Brisk-<version>-windows-x64`).
fn package_root(staging: &Path) -> Result<PathBuf, String> {
    let exe = if cfg!(windows) { "brisk.exe" } else { "brisk" };
    if staging.join(exe).is_file() {
        return Ok(staging.to_path_buf());
    }
    let dirs: Vec<PathBuf> = std::fs::read_dir(staging).map_err(|e| e.to_string())?.flatten().map(|e| e.path()).filter(|p| p.is_dir()).collect();
    match dirs.as_slice() {
        [one] if one.join(exe).is_file() => Ok(one.clone()),
        _ => Err(format!("the update has no {exe}")),
    }
}

/// Every file under `root`, relative to it.
fn files(root: &Path) -> Vec<PathBuf> {
    let mut out = Vec::new();
    let mut stack = vec![root.to_path_buf()];
    while let Some(d) = stack.pop() {
        let Ok(rd) = std::fs::read_dir(&d) else { continue };
        for e in rd.flatten() {
            let p = e.path();
            if p.is_dir() {
                stack.push(p);
            } else if let Ok(rel) = p.strip_prefix(root) {
                out.push(rel.to_path_buf());
            }
        }
    }
    out.sort();
    out
}

fn old_name(p: &Path) -> PathBuf {
    let mut s = p.as_os_str().to_os_string();
    s.push(OLD_SUFFIX);
    PathBuf::from(s)
}

/// Moves `p` out of the way to `<p>.brisk-old`, or `<p>.<n>.brisk-old` when an older one
/// cannot be removed (it belongs to an exe that still runs).
fn retire(p: &Path) -> std::io::Result<PathBuf> {
    let mut old = old_name(p);
    if old.exists() && std::fs::remove_file(&old).is_err() {
        let mut n = 1;
        loop {
            let mut s = p.as_os_str().to_os_string();
            s.push(format!(".{n}{OLD_SUFFIX}"));
            old = PathBuf::from(s);
            if !old.exists() || std::fs::remove_file(&old).is_ok() {
                break;
            }
            n += 1;
        }
    }
    std::fs::rename(p, &old)?;
    Ok(old)
}

/// Swaps the files of the new package at `new_root` into `dir`. Files at the top of `dir`
/// that the package does not have stay (the `data` folder of a portable install, the
/// user's own files); inside the package's folders (`lib`), files it no longer has are
/// retired too, so an old debugpy does not linger next to the new one.
fn swap(new_root: &Path, dir: &Path) -> Result<usize, String> {
    let new_files = files(new_root);
    let mut stale = Vec::new();
    let top_dirs: std::collections::BTreeSet<PathBuf> =
        new_files.iter().filter(|f| f.components().count() > 1).filter_map(|f| f.components().next()).map(|c| PathBuf::from(c.as_os_str())).collect();
    for t in &top_dirs {
        for f in files(&dir.join(t)) {
            let rel = t.join(&f);
            if !new_files.contains(&rel) && !rel.to_string_lossy().ends_with(OLD_SUFFIX) {
                stale.push(rel);
            }
        }
    }
    // (target, where the old file went) for the rollback.
    let mut done: Vec<(PathBuf, Option<PathBuf>)> = Vec::new();
    let rollback = |done: &[(PathBuf, Option<PathBuf>)]| {
        for (target, old) in done.iter().rev() {
            let _ = std::fs::remove_file(target);
            if let Some(old) = old {
                let _ = std::fs::rename(old, target);
            }
        }
    };
    for rel in &new_files {
        let target = dir.join(rel);
        let step = (|| -> std::io::Result<Option<PathBuf>> {
            if let Some(parent) = target.parent() {
                std::fs::create_dir_all(parent)?;
            }
            let old = if target.exists() { Some(retire(&target)?) } else { None };
            if let Err(e) = std::fs::rename(new_root.join(rel), &target) {
                if let Some(old) = &old {
                    let _ = std::fs::rename(old, &target);
                }
                return Err(e);
            }
            Ok(old)
        })();
        match step {
            Ok(old) => done.push((target, old)),
            Err(e) => {
                rollback(&done);
                return Err(format!("cannot replace {}: {e}", target.display()));
            }
        }
    }
    for rel in &stale {
        let _ = retire(&dir.join(rel));
    }
    Ok(new_files.len())
}

/// Removes the files a previous update retired, and the folders that held only them. Files
/// of an exe that still runs stay until a later start.
pub fn clean_old(dir: &Path) {
    let _ = std::fs::remove_dir_all(dir.join(".brisk-update"));
    for rel in files(dir) {
        if rel.starts_with("data") || !rel.to_string_lossy().ends_with(OLD_SUFFIX) {
            continue;
        }
        let p = dir.join(&rel);
        if std::fs::remove_file(&p).is_ok() {
            // Folders a release dropped (an old dist-info) go once they are empty.
            let mut parent = p.parent();
            while let Some(d) = parent.filter(|d| *d != dir) {
                if std::fs::remove_dir(d).is_err() {
                    break;
                }
                parent = d.parent();
            }
        }
    }
}

/// What the automatic check remembers between starts (`update.json` in the settings folder).
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct State {
    /// Seconds since 1970 of the last check that got an answer.
    pub last_check: u64,
    /// A version the user chose to skip.
    pub skipped: Option<String>,
    /// The newer release the last check found: the title bar shows it on later starts
    /// without asking GitHub again.
    pub available: Option<Release>,
}

fn release_json(r: &Release) -> serde_json::Value {
    let mut v = serde_json::json!({ "version": r.version, "title": r.title, "notes": r.notes, "date": r.date, "page": r.page });
    if let Some(a) = &r.asset {
        v["asset"] = serde_json::json!({ "name": a.name, "url": a.url, "size": a.size });
    }
    v
}

fn release_from(v: &serde_json::Value) -> Option<Release> {
    let s = |v: &serde_json::Value, k: &str| v.get(k).and_then(|x| x.as_str()).unwrap_or("").to_string();
    let version = s(v, "version");
    if version.is_empty() {
        return None;
    }
    let asset = v.get("asset").map(|a| Asset { name: s(a, "name"), url: s(a, "url"), size: a.get("size").and_then(|x| x.as_u64()).unwrap_or(0) });
    Some(Release { version, title: s(v, "title"), notes: s(v, "notes"), date: s(v, "date"), page: s(v, "page"), asset })
}

impl State {
    pub fn load(path: &Path) -> State {
        let Ok(text) = std::fs::read_to_string(path) else { return State::default() };
        let Ok(v) = serde_json::from_str::<serde_json::Value>(&text) else { return State::default() };
        State {
            last_check: v.get("last_check").and_then(|x| x.as_u64()).unwrap_or(0),
            skipped: v.get("skipped").and_then(|x| x.as_str()).map(str::to_string),
            available: v.get("available").and_then(release_from),
        }
    }

    pub fn save(&self, path: &Path) -> std::io::Result<()> {
        let mut v = serde_json::json!({ "last_check": self.last_check });
        if let Some(s) = &self.skipped {
            v["skipped"] = s.clone().into();
        }
        if let Some(r) = &self.available {
            v["available"] = release_json(r);
        }
        if let Some(dir) = path.parent() {
            std::fs::create_dir_all(dir)?;
        }
        std::fs::write(path, serde_json::to_string_pretty(&v).unwrap_or_default())
    }

    /// Whether the automatic check should run at `now` (seconds since 1970).
    pub fn due(&self, now: u64) -> bool {
        now.saturating_sub(self.last_check) >= CHECK_EVERY.as_secs() || now < self.last_check
    }
}

/// Seconds since 1970.
pub fn now() -> u64 {
    std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).map_or(0, |d| d.as_secs())
}

/// `37.4 MB`.
pub fn megabytes(bytes: u64) -> String {
    format!("{:.1} MB", bytes as f64 / (1024. * 1024.))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn temp(name: &str) -> PathBuf {
        let d = std::env::temp_dir().join(format!("brisk-update-test-{name}-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&d);
        std::fs::create_dir_all(&d).unwrap();
        d
    }

    fn write(p: &Path, text: &str) {
        std::fs::create_dir_all(p.parent().unwrap()).unwrap();
        std::fs::write(p, text).unwrap();
    }

    #[test]
    fn versions() {
        assert!(is_newer("0.5.0", "0.4.0"));
        assert!(is_newer("v0.10.0", "0.9.3"));
        assert!(is_newer("1.0", "0.99.99"));
        assert!(!is_newer("0.4.0", "0.4.0"));
        assert!(!is_newer("v0.4.0", "0.4.0"));
        assert!(!is_newer("0.3.9", "0.4.0"));
        assert!(is_newer("0.5.0", "0.5.0-beta"));
        assert!(!is_newer("0.5.0-beta", "0.5.0"));
        assert!(is_newer("0.5.0-beta", "0.4.9"));
        assert!(!is_newer("0.4.0.0", "0.4"));
    }

    #[test]
    fn parses_github_release() {
        let json = r###"{
            "tag_name": "v0.5.0", "name": "Brisk 0.5.0", "html_url": "https://github.com/x/brisk/releases/tag/v0.5.0",
            "published_at": "2026-12-01T10:00:00Z", "body": "## Highlights\r\n\r\n- One",
            "assets": [
                {"name": "Brisk-0.5.0-linux-x64.tar.gz", "browser_download_url": "https://x/l", "size": 1},
                {"name": "Brisk-0.5.0-windows-x64.zip", "browser_download_url": "https://x/w", "size": 39224176}
            ]
        }"###;
        let r = parse_release(json).unwrap();
        assert_eq!(r.version, "0.5.0");
        assert_eq!(r.title, "Brisk 0.5.0");
        assert_eq!(r.date, "2026-12-01");
        assert_eq!(r.notes, "## Highlights\n\n- One");
        if cfg!(all(windows, target_arch = "x86_64")) {
            assert_eq!(r.asset, Some(Asset { name: "Brisk-0.5.0-windows-x64.zip".into(), url: "https://x/w".into(), size: 39224176 }));
        } else {
            assert_eq!(r.asset, None);
        }
    }

    #[test]
    fn github_errors_are_readable() {
        assert_eq!(parse_release(r#"{"message": "Not Found"}"#).unwrap_err(), "GitHub: Not Found");
        assert!(parse_release("<html>").unwrap_err().starts_with("unexpected answer"));
        let r = parse_release(r#"{"tag_name": "0.6.0"}"#).unwrap();
        assert_eq!((r.title.as_str(), r.date.as_str(), r.asset), ("Brisk 0.6.0", "", None));
    }

    /// Asks the real GitHub: `cargo test -p brisk-update -- --ignored`.
    #[test]
    #[ignore]
    fn live_github() {
        let latest = latest().unwrap();
        assert!(!is_newer(CURRENT, &latest.version), "{CURRENT} is newer than the latest release {}", latest.version);
        assert!(latest.notes.contains("##"), "{latest:?}");
        let this = release(CURRENT).unwrap();
        assert_eq!(this.version, CURRENT);
        if cfg!(all(windows, target_arch = "x86_64")) {
            let a = this.asset.unwrap();
            assert!(a.url.starts_with("https://github.com/") && a.size > 1_000_000, "{a:?}");
        }
    }

    #[test]
    fn state_round_trip_and_due() {
        let d = temp("state");
        let p = d.join("update.json");
        assert_eq!(State::load(&p), State::default());
        let asset = Some(Asset { name: "brisk-0.5.0-windows-x64.zip".into(), url: "https://x/y.zip".into(), size: 42 });
        let available = Some(Release {
            version: "0.5.1".into(),
            title: "Brisk 0.5.1".into(),
            notes: "- One\n- Two".into(),
            date: "2026-11-27".into(),
            page: "https://x".into(),
            asset,
        });
        let s = State { last_check: 1000, skipped: Some("0.5.0".into()), available };
        s.save(&p).unwrap();
        assert_eq!(State::load(&p), s);
        assert!(!s.due(1000 + 3600));
        assert!(s.due(1000 + CHECK_EVERY.as_secs()));
        assert!(s.due(10), "a clock set back checks again");
        std::fs::write(&p, "not json").unwrap();
        assert_eq!(State::load(&p), State::default());
        let _ = std::fs::remove_dir_all(&d);
    }

    #[test]
    fn swap_replaces_adds_and_retires() {
        let d = temp("swap");
        let (new, dir) = (d.join("new"), d.join("install"));
        write(&dir.join("brisk.exe"), "old exe");
        write(&dir.join("README.md"), "old readme");
        write(&dir.join("notes.txt"), "the user's");
        write(&dir.join("data/settings.json"), "{}");
        write(&dir.join("lib/ty.exe"), "old ty");
        write(&dir.join("lib/debugpy-1.0.dist-info/METADATA"), "old meta");
        write(&dir.join("lib/debugpy/gone.py"), "old module");
        write(&new.join("brisk.exe"), "new exe");
        write(&new.join("README.md"), "new readme");
        write(&new.join("lib/ty.exe"), "new ty");
        write(&new.join("lib/debugpy-2.0.dist-info/METADATA"), "new meta");
        write(&new.join("lib/debugpy/__init__.py"), "new module");

        assert_eq!(swap(&new, &dir).unwrap(), 5);
        let read = |p: &str| std::fs::read_to_string(dir.join(p)).ok();
        assert_eq!(read("brisk.exe").as_deref(), Some("new exe"));
        assert_eq!(read("brisk.exe.brisk-old").as_deref(), Some("old exe"));
        assert_eq!(read("lib/ty.exe").as_deref(), Some("new ty"));
        assert_eq!(read("lib/debugpy/__init__.py").as_deref(), Some("new module"));
        assert_eq!(read("lib/debugpy/gone.py"), None);
        assert_eq!(read("lib/debugpy-1.0.dist-info/METADATA"), None);
        assert_eq!(read("notes.txt").as_deref(), Some("the user's"));
        assert_eq!(read("data/settings.json").as_deref(), Some("{}"));

        clean_old(&dir);
        let left: Vec<String> = files(&dir).iter().map(|p| p.to_string_lossy().replace('\\', "/")).collect();
        assert_eq!(
            left,
            ["README.md", "brisk.exe", "data/settings.json", "lib/debugpy/__init__.py", "lib/debugpy-2.0.dist-info/METADATA", "lib/ty.exe", "notes.txt"]
        );
        assert!(!dir.join("lib/debugpy-1.0.dist-info").exists(), "an emptied folder goes too");
        let _ = std::fs::remove_dir_all(&d);
    }

    #[test]
    fn retire_steps_aside_when_an_old_copy_is_kept() {
        let d = temp("retire");
        let p = d.join("brisk.exe");
        write(&p, "v2");
        write(&old_name(&p), "v1");
        let old = retire(&p).unwrap();
        assert_eq!(std::fs::read_to_string(&old).unwrap(), "v2");
        assert!(!p.exists());
        let _ = std::fs::remove_dir_all(&d);
    }

    #[cfg(windows)]
    #[test]
    fn install_from_a_zip_and_download_from_a_file_url() {
        let d = temp("install");
        let pkg = d.join("src/Brisk-9.9.9-windows-x64");
        write(&pkg.join("brisk.exe"), "new exe");
        write(&pkg.join("lib/ty.exe"), "new ty");
        let zip = d.join("Brisk-9.9.9-windows-x64.zip");
        let st = std::process::Command::new(system_tool("tar"))
            .current_dir(d.join("src"))
            .args(["-a", "-c", "-f"])
            .arg(&zip)
            .arg("Brisk-9.9.9-windows-x64")
            .status()
            .unwrap();
        assert!(st.success());

        // file:// through curl, as the smoke feed does.
        let size = std::fs::metadata(&zip).unwrap().len();
        let url = format!("file:///{}", zip.to_string_lossy().replace('\\', "/"));
        let asset = Asset { name: "Brisk-9.9.9-windows-x64.zip".into(), url, size };
        let got = AtomicU64::new(0);
        let dl = d.join("dl/update.zip");
        download(&asset, &dl, &got, &AtomicBool::new(false)).unwrap();
        assert_eq!(got.load(Ordering::Relaxed), size);
        let short = Asset { size: size + 1, ..asset.clone() };
        assert!(download(&short, &dl, &got, &AtomicBool::new(false)).unwrap_err().contains("incomplete"));
        download(&asset, &dl, &got, &AtomicBool::new(false)).unwrap();

        let dir = d.join("install");
        write(&dir.join("brisk.exe"), "old exe");
        write(&dir.join("portable"), "");
        assert_eq!(install(&dl, &dir).unwrap(), 2);
        assert_eq!(std::fs::read_to_string(dir.join("brisk.exe")).unwrap(), "new exe");
        assert_eq!(std::fs::read_to_string(dir.join("lib/ty.exe")).unwrap(), "new ty");
        assert!(!dir.join(".brisk-update").exists());
        let _ = std::fs::remove_dir_all(&d);
    }
}
