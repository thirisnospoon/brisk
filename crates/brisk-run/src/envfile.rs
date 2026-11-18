//! `.env` files of a run configuration (PyCharm's "Paths to .env files"): `KEY=VALUE` lines
//! mixed into the run's environment before the configuration's own variables, which win.
//!
//! Syntax as python-dotenv reads it: `#` comments, blank lines, `export KEY=…`, values in
//! single quotes (literal), double quotes (`\n`, `\t`, `\"` escapes) or bare (trailing
//! ` # comment` dropped); `${VAR}` in double-quoted and bare values expands from earlier
//! lines, then from the environment.

use crate::{resolve, RunConfig};
use std::collections::HashMap;
use std::path::{Path, PathBuf};

/// The configuration's `.env` paths, resolved against the project.
pub fn paths(root: &Path, cfg: &RunConfig) -> Vec<PathBuf> {
    cfg.env_files.split(';').map(str::trim).filter(|p| !p.is_empty()).map(|p| resolve(root, p)).collect()
}

fn expand(v: &str, known: &HashMap<String, String>) -> String {
    let mut out = String::new();
    let mut rest = v;
    while let Some(i) = rest.find("${") {
        let Some(end) = rest[i + 2..].find('}') else { break };
        out.push_str(&rest[..i]);
        let name = &rest[i + 2..i + 2 + end];
        let (name, default) = name.split_once(":-").unwrap_or((name, ""));
        let val = known.get(name).cloned().or_else(|| std::env::var(name).ok()).filter(|v| !v.is_empty()).unwrap_or_else(|| default.to_string());
        out.push_str(&val);
        rest = &rest[i + 3 + end..];
    }
    out.push_str(rest);
    out
}

/// The variables of a `.env` text, in order.
pub fn parse(text: &str) -> Vec<(String, String)> {
    let mut out: Vec<(String, String)> = Vec::new();
    let mut known: HashMap<String, String> = HashMap::new();
    let mut lines = text.lines();
    while let Some(raw) = lines.next() {
        let line = raw.trim_start_matches('\u{feff}').trim();
        if line.is_empty() || line.starts_with('#') {
            continue;
        }
        let line = line.strip_prefix("export ").map(str::trim_start).unwrap_or(line);
        let Some((k, v)) = line.split_once('=') else { continue };
        let key = k.trim();
        if key.is_empty() || key.contains(char::is_whitespace) {
            continue;
        }
        let v = v.trim_start();
        let value = if let Some(body) = v.strip_prefix('\'') {
            // Literal; may span lines.
            let mut s = body.to_string();
            while !s.contains('\'') {
                match lines.next() {
                    Some(l) => {
                        s.push('\n');
                        s.push_str(l);
                    }
                    None => break,
                }
            }
            s.split('\'').next().unwrap_or("").to_string()
        } else if let Some(body) = v.strip_prefix('"') {
            let mut s = body.to_string();
            let closed = |s: &str| {
                let mut esc = false;
                s.chars().any(|c| {
                    let end = c == '"' && !esc;
                    esc = c == '\\' && !esc;
                    end
                })
            };
            while !closed(&s) {
                match lines.next() {
                    Some(l) => {
                        s.push('\n');
                        s.push_str(l);
                    }
                    None => break,
                }
            }
            let mut val = String::new();
            let mut chars = s.chars();
            while let Some(c) = chars.next() {
                match c {
                    '"' => break,
                    '\\' => match chars.next() {
                        Some('n') => val.push('\n'),
                        Some('t') => val.push('\t'),
                        Some('r') => val.push('\r'),
                        Some(o) => val.push(o),
                        None => {}
                    },
                    c => val.push(c),
                }
            }
            expand(&val, &known)
        } else {
            let bare = v.split(" #").next().unwrap_or(v).trim_end();
            expand(bare, &known)
        };
        known.insert(key.to_string(), value.clone());
        out.retain(|(k, _)| k != key);
        out.push((key.to_string(), value));
    }
    out
}

/// All variables of the configuration's `.env` files (later files win). An error names a
/// file that cannot be read.
pub fn load(root: &Path, cfg: &RunConfig) -> Result<Vec<(String, String)>, String> {
    let mut out: Vec<(String, String)> = Vec::new();
    for p in paths(root, cfg) {
        let text = std::fs::read_to_string(&p).map_err(|e| format!("Cannot read the .env file {}: {e}", p.display()))?;
        for (k, v) in parse(&text) {
            out.retain(|(o, _)| *o != k);
            out.push((k, v));
        }
    }
    Ok(out)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn dotenv_syntax() {
        let t = "\u{feff}# comment\n\nA=1\nexport B = two words  # note\nC='lit ${A} # kept'\nD=\"x\\ny \\\"q\\\" ${A}\"\nE=${A}-${MISSING_BRISK_VAR:-def}\nM=\"line1\nline2\"\nA=override\nbad line\n=nokey\n";
        let v: HashMap<String, String> = parse(t).into_iter().collect();
        assert_eq!(v["A"], "override");
        assert_eq!(v["B"], "two words");
        assert_eq!(v["C"], "lit ${A} # kept");
        assert_eq!(v["D"], "x\ny \"q\" 1");
        assert_eq!(v["E"], "1-def");
        assert_eq!(v["M"], "line1\nline2");
        assert_eq!(v.len(), 6);
    }

    #[test]
    fn files_resolve_and_merge() {
        let dir = std::env::temp_dir().join(format!("brisk-envfile-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(dir.join("cfg")).unwrap();
        std::fs::write(dir.join(".env"), "A=1\nB=2\n").unwrap();
        std::fs::write(dir.join("cfg").join("dev.env"), "B=dev\n").unwrap();
        let cfg = RunConfig { env_files: ".env; $PROJECT_DIR$/cfg/dev.env".into(), ..Default::default() };
        assert_eq!(load(&dir, &cfg).unwrap(), vec![("A".to_string(), "1".to_string()), ("B".to_string(), "dev".to_string())]);
        let missing = RunConfig { env_files: "nope.env".into(), ..Default::default() };
        assert!(load(&dir, &missing).unwrap_err().contains("nope.env"));
        let _ = std::fs::remove_dir_all(&dir);
    }
}
