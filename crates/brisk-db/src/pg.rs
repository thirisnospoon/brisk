//! PostgreSQL through the `postgres` crate. Statements go over the simple-query protocol, so
//! every value arrives as text and any statement (DDL, DO blocks, COPY-less scripts) works.

use crate::{Canceller, Connection, DataSource, Driver, QueryOutput, Ssl};
use postgres::{Client, SimpleQueryMessage};
use postgres_native_tls::MakeTlsConnector;
use std::sync::Arc;
use std::time::Duration;

pub const INTROSPECT: &str = "\
select c.table_schema, c.table_name, t.table_type, c.column_name,
       case when c.data_type = 'USER-DEFINED' then c.udt_name
            when c.data_type = 'ARRAY' then substr(c.udt_name, 2) || '[]'
            when c.character_maximum_length is not null then c.data_type || '(' || c.character_maximum_length || ')'
            else c.data_type end,
       c.is_nullable,
       exists(select 1 from information_schema.table_constraints tc
              join information_schema.key_column_usage k
                on k.constraint_name = tc.constraint_name and k.table_schema = tc.table_schema and k.table_name = tc.table_name
              where tc.constraint_type = 'PRIMARY KEY' and k.table_schema = c.table_schema
                and k.table_name = c.table_name and k.column_name = c.column_name)
from information_schema.columns c
join information_schema.tables t on t.table_schema = c.table_schema and t.table_name = c.table_name
where c.table_schema not in ('pg_catalog', 'information_schema') and c.table_schema not like 'pg_toast%'
  and c.table_schema not like 'pg_temp%'
order by 1, 2, c.ordinal_position";

pub const SCHEMAS: &str = "\
select schema_name from information_schema.schemata
where schema_name not in ('pg_catalog', 'information_schema') and schema_name not like 'pg_toast%'
  and schema_name not like 'pg_temp%'";

pub struct Pg {
    client: Client,
    tls: MakeTlsConnector,
}

fn tls() -> Result<MakeTlsConnector, String> {
    // libpq "require"/"prefer" semantics: encrypt, but don't verify the certificate.
    let c = native_tls::TlsConnector::builder().danger_accept_invalid_certs(true).danger_accept_invalid_hostnames(true).build().map_err(|e| e.to_string())?;
    Ok(MakeTlsConnector::new(c))
}

pub fn connect(ds: &DataSource, password: &str) -> Result<Pg, String> {
    let mut cfg = postgres::Config::new();
    cfg.host(if ds.host.is_empty() { "localhost" } else { &ds.host })
        .port(if ds.port == 0 { 5432 } else { ds.port })
        .user(&ds.user)
        .application_name("Brisk")
        .connect_timeout(Duration::from_secs(10));
    if !password.is_empty() {
        cfg.password(password);
    }
    if !ds.database.is_empty() {
        cfg.dbname(&ds.database);
    }
    cfg.ssl_mode(match ds.ssl {
        Ssl::Disable => postgres::config::SslMode::Disable,
        Ssl::Prefer => postgres::config::SslMode::Prefer,
        Ssl::Require => postgres::config::SslMode::Require,
    });
    let tls = tls()?;
    let client = cfg.connect(tls.clone()).map_err(fmt_err)?;
    Ok(Pg { client, tls })
}

fn fmt_err(e: postgres::Error) -> String {
    fmt_err_at(e, 0)
}

/// `fmt_err` for a statement sent with `shift` characters of wrapper before it: the error
/// position points into the user's text.
fn fmt_err_at(e: postgres::Error, shift: u32) -> String {
    if let Some(db) = e.as_db_error() {
        let mut s = format!("[{}] {}: {}", db.code().code(), db.severity(), db.message());
        if let Some(d) = db.detail() {
            s.push_str(&format!("\nDetail: {d}"));
        }
        if let Some(h) = db.hint() {
            s.push_str(&format!("\nHint: {h}"));
        }
        if let Some(p) = db.position() {
            if let postgres::error::ErrorPosition::Original(p) = p {
                s.push_str(&format!("\nPosition: {}", p.saturating_sub(shift).max(1)));
            }
        }
        return s;
    }
    // Include the source chain: "error connecting to server: Connection refused".
    let mut s = e.to_string();
    let mut src = std::error::Error::source(&e);
    while let Some(x) = src {
        let m = x.to_string();
        if !s.contains(&m) {
            s.push_str(": ");
            s.push_str(&m);
        }
        src = x.source();
    }
    s
}

/// The text before the user's statement in [`wrap`]. ASCII, so its length in bytes is its
/// length in characters (what the server's error position counts).
const WRAP_HEAD: &str = "select * from (\n";

/// Can this statement be wrapped as `select * from (...) limit n`? Only plain reads; anything
/// with locking clauses (`for update`, `for share`, ...), `select ... into` or data-modifying
/// CTEs runs as written. Words in strings, quoted names and comments do not count.
pub(crate) fn wrappable(sql: &str, driver: crate::Driver) -> bool {
    let k = crate::first_keyword(sql);
    if !matches!(k.as_str(), "select" | "with" | "table" | "values") {
        return false;
    }
    let l = crate::split::code_only(sql, driver).to_ascii_lowercase();
    let words: Vec<&str> = l.split(|c: char| !(c.is_alphanumeric() || c == '_')).filter(|w| !w.is_empty()).collect();
    let bad = ["into", "insert", "update", "delete", "merge"];
    // PostgreSQL / MySQL 8 `for update|share|no key update|key share`, MySQL `lock in share mode`.
    let locking = words.windows(2).any(|w| (w[0] == "for" && matches!(w[1], "update" | "share" | "no" | "key")) || (w[0] == "lock" && w[1] == "in"));
    !locking && !words.iter().any(|w| bad.contains(w))
}

/// `sql` capped at `limit + 1` rows (one more tells that there are more).
pub(crate) fn wrap(sql: &str, limit: usize) -> String {
    format!("{WRAP_HEAD}{sql}\n) brisk_q limit {}", limit + 1)
}

impl Connection for Pg {
    fn driver(&self) -> Driver {
        Driver::Postgres
    }

    fn run(&mut self, sql: &str, limit: usize) -> Result<QueryOutput, String> {
        // The simple protocol buffers the whole result; cap big reads on the server side.
        let wrapped;
        let (q, shift) = if limit < usize::MAX && wrappable(sql, Driver::Postgres) {
            wrapped = wrap(sql, limit);
            (wrapped.as_str(), WRAP_HEAD.len() as u32)
        } else {
            (sql, 0)
        };
        let msgs = self.client.simple_query(q).map_err(|e| fmt_err_at(e, shift))?;
        let mut out = QueryOutput::default();
        for m in msgs {
            match m {
                SimpleQueryMessage::RowDescription(cols) => {
                    out.columns = cols.iter().map(|c| c.name().to_string()).collect();
                }
                SimpleQueryMessage::Row(r) => {
                    if out.columns.is_empty() {
                        out.columns = r.columns().iter().map(|c| c.name().to_string()).collect();
                    }
                    if out.rows.len() >= limit {
                        out.truncated = true;
                        continue;
                    }
                    out.rows.push((0..r.len()).map(|i| r.get(i).map(str::to_string)).collect());
                }
                SimpleQueryMessage::CommandComplete(n) => {
                    if out.columns.is_empty() {
                        out.affected = Some(n);
                    }
                }
                _ => {}
            }
        }
        Ok(out)
    }

    fn canceller(&self) -> Canceller {
        let token = self.client.cancel_token();
        let tls = self.tls.clone();
        Canceller(Arc::new(move || {
            let _ = token.cancel_query(tls.clone());
        }))
    }

    fn server_version(&mut self) -> String {
        match self.client.simple_query("show server_version") {
            Ok(m) => m
                .into_iter()
                .find_map(|m| match m {
                    SimpleQueryMessage::Row(r) => r.get(0).map(|v| format!("PostgreSQL {v}")),
                    _ => None,
                })
                .unwrap_or_else(|| "PostgreSQL".into()),
            Err(e) => fmt_err(e),
        }
    }
}

#[cfg(test)]
mod t {
    #[test]
    fn wrap_rules() {
        let w = |s: &str| super::wrappable(s, crate::Driver::Postgres);
        assert!(w("select * from t"));
        assert!(w("  -- c\nWITH x as (select 1) select * from x"));
        assert!(!w("select * from t for update"));
        assert!(!w("select * from t for no key update skip locked"));
        assert!(!w("select * from t FOR SHARE nowait"));
        assert!(!w("with d as (delete from t returning *) select * from d"));
        assert!(!w("select 1 into tmp"));
        assert!(!w("insert into t values (1)"));
        // The words in strings, quoted names and comments, or as part of a name, are data.
        assert!(w("select * from log where action = 'insert into x' -- for update"));
        assert!(w("select \"update\", locked_at, share from t /* delete */"));
        assert!(w("select * from t where note = E'it\\'s insert'"));
        assert!(w("select * from t where path = 'C:\\' and kind = 'locked'"));
    }

    #[test]
    fn wrapper_head_length() {
        let q = super::wrap("select 1", 10);
        assert!(q.starts_with(super::WRAP_HEAD) && q[super::WRAP_HEAD.len()..].starts_with("select 1"));
    }
}
