//! Profile (cProfile), like PyCharm's: the run goes through `python -m cProfile -o F`, then
//! a small pstats dump gives a row per function (calls, own time, total time).

use crate::{split_args, Command, RunConfig};
use std::path::{Path, PathBuf};

#[derive(Clone, Debug, PartialEq)]
pub struct Row {
    /// `~` for built-ins (not openable).
    pub file: String,
    /// 1-based; 0 for built-ins.
    pub line: usize,
    pub func: String,
    pub calls: u64,
    /// Primitive (non-recursive) calls: shown as `calls/primitive` when they differ.
    pub primitive: u64,
    /// Seconds.
    pub own: f64,
    pub total: f64,
}

impl Row {
    /// `func (file:line)` as PyCharm's Name column shows it.
    pub fn name(&self, root: &Path) -> String {
        if self.file == "~" {
            return self.func.clone();
        }
        let p = Path::new(&self.file);
        let shown = p.strip_prefix(root).unwrap_or(p).to_string_lossy().replace('\\', "/");
        format!("{} ({shown}:{})", self.func, self.line)
    }

    pub fn source(&self) -> Option<(PathBuf, usize)> {
        let p = PathBuf::from(&self.file);
        (self.file != "~" && p.is_file()).then_some((p, self.line))
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum SortBy {
    Name,
    Calls,
    Own,
    Total,
}

/// Sort rows; numbers descend first, names ascend first (`desc` flips either).
pub fn sort(rows: &mut [Row], by: SortBy, desc: bool, root: &Path) {
    rows.sort_by(|a, b| {
        let o = match by {
            SortBy::Name => a.name(root).to_lowercase().cmp(&b.name(root).to_lowercase()),
            SortBy::Calls => a.calls.cmp(&b.calls),
            SortBy::Own => a.own.total_cmp(&b.own),
            SortBy::Total => a.total.total_cmp(&b.total),
        };
        if desc {
            o.reverse()
        } else {
            o
        }
    });
}

pub fn stats_file(root: &Path) -> PathBuf {
    root.join(".brisk").join("profile.pstats")
}

/// `python [opts] -m cProfile -o F <the rest>` (scripts, `-m module` and pytest).
pub fn wrap(cmd: &mut Command, cfg: &RunConfig, stats: &Path) {
    let at = split_args(&cfg.interpreter_options).len().min(cmd.args.len());
    let extra = ["-m".to_string(), "cProfile".into(), "-o".into(), stats.display().to_string()];
    cmd.args.splice(at..at, extra);
}

/// The pstats dump: `python -c DUMP <stats> <out.json>`.
pub const DUMP: &str = "import json, pstats, sys\n\
s = pstats.Stats(sys.argv[1])\n\
rows = [[f, l, n, nc, cc, tt, ct] for (f, l, n), (cc, nc, tt, ct, _) in s.stats.items()]\n\
json.dump({'total': s.total_tt, 'rows': rows}, open(sys.argv[2], 'w', encoding='utf-8'))\n";

/// Rows and the total time of a dump.
pub fn parse(json: &str) -> Result<(Vec<Row>, f64), String> {
    let v: serde_json::Value = serde_json::from_str(json).map_err(|e| format!("Bad profile dump: {e}"))?;
    let rows = v["rows"]
        .as_array()
        .map(|a| {
            a.iter()
                .filter_map(|r| {
                    let r = r.as_array()?;
                    Some(Row {
                        file: r.first()?.as_str()?.to_string(),
                        line: r.get(1)?.as_u64()? as usize,
                        func: r.get(2)?.as_str()?.to_string(),
                        calls: r.get(3)?.as_u64()?,
                        primitive: r.get(4)?.as_u64()?,
                        own: r.get(5)?.as_f64()?,
                        total: r.get(6)?.as_f64()?,
                    })
                })
                .collect()
        })
        .unwrap_or_default();
    Ok((rows, v["total"].as_f64().unwrap_or(0.0)))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parse_and_sort() {
        let json = r#"{"total": 0.5, "rows": [
            ["/p/app.py", 3, "slow", 1, 1, 0.4, 0.45],
            ["~", 0, "<built-in method builtins.len>", 1000, 1000, 0.01, 0.01],
            ["/p/app.py", 10, "fib", 177, 1, 0.04, 0.04]
        ]}"#;
        let (mut rows, total) = parse(json).unwrap();
        assert_eq!((rows.len(), total), (3, 0.5));
        let root = Path::new("/p");
        sort(&mut rows, SortBy::Own, true, root);
        assert_eq!(rows[0].func, "slow");
        assert_eq!(rows[0].name(root), "slow (app.py:3)");
        sort(&mut rows, SortBy::Calls, true, root);
        assert_eq!(rows[0].calls, 1000);
        assert_eq!(rows[0].name(root), "<built-in method builtins.len>");
        sort(&mut rows, SortBy::Name, false, root);
        assert_eq!(rows[0].func, "<built-in method builtins.len>");
        assert!(rows[0].source().is_none());
    }
}
