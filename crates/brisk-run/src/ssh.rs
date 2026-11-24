//! SSH interpreters, like PyCharm's: a Python on another machine reached through the system
//! `ssh` (so `~/.ssh/config`, keys and the agent work as in a terminal; Brisk never asks for
//! or stores a password). The project is uploaded to a folder on the host before each run
//! (only what changed), the run executes there, and paths in its output map back to the
//! project.

use crate::{Command, RunConfig, RunKind};
use serde::{Deserialize, Serialize};
use std::collections::HashMap;
use std::io::Write;
use std::path::{Path, PathBuf};
use std::process::Stdio;

/// The scheme of an SSH interpreter in `run.json`'s `interpreter`: `ssh://dev-server/usr/bin/python3`.
pub const SCHEME: &str = "ssh://";

/// Marks a run's command as remote, with `<remote root>\t<local root>`, so the console maps
/// paths in the output back to the project.
pub const MAP_ENV: &str = "BRISK_REMOTE_MAP";
/// The host of a remote run's command (what to sync to before it starts).
pub const HOST_ENV: &str = "BRISK_REMOTE_HOST";

#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct SshInterpreter {
    /// What `ssh` takes: an alias from `~/.ssh/config` or `user@host`, optionally `:port`.
    pub host: String,
    /// Absolute path of the interpreter on the host.
    pub python: String,
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub version: String,
    /// The project's folder on the host.
    #[serde(rename = "remoteRoot")]
    pub remote_root: String,
}

impl SshInterpreter {
    pub fn uri(&self) -> String {
        format!("{SCHEME}{}{}", self.host, self.python)
    }

    /// "Remote Python 3.12 (dev-server)".
    pub fn label(&self) -> String {
        let v: String = self.version.split('.').take(2).collect::<Vec<_>>().join(".");
        let v = if v.is_empty() { "Python".to_string() } else { format!("Python {v}") };
        format!("Remote {v} ({})", self.host)
    }
}

/// `ssh://host/abs/python` → (host, python).
pub fn parse_uri(s: &str) -> Option<(String, String)> {
    let rest = s.strip_prefix(SCHEME)?;
    let slash = rest.find('/')?;
    let (host, python) = rest.split_at(slash);
    (!host.is_empty()).then(|| (host.to_string(), python.to_string()))
}

/// The `ssh` program: on PATH, else Windows' built-in OpenSSH.
pub fn ssh_exe() -> PathBuf {
    #[cfg(windows)]
    {
        let sys = PathBuf::from(std::env::var("SystemRoot").unwrap_or_else(|_| r"C:\Windows".into())).join(r"System32\OpenSSH\ssh.exe");
        let on_path = std::env::var_os("PATH").and_then(|p| std::env::split_paths(&p).map(|d| d.join("ssh.exe")).find(|p| p.is_file()));
        // Without Windows' OpenSSH feature, the one Git for Windows ships.
        let git = || {
            ["ProgramFiles", "ProgramW6432", "LOCALAPPDATA"]
                .iter()
                .filter_map(std::env::var_os)
                .flat_map(|d| [PathBuf::from(&d).join(r"Git\usr\bin\ssh.exe"), PathBuf::from(&d).join(r"Programs\Git\usr\bin\ssh.exe")])
                .find(|p| p.is_file())
        };
        on_path.or_else(|| sys.is_file().then(|| sys.clone())).or_else(git).unwrap_or(sys)
    }
    #[cfg(not(windows))]
    {
        PathBuf::from("ssh")
    }
}

/// Options for every connection: never prompt (no password or host key questions in a
/// hidden process), give up on a dead host quickly. `user@host:port` becomes `-p port`.
pub fn connect_args(host: &str) -> Vec<String> {
    let mut a = vec!["-o".to_string(), "BatchMode=yes".into(), "-o".into(), "ConnectTimeout=15".into(), "-o".into(), "ServerAliveInterval=30".into()];
    match host.rsplit_once(':').filter(|(_, p)| !p.is_empty() && p.bytes().all(|b| b.is_ascii_digit())) {
        Some((h, port)) => {
            a.extend(["-p".to_string(), port.to_string(), h.to_string()]);
        }
        None => a.push(host.to_string()),
    }
    a
}

/// A single-quoted shell word.
pub fn quote(s: &str) -> String {
    if !s.is_empty() && s.bytes().all(|b| b.is_ascii_alphanumeric() || b"/._-=:,+@%".contains(&b)) {
        return s.to_string();
    }
    format!("'{}'", s.replace('\'', r"'\''"))
}

fn ssh_command(host: &str) -> std::process::Command {
    let mut c = std::process::Command::new(ssh_exe());
    c.args(connect_args(host));
    #[cfg(windows)]
    {
        use std::os::windows::process::CommandExt;
        c.creation_flags(0x0800_0000); // CREATE_NO_WINDOW
    }
    c
}

/// Runs a shell script on the host; its stdout, or the error ssh or the script printed.
pub fn run_script(host: &str, script: &str, input: Option<&[u8]>) -> Result<String, String> {
    let mut c = ssh_command(host);
    c.arg(script).stdin(if input.is_some() { Stdio::piped() } else { Stdio::null() }).stdout(Stdio::piped()).stderr(Stdio::piped());
    let mut child = c.spawn().map_err(|e| format!("Cannot start ssh: {e}"))?;
    if let (Some(data), Some(mut stdin)) = (input, child.stdin.take()) {
        let data = data.to_vec();
        std::thread::spawn(move || {
            let _ = stdin.write_all(&data);
        });
    }
    let out = child.wait_with_output().map_err(|e| e.to_string())?;
    if out.status.success() {
        Ok(String::from_utf8_lossy(&out.stdout).into_owned())
    } else {
        let err = String::from_utf8_lossy(&out.stderr);
        let line = err.lines().rev().find(|l| !l.trim().is_empty()).unwrap_or("").trim().to_string();
        Err(if line.is_empty() { format!("ssh {host} failed ({})", out.status) } else { line })
    }
}

/// Host aliases from `~/.ssh/config` (no wildcards), for the host list.
pub fn config_hosts() -> Vec<String> {
    let Some(home) = std::env::var_os("USERPROFILE").or_else(|| std::env::var_os("HOME")) else { return Vec::new() };
    let Ok(text) = std::fs::read_to_string(PathBuf::from(home).join(".ssh").join("config")) else { return Vec::new() };
    let mut out = Vec::new();
    for line in text.lines() {
        let t = line.trim();
        let Some(rest) = t.strip_prefix("Host ").or_else(|| t.strip_prefix("host ")).or_else(|| t.strip_prefix("Host\t")) else { continue };
        if let Some(first) = rest.split_whitespace().next().filter(|h| !h.contains(['*', '?', '!'])) {
            if !out.iter().any(|h| h == first) {
                out.push(first.to_string());
            }
        }
    }
    out
}

/// What [`probe`] found on a host: its home folder and the Pythons with their versions.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct HostInfo {
    pub home: String,
    pub pythons: Vec<(String, String)>,
}

const PROBE: &str = r#"echo "HOME $HOME"
for p in python3 python python3.14 python3.13 python3.12 python3.11 python3.10 python3.9 \
    "$HOME"/.venv/bin/python "$HOME"/venv/bin/python "$HOME"/*/.venv/bin/python "$HOME"/*/venv/bin/python \
    "$HOME"/.pyenv/shims/python "$HOME"/miniconda3/bin/python "$HOME"/anaconda3/bin/python /opt/conda/bin/python; do
  f=$(command -v "$p" 2>/dev/null) || continue
  case "$f" in /*) ;; *) continue ;; esac
  v=$("$f" -c 'import sys; print(".".join(map(str, sys.version_info[:3])))' 2>/dev/null) || continue
  echo "PY $f $v"
done"#;

/// Connects and lists the host's Pythons (`which` ones and common venv / conda places).
pub fn probe(host: &str) -> Result<HostInfo, String> {
    let out = run_script(host, PROBE, None)?;
    Ok(parse_probe(&out))
}

fn parse_probe(out: &str) -> HostInfo {
    let mut info = HostInfo::default();
    for line in out.lines() {
        if let Some(h) = line.strip_prefix("HOME ") {
            info.home = h.trim().to_string();
        } else if let Some(rest) = line.strip_prefix("PY ") {
            if let Some((path, v)) = rest.rsplit_once(' ') {
                if !info.pythons.iter().any(|(p, _)| p == path) {
                    info.pythons.push((path.to_string(), v.to_string()));
                }
            }
        }
    }
    info
}

/// The default project folder on the host: `<home>/brisk-projects/<project name>`.
pub fn default_remote_root(home: &str, root: &Path) -> String {
    let name: String = root.file_name().map(|n| n.to_string_lossy().into_owned()).unwrap_or_else(|| "project".into());
    let name: String = name.chars().map(|c| if c.is_ascii_alphanumeric() || "._-".contains(c) { c } else { '_' }).collect();
    format!("{}/brisk-projects/{name}", home.trim_end_matches('/'))
}

// --- Upload ---------------------------------------------------------------------------------

/// What one sync did.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct SyncStats {
    pub uploaded: usize,
    pub deleted: usize,
    pub bytes: u64,
}

/// Files over this size are not uploaded (datasets, models): the run reports them missing.
const MAX_FILE: u64 = 50 << 20;

#[derive(Serialize, Deserialize, Default)]
struct Manifest {
    /// Written to `<remote root>/.brisk-sync` by the last sync; when the host's copy differs
    /// (the folder was deleted or another machine synced), everything is uploaded again.
    id: String,
    files: HashMap<String, (u64, u64)>,
}

fn manifest_path(host: &str, root: &Path, remote_root: &str) -> PathBuf {
    use std::hash::{Hash, Hasher};
    let mut h = std::collections::hash_map::DefaultHasher::new();
    (host, root, remote_root).hash(&mut h);
    std::env::temp_dir().join("brisk-ssh-sync").join(format!("{:016x}.json", h.finish()))
}

/// Uploads the project's files (`.gitignore`d ones, `.git`, venvs and caches excluded) to
/// `remote_root`: only files changed since the last sync, and removes files deleted locally.
pub fn sync(host: &str, root: &Path, remote_root: &str) -> Result<SyncStats, String> {
    let list = brisk_project::FileList::scan(root);
    let mut now: HashMap<String, (u64, u64)> = HashMap::new();
    for rel in list.files {
        let Ok(m) = std::fs::metadata(root.join(&rel)) else { continue };
        if m.len() > MAX_FILE {
            continue;
        }
        let mtime = m.modified().ok().and_then(|t| t.duration_since(std::time::UNIX_EPOCH).ok()).map(|d| d.as_millis() as u64).unwrap_or(0);
        now.insert(rel, (m.len(), mtime));
    }
    let mpath = manifest_path(host, root, remote_root);
    let old: Manifest = std::fs::read(&mpath).ok().and_then(|b| serde_json::from_slice(&b).ok()).unwrap_or_default();
    let rr = quote(remote_root);
    let remote_id = run_script(host, &format!("cat {rr}/.brisk-sync 2>/dev/null; true"), None)?.trim().to_string();
    let fresh = old.id.is_empty() || remote_id != old.id;
    let mut changed: Vec<&String> = now.iter().filter(|(k, v)| fresh || old.files.get(*k) != Some(v)).map(|(k, _)| k).collect();
    changed.sort();
    let deleted: Vec<&String> = if fresh { Vec::new() } else { old.files.keys().filter(|k| !now.contains_key(*k)).collect() };
    let id = format!("{:x}", std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).map(|d| d.as_nanos()).unwrap_or(0));
    let mut stats = SyncStats { uploaded: changed.len(), deleted: deleted.len(), bytes: 0 };
    let mut tar = Vec::new();
    for rel in &changed {
        let data = std::fs::read(root.join(rel.as_str())).map_err(|e| format!("{rel}: {e}"))?;
        stats.bytes += data.len() as u64;
        tar_entry(&mut tar, rel, &data, now[*rel].1 / 1000)?;
    }
    tar.extend(std::iter::repeat_n(0u8, 1024));
    let mut script = format!("set -e\nmkdir -p {rr}\ncd {rr}\n");
    for d in &deleted {
        script.push_str(&format!("rm -f -- {}\n", quote(d)));
    }
    script.push_str(&format!("tar -xmf - 2>/dev/null || tar -xf -\necho {id} > .brisk-sync\n"));
    run_script(host, &script, Some(&tar))?;
    let m = Manifest { id, files: now };
    if let Some(dir) = mpath.parent() {
        let _ = std::fs::create_dir_all(dir);
    }
    let _ = std::fs::write(&mpath, serde_json::to_vec(&m).unwrap_or_default());
    Ok(stats)
}

/// One ustar entry; long paths use the 155-byte prefix field.
fn tar_entry(out: &mut Vec<u8>, path: &str, data: &[u8], mtime: u64) -> Result<(), String> {
    let mut h = [0u8; 512];
    let bytes = path.as_bytes();
    let (prefix, name) = if bytes.len() <= 100 {
        (&b""[..], bytes)
    } else {
        let split = path[..path.len().min(156)].rfind('/').filter(|&i| path.len() - i - 1 <= 100).ok_or_else(|| format!("path too long to upload: {path}"))?;
        (&bytes[..split], &bytes[split + 1..])
    };
    h[..name.len()].copy_from_slice(name);
    let octal = |field: &mut [u8], v: u64| {
        let s = format!("{:0width$o}\0", v, width = field.len() - 1);
        field.copy_from_slice(s.as_bytes());
    };
    octal(&mut h[100..108], 0o644);
    octal(&mut h[108..116], 0);
    octal(&mut h[116..124], 0);
    octal(&mut h[124..136], data.len() as u64);
    octal(&mut h[136..148], mtime);
    h[156] = b'0';
    h[257..263].copy_from_slice(b"ustar\0");
    h[263..265].copy_from_slice(b"00");
    h[345..345 + prefix.len()].copy_from_slice(prefix);
    h[148..156].copy_from_slice(b"        ");
    let sum: u32 = h.iter().map(|&b| b as u32).sum();
    let s = format!("{sum:06o}\0 ");
    h[148..156].copy_from_slice(s.as_bytes());
    out.extend_from_slice(&h);
    out.extend_from_slice(data);
    let pad = (512 - data.len() % 512) % 512;
    out.extend(std::iter::repeat_n(0u8, pad));
    Ok(())
}

// --- Running --------------------------------------------------------------------------------

/// A local path inside the project as the host's path; anything else unchanged.
pub fn to_remote(root: &Path, remote_root: &str, s: &str) -> String {
    let local = root.display().to_string();
    let rr = remote_root.trim_end_matches('/');
    let starts = s.len() >= local.len() && s.is_char_boundary(local.len()) && s[..local.len()].eq_ignore_ascii_case(&local);
    if !starts {
        return s.to_string();
    }
    let rest = &s[local.len()..];
    if !rest.is_empty() && !rest.starts_with(['\\', '/']) {
        return s.to_string();
    }
    format!("{rr}{}", rest.replace('\\', "/"))
}

/// A path the host printed (`/home/dev/brisk-projects/app/x.py`) as the project's file.
pub fn to_local(cmd_env: &HashMap<String, String>, path: &str) -> Option<PathBuf> {
    let (rr, local) = cmd_env.get(MAP_ENV)?.split_once('\t')?;
    let rest = path.strip_prefix(rr)?;
    if !rest.is_empty() && !rest.starts_with('/') {
        return None;
    }
    Some(PathBuf::from(local).join(rest.trim_start_matches('/').replace('/', std::path::MAIN_SEPARATOR_STR)))
}

/// The port a web server configuration listens on, for forwarding it to this machine.
pub fn server_port(kind: RunKind, args: &[String]) -> Option<u16> {
    let after = |flag: &str| args.iter().position(|a| a == flag).and_then(|i| args.get(i + 1)).and_then(|p| p.parse().ok());
    let eq = |flag: &str| args.iter().find_map(|a| a.strip_prefix(flag)).and_then(|p| p.parse().ok());
    match kind {
        RunKind::FastApi => after("--port").or_else(|| eq("--port=")).or(Some(8000)),
        RunKind::Flask => after("--port").or_else(|| after("-p")).or_else(|| eq("--port=")).or(Some(5000)),
        RunKind::Django => {
            let i = args.iter().position(|a| a == "runserver")?;
            let addr = args[i + 1..].iter().find(|a| !a.starts_with('-'));
            addr.and_then(|a| a.rsplit(':').next()).and_then(|p| p.parse().ok()).or(Some(8000))
        }
        _ => None,
    }
}

/// Turns a local run command into one that runs on the host: `ssh -tt host 'cd … && exec env
/// … python …'`. Paths inside the project become the host's; the configuration's variables
/// go along (not the local PATH or venv); a web server's port is forwarded to localhost.
/// Stopping the local `ssh` hangs up the remote process.
pub fn wrap(root: &Path, remote: &SshInterpreter, cfg: &RunConfig, cmd: &Command) -> Command {
    let map = |s: &str| to_remote(root, &remote.remote_root, s);
    let cwd = map(&cmd.cwd.display().to_string());
    let mut env: Vec<(String, String)> = cmd
        .env
        .iter()
        .filter(|(k, _)| !matches!(k.as_str(), "PATH" | "VIRTUAL_ENV" | "PYTHONPATH"))
        .map(|(k, v)| (k.clone(), map(v)))
        .collect();
    env.push(("PYTHONPATH".into(), remote.remote_root.clone()));
    if let Some(bin) = remote.python.rsplit_once('/').map(|(d, _)| d).filter(|d| d.ends_with("/bin")) {
        let venv = bin.trim_end_matches("/bin");
        if remote.python.contains("venv") || remote.python.contains("conda") {
            env.push(("VIRTUAL_ENV".into(), venv.to_string()));
        }
    }
    env.sort();
    let args: Vec<String> = cmd.args.iter().map(|a| map(a)).collect();
    let mut script = format!("cd {} && exec env", quote(&cwd));
    for (k, v) in &env {
        script.push_str(&format!(" {}", quote(&format!("{k}={v}"))));
    }
    if remote.python.contains("/bin/") && (remote.python.contains("venv") || remote.python.contains("conda")) {
        let bin = remote.python.rsplit_once('/').map(|(d, _)| d).unwrap_or("");
        script = script.replacen("exec env", &format!("export PATH={}:\"$PATH\" && exec env", quote(bin)), 1);
    }
    script.push(' ');
    script.push_str(&quote(&remote.python));
    for a in &args {
        script.push(' ');
        script.push_str(&quote(a));
    }
    let mut ssh_args = vec!["-tt".to_string(), "-q".into()];
    if let Some(port) = server_port(cfg.kind, &args) {
        ssh_args.extend(["-o".into(), "ExitOnForwardFailure=no".into(), "-L".into(), format!("{port}:127.0.0.1:{port}")]);
    }
    ssh_args.extend(connect_args(&remote.host));
    ssh_args.push(script);
    let mut local_env = HashMap::new();
    local_env.insert(HOST_ENV.to_string(), remote.host.clone());
    local_env.insert(MAP_ENV.to_string(), format!("{}\t{}", remote.remote_root.trim_end_matches('/'), root.display()));
    Command { program: ssh_exe(), args: ssh_args, cwd: cmd.cwd.clone(), env: local_env }
}

/// Whether a command is a remote run made by [`wrap`].
pub fn is_remote(cmd: &Command) -> bool {
    cmd.env.contains_key(MAP_ENV)
}

/// What a remote run's command syncs before it starts: host, local root, remote root.
pub fn sync_target(cmd: &Command) -> Option<(String, PathBuf, String)> {
    let host = cmd.env.get(HOST_ENV)?.clone();
    let (rr, local) = cmd.env.get(MAP_ENV)?.split_once('\t')?;
    Some((host, PathBuf::from(local), rr.to_string()))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn uri_round_trip_and_label() {
        let s = SshInterpreter { host: "dev-server".into(), python: "/usr/bin/python3".into(), version: "3.12.3".into(), remote_root: "/home/dev/brisk-projects/app".into() };
        assert_eq!(s.uri(), "ssh://dev-server/usr/bin/python3");
        assert_eq!(parse_uri(&s.uri()), Some(("dev-server".into(), "/usr/bin/python3".into())));
        assert_eq!(s.label(), "Remote Python 3.12 (dev-server)");
        assert_eq!(parse_uri("C:\\py\\python.exe"), None);
    }

    #[test]
    fn host_with_port() {
        assert!(connect_args("dev@10.0.0.5:2222").ends_with(&["-p".to_string(), "2222".into(), "dev@10.0.0.5".into()]));
        assert_eq!(connect_args("dev-server").last().unwrap(), "dev-server");
    }

    #[test]
    fn quoting() {
        assert_eq!(quote("/usr/bin/python3"), "/usr/bin/python3");
        assert_eq!(quote("a b"), "'a b'");
        assert_eq!(quote("it's"), r"'it'\''s'");
        assert_eq!(quote(""), "''");
    }

    #[test]
    fn probe_output() {
        let info = parse_probe("HOME /home/dev\nPY /usr/bin/python3 3.12.3\nPY /home/dev/.venv/bin/python 3.12.3\nPY /usr/bin/python3 3.12.3\n");
        assert_eq!(info.home, "/home/dev");
        assert_eq!(info.pythons.len(), 2);
        assert_eq!(default_remote_root("/home/dev/", Path::new(r"C:\work\My App")), "/home/dev/brisk-projects/My_App");
    }

    #[test]
    fn paths_both_ways() {
        let root = Path::new(r"C:\work\app");
        assert_eq!(to_remote(root, "/srv/app", r"C:\work\app\pkg\main.py"), "/srv/app/pkg/main.py");
        assert_eq!(to_remote(root, "/srv/app", r"c:\WORK\app"), "/srv/app");
        assert_eq!(to_remote(root, "/srv/app", r"C:\work\application\x.py"), r"C:\work\application\x.py");
        assert_eq!(to_remote(root, "/srv/app", "-m"), "-m");
        let mut env = HashMap::new();
        env.insert(MAP_ENV.to_string(), "/srv/app\tC:\\work\\app".to_string());
        assert_eq!(to_local(&env, "/srv/app/pkg/main.py"), Some(PathBuf::from(r"C:\work\app").join("pkg").join("main.py")));
        assert_eq!(to_local(&env, "/srv/application/x.py"), None);
        assert_eq!(to_local(&env, "/usr/lib/python3.12/x.py"), None);
    }

    #[test]
    fn wrapped_command() {
        let root = Path::new(r"C:\work\app");
        let remote = SshInterpreter { host: "dev-server".into(), python: "/home/dev/.venv/bin/python".into(), version: String::new(), remote_root: "/home/dev/brisk-projects/app".into() };
        let cfg = RunConfig { kind: RunKind::FastApi, ..Default::default() };
        let mut env = HashMap::new();
        env.insert("PYTHONUNBUFFERED".to_string(), "1".to_string());
        env.insert("PATH".to_string(), r"C:\Windows".to_string());
        env.insert("PYTHONPATH".to_string(), r"C:\work\app".to_string());
        env.insert("DATA".to_string(), r"C:\work\app\data".to_string());
        let cmd = Command { program: "/home/dev/.venv/bin/python".into(), args: vec!["-m".into(), "uvicorn".into(), "main:app".into(), "--port".into(), "8001".into()], cwd: root.to_path_buf(), env };
        let w = wrap(root, &remote, &cfg, &cmd);
        assert!(is_remote(&w));
        let script = w.args.last().unwrap();
        assert!(script.starts_with("cd /home/dev/brisk-projects/app && export PATH=/home/dev/.venv/bin:\"$PATH\" && exec env"), "{script}");
        assert!(script.contains("DATA=/home/dev/brisk-projects/app/data") && script.contains("PYTHONPATH=/home/dev/brisk-projects/app") && !script.contains("Windows"), "{script}");
        assert!(script.ends_with("/home/dev/.venv/bin/python -m uvicorn main:app --port 8001"), "{script}");
        assert!(w.args.windows(2).any(|a| a[0] == "-L" && a[1] == "8001:127.0.0.1:8001"));
        assert_eq!(server_port(RunKind::Django, &["manage.py".into(), "runserver".into(), "0.0.0.0:8005".into()]), Some(8005));
        assert_eq!(server_port(RunKind::Python, &[]), None);
    }

    /// Against a real host: `BRISK_SSH_TEST_HOST=dev-server cargo test -p brisk-run ssh -- --ignored`.
    /// Uploads a small project to `~/brisk-projects/brisk-ssh-test-<pid>`, changes and deletes
    /// a file, syncs again, runs the script there, and removes the folder.
    #[test]
    #[ignore]
    fn sync_and_run_on_a_host() {
        let Ok(host) = std::env::var("BRISK_SSH_TEST_HOST") else { return };
        let info = probe(&host).unwrap();
        let (python, _) = info.pythons.first().cloned().expect("a Python on the host");
        let root = std::env::temp_dir().join(format!("brisk-ssh-test-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&root);
        std::fs::create_dir_all(root.join("pkg")).unwrap();
        std::fs::write(root.join("main.py"), "import pkg.util
print('sum', pkg.util.add(2, 3))
").unwrap();
        std::fs::write(root.join("pkg/__init__.py"), "").unwrap();
        std::fs::write(root.join("pkg/util.py"), "def add(a, b):
    return a + b
").unwrap();
        std::fs::write(root.join("old.txt"), "x").unwrap();
        let rr = default_remote_root(&info.home, &root);
        let first = sync(&host, &root, &rr).unwrap();
        assert_eq!((first.uploaded, first.deleted), (4, 0));
        std::thread::sleep(std::time::Duration::from_millis(20));
        std::fs::write(root.join("pkg/util.py"), "def add(a, b):
    return a + b + 100
").unwrap();
        std::fs::remove_file(root.join("old.txt")).unwrap();
        let second = sync(&host, &root, &rr).unwrap();
        assert_eq!((second.uploaded, second.deleted), (1, 1));
        let out = run_script(&host, &format!("cd {} && PYTHONPATH=. {} main.py && ls", quote(&rr), quote(&python)), None).unwrap();
        assert!(out.contains("sum 105") && !out.contains("old.txt"), "{out}");
        run_script(&host, &format!("rm -rf {}", quote(&rr)), None).unwrap();
        let _ = std::fs::remove_dir_all(&root);
    }

    #[test]
    fn tar_entries_are_valid_ustar() {
        let mut t = Vec::new();
        tar_entry(&mut t, "pkg/a.py", b"print(1)\n", 0).unwrap();
        assert_eq!(t.len(), 1024);
        assert_eq!(&t[257..262], b"ustar");
        let long = format!("{}/{}", "d".repeat(120), "f.py");
        tar_entry(&mut t, &long, b"", 0).unwrap();
        assert_eq!(&t[1024 + 345..1024 + 345 + 120], "d".repeat(120).as_bytes());
    }
}
