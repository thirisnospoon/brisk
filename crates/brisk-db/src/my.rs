//! MySQL / MariaDB through the `mysql` crate (pure Rust protocol, native TLS).

use crate::{Canceller, Connection, DataSource, Driver, QueryOutput, Ssl};
use mysql::prelude::Queryable;
use mysql::{Conn, Opts, OptsBuilder, SslOpts, Value};
use std::sync::Arc;
use std::time::Duration;

pub const INTROSPECT: &str = "\
select c.table_schema, c.table_name, t.table_type, c.column_name, c.column_type, c.is_nullable,
       c.column_key = 'PRI'
from information_schema.columns c
join information_schema.tables t on t.table_schema = c.table_schema and t.table_name = c.table_name
where c.table_schema not in ('mysql', 'information_schema', 'performance_schema', 'sys')
order by c.table_schema, c.table_name, c.ordinal_position";

pub const SCHEMAS: &str = "\
select schema_name from information_schema.schemata
where schema_name not in ('mysql', 'information_schema', 'performance_schema', 'sys')";

pub struct My {
    conn: Conn,
    opts: Opts,
}

fn opts(ds: &DataSource, password: &str, ssl: bool) -> Opts {
    let mut b = OptsBuilder::new()
        .ip_or_hostname(Some(if ds.host.is_empty() { "localhost".to_string() } else { ds.host.clone() }))
        .tcp_port(if ds.port == 0 { 3306 } else { ds.port })
        .user(Some(ds.user.clone()))
        .pass((!password.is_empty()).then(|| password.to_string()))
        .db_name((!ds.database.is_empty()).then(|| ds.database.clone()))
        .tcp_connect_timeout(Some(Duration::from_secs(10)))
        .prefer_socket(false);
    if ssl {
        b = b.ssl_opts(Some(SslOpts::default().with_danger_accept_invalid_certs(true).with_danger_skip_domain_validation(true)));
    }
    b.into()
}

pub fn connect(ds: &DataSource, password: &str) -> Result<My, String> {
    let try_ssl = ds.ssl != Ssl::Disable;
    let first = opts(ds, password, try_ssl);
    match Conn::new(first.clone()) {
        Ok(conn) => Ok(My { conn, opts: first }),
        Err(e) if ds.ssl == Ssl::Prefer => {
            // Server without TLS: fall back to plain, like `prefer` in libpq.
            let plain = opts(ds, password, false);
            match Conn::new(plain.clone()) {
                Ok(conn) => Ok(My { conn, opts: plain }),
                Err(_) => Err(fmt_err(e)),
            }
        }
        Err(e) => Err(fmt_err(e)),
    }
}

fn fmt_err(e: mysql::Error) -> String {
    match e {
        mysql::Error::MySqlError(m) => format!("[{}] {}", m.state, m.message),
        e => e.to_string(),
    }
}

fn cell(v: Value) -> Option<String> {
    Some(match v {
        Value::NULL => return None,
        Value::Bytes(b) => match String::from_utf8(b) {
            Ok(s) => s,
            Err(e) => {
                let b = e.into_bytes();
                let mut s = String::from("0x");
                for x in b.iter().take(64) {
                    s.push_str(&format!("{x:02X}"));
                }
                s
            }
        },
        Value::Int(i) => i.to_string(),
        Value::UInt(u) => u.to_string(),
        Value::Float(f) => f.to_string(),
        Value::Double(f) => f.to_string(),
        Value::Date(y, mo, d, h, mi, s, us) => {
            if h == 0 && mi == 0 && s == 0 && us == 0 {
                format!("{y:04}-{mo:02}-{d:02}")
            } else if us == 0 {
                format!("{y:04}-{mo:02}-{d:02} {h:02}:{mi:02}:{s:02}")
            } else {
                format!("{y:04}-{mo:02}-{d:02} {h:02}:{mi:02}:{s:02}.{us:06}")
            }
        }
        Value::Time(neg, d, h, mi, s, us) => {
            let sign = if neg { "-" } else { "" };
            let h = d * 24 + h as u32;
            if us == 0 { format!("{sign}{h:02}:{mi:02}:{s:02}") } else { format!("{sign}{h:02}:{mi:02}:{s:02}.{us:06}") }
        }
    })
}

/// A plain read without a `limit` of its own, with `limit + 1` added (one more tells that
/// there are more). Not wrapped in a derived table as for PostgreSQL: MySQL may drop the
/// ORDER BY of a derived table and rejects duplicate column names in one. The limit goes on
/// its own line so a trailing `--` comment does not swallow it.
fn capped_read(sql: &str, limit: usize) -> Option<String> {
    if limit == usize::MAX || !crate::pg::wrappable(sql, Driver::Mysql) {
        return None;
    }
    let code = crate::split::code_only(sql, Driver::Mysql).to_ascii_lowercase();
    if code.split(|c: char| !(c.is_alphanumeric() || c == '_')).any(|w| w == "limit") {
        return None;
    }
    Some(format!("{sql}\nlimit {}", limit + 1))
}

impl Connection for My {
    fn driver(&self) -> Driver {
        Driver::Mysql
    }

    fn run(&mut self, sql: &str, limit: usize) -> Result<QueryOutput, String> {
        let mut out = QueryOutput::default();
        // The rows past the limit would still be read (and dropped) to finish the result:
        // cap plain reads on the server side.
        let capped;
        let q = match capped_read(sql, limit) {
            Some(c) => {
                capped = c;
                capped.as_str()
            }
            None => sql,
        };
        let mut res = self.conn.query_iter(q).map_err(fmt_err)?;
        // A statement may return several sets (CALL); show the first one that has columns.
        while let Some(set) = res.iter() {
            let cols: Vec<String> = set.columns().as_ref().iter().map(|c| c.name_str().into_owned()).collect();
            let affected = set.affected_rows();
            if cols.is_empty() || !out.columns.is_empty() {
                if out.columns.is_empty() {
                    out.affected = Some(affected);
                }
                for _ in set {}
                continue;
            }
            out.columns = cols;
            for row in set {
                let row = row.map_err(fmt_err)?;
                if out.rows.len() >= limit {
                    out.truncated = true;
                    // Remaining rows are drained when the result is dropped.
                    break;
                }
                out.rows.push(row.unwrap().into_iter().map(cell).collect());
            }
        }
        Ok(out)
    }

    fn canceller(&self) -> Canceller {
        let id = self.conn.connection_id();
        let opts = self.opts.clone();
        Canceller(Arc::new(move || {
            if let Ok(mut c) = Conn::new(opts.clone()) {
                let _ = c.query_drop(format!("KILL QUERY {id}"));
            }
        }))
    }

    fn server_version(&mut self) -> String {
        let (a, b, c) = self.conn.server_version();
        let maria = self.conn.query_first::<String, _>("select version()").ok().flatten().is_some_and(|v| v.to_lowercase().contains("mariadb"));
        format!("{} {a}.{b}.{c}", if maria { "MariaDB" } else { "MySQL" })
    }
}

#[cfg(test)]
mod t {
    #[test]
    fn caps_plain_reads() {
        let c = |s: &str| super::capped_read(s, 10);
        assert_eq!(c("select * from t order by id -- newest").as_deref(), Some("select * from t order by id -- newest\nlimit 11"));
        assert_eq!(c("select * from t limit 5"), None, "its own limit");
        assert_eq!(c("select * from t for update"), None);
        assert_eq!(c("select * from t lock in share mode"), None);
        assert_eq!(c("select 1 into @x"), None);
        assert_eq!(c("update t set a = 1"), None);
        // Words in strings do not count.
        assert!(c("select * from t where note = 'no limit, into the wild'").is_some());
        assert_eq!(super::capped_read("select 1", usize::MAX), None);
    }
}
