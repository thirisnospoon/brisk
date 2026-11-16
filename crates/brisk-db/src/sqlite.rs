//! SQLite through rusqlite with the bundled library (no system dependency).

use crate::{Canceller, Connection, Driver, QueryOutput};
use rusqlite::types::ValueRef;
use std::path::Path;
use std::sync::Arc;

pub struct Sqlite {
    conn: rusqlite::Connection,
}

pub fn connect(path: &Path) -> Result<Sqlite, String> {
    if path.as_os_str().is_empty() {
        return Err("No database file specified".into());
    }
    if let Some(dir) = path.parent() {
        if !dir.as_os_str().is_empty() && !dir.is_dir() {
            return Err(format!("Directory does not exist: {}", dir.display()));
        }
    }
    let conn = rusqlite::Connection::open(path).map_err(|e| e.to_string())?;
    conn.busy_timeout(std::time::Duration::from_secs(5)).map_err(|e| e.to_string())?;
    Ok(Sqlite { conn })
}

pub(crate) fn cell(v: ValueRef<'_>) -> Option<String> {
    match v {
        ValueRef::Null => None,
        ValueRef::Integer(i) => Some(i.to_string()),
        ValueRef::Real(f) => Some(f.to_string()),
        ValueRef::Text(t) => Some(String::from_utf8_lossy(t).into_owned()),
        ValueRef::Blob(b) => {
            let mut s = String::with_capacity(2 + b.len().min(64) * 2);
            s.push_str("0x");
            for x in b.iter().take(64) {
                s.push_str(&format!("{x:02X}"));
            }
            if b.len() > 64 {
                s.push('…');
            }
            Some(s)
        }
    }
}

impl Connection for Sqlite {
    fn driver(&self) -> Driver {
        Driver::Sqlite
    }

    fn run(&mut self, sql: &str, limit: usize) -> Result<QueryOutput, String> {
        let mut st = self.conn.prepare(sql).map_err(|e| e.to_string())?;
        let mut out = QueryOutput::default();
        if st.column_count() == 0 {
            let n = st.execute([]).map_err(|e| e.to_string())?;
            out.affected = Some(n as u64);
            return Ok(out);
        }
        out.columns = st.column_names().into_iter().map(str::to_string).collect();
        let ncol = out.columns.len();
        let mut rows = st.query([]).map_err(|e| e.to_string())?;
        while let Some(r) = rows.next().map_err(|e| e.to_string())? {
            if out.rows.len() >= limit {
                out.truncated = true;
                break;
            }
            out.rows.push((0..ncol).map(|i| r.get_ref(i).map(cell).unwrap_or(None)).collect());
        }
        Ok(out)
    }

    fn canceller(&self) -> Canceller {
        let h = self.conn.get_interrupt_handle();
        Canceller(Arc::new(move || h.interrupt()))
    }

    fn server_version(&mut self) -> String {
        format!("SQLite {}", rusqlite::version())
    }
}
