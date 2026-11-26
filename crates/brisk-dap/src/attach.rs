//! The Python processes Attach to Process lists.

use brisk_platform::process::command;

/// `pid<TAB>command line` for every process (Windows: CIM; elsewhere: `ps`).
pub fn process_lines() -> String {
    let mut c = if cfg!(windows) {
        let mut c = command("powershell");
        c.args([
            "-NoProfile",
            "-NonInteractive",
            "-Command",
            "Get-CimInstance Win32_Process -Filter \"Name like 'python%'\" | ForEach-Object { \"$($_.ProcessId)`t$($_.CommandLine)\" }",
        ]);
        c
    } else {
        let mut c = command("ps");
        c.args(["-eo", "pid=,args="]);
        c
    };
    c.stdin(std::process::Stdio::null()).stderr(std::process::Stdio::null());
    c.output().map(|o| String::from_utf8_lossy(&o.stdout).into_owned()).unwrap_or_default()
}

/// What a Python command line runs, short: `app.py` or `-m http.server` (the list's label).
pub fn process_target(cmd: &str) -> String {
    let rest = match cmd.strip_prefix('"') {
        Some(q) => q.split_once('"').map_or("", |x| x.1),
        None => cmd.split_once(char::is_whitespace).map_or("", |x| x.1),
    };
    let mut it = rest.split_whitespace();
    while let Some(t) = it.next() {
        if t == "-m" {
            return format!("-m {}", it.next().unwrap_or(""));
        }
        if t == "-c" {
            return "-c …".into();
        }
        if !t.starts_with('-') {
            let t = t.trim_matches('"');
            return t.rsplit(['/', '\\']).next().unwrap_or(t).to_string();
        }
    }
    String::new()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn process_targets() {
        assert_eq!(process_target(r#""C:\Py 3\python.exe" -u C:/x/app.py --port 1"#), "app.py");
        assert_eq!(process_target("python -m http.server 8765"), "-m http.server");
        assert_eq!(process_target("python"), "");
    }
}
