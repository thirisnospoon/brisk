//! Shells: the ones the Terminal offers, and how a command line is run through one.

/// A shell the Terminal can start.
#[derive(Clone, Debug)]
pub struct Shell {
    pub label: &'static str,
    pub program: String,
    pub args: Vec<String>,
}

/// The shells on this machine, the default first. On Windows that is Windows PowerShell,
/// then PowerShell 7, Command Prompt and Git Bash where installed. WSL is not offered:
/// starting it boots its virtual machine. Elsewhere it is the login shell (`$SHELL`).
pub fn shells() -> Vec<Shell> {
    if !cfg!(windows) {
        let program = std::env::var("SHELL").ok().filter(|s| !s.is_empty()).unwrap_or_else(|| "/bin/sh".into());
        return vec![Shell { label: "Shell", program, args: Vec::new() }];
    }
    let mut v = vec![Shell { label: "PowerShell", program: "powershell.exe".into(), args: vec!["-NoLogo".into()] }];
    let pf =
        |sub: &str| ["ProgramFiles", "ProgramW6432"].iter().filter_map(std::env::var_os).map(|d| std::path::PathBuf::from(d).join(sub)).find(|p| p.is_file());
    if let Some(p) = pf(r"PowerShell\7\pwsh.exe") {
        v.push(Shell { label: "PowerShell 7", program: p.display().to_string(), args: vec!["-NoLogo".into()] });
    }
    v.push(Shell { label: "Command Prompt", program: "cmd.exe".into(), args: Vec::new() });
    if let Some(p) = pf(r"Git\bin\bash.exe") {
        v.push(Shell { label: "Git Bash", program: p.display().to_string(), args: vec!["--login".into(), "-i".into()] });
    }
    v
}

/// The shell a new terminal starts: Windows PowerShell; `None` elsewhere, where the terminal
/// takes `$SHELL` or the passwd entry itself.
pub fn default_shell() -> Option<(String, Vec<String>)> {
    if cfg!(windows) {
        Some(("powershell.exe".into(), vec!["-NoLogo".into()]))
    } else {
        None
    }
}

/// The program and arguments that run `line` through the system shell: `%ComSpec% /C line`
/// (`cmd.exe` when unset) on Windows, `/bin/sh -c line` elsewhere.
pub fn command_line(line: &str) -> (String, Vec<String>) {
    if cfg!(windows) {
        (std::env::var("ComSpec").unwrap_or_else(|_| "cmd.exe".into()), vec!["/C".into(), line.into()])
    } else {
        ("/bin/sh".into(), vec!["-c".into(), line.into()])
    }
}

/// The separator of the folders in `PATH`: `;` on Windows, `:` elsewhere.
pub fn path_separator() -> &'static str {
    if cfg!(windows) {
        ";"
    } else {
        ":"
    }
}

#[cfg(test)]
mod tests {
    #[test]
    fn command_line_ends_with_the_line() {
        let (program, args) = super::command_line("echo hi");
        assert!(!program.is_empty());
        assert_eq!(args.last().map(String::as_str), Some("echo hi"));
    }

    #[test]
    fn the_default_shell_comes_first() {
        let shells = super::shells();
        if let Some((program, _)) = super::default_shell() {
            assert_eq!(shells[0].program, program);
        }
    }
}
