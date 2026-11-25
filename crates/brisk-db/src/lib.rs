//! Database access for the Database tool window: data sources, connections, queries and
//! schema introspection. UI-free; every call here blocks, so the app runs them off the UI thread.
//!
//! Drivers are native Rust/C libraries (no JDBC, no JVM): SQLite (bundled), PostgreSQL and
//! MySQL/MariaDB. Results come back as text, which is what a grid shows anyway.

mod config;
mod my;
mod pg;
mod secrets;
mod split;
mod sqlite;
#[cfg(test)]
mod tests;

pub use config::{config_dir, load as load_sources, save as save_sources, CONFIG_FILE};
pub use secrets::{forget_password, load_password, store_password};
pub use split::{split_statements, statement_at};

use serde::{Deserialize, Serialize};
use std::path::Path;
use std::sync::Arc;
use std::time::{Duration, Instant};

/// PyCharm's default page size for result sets.
pub const DEFAULT_LIMIT: usize = 500;

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum Driver {
    Postgres,
    Mysql,
    Sqlite,
}

impl Driver {
    pub const ALL: [Driver; 3] = [Driver::Postgres, Driver::Mysql, Driver::Sqlite];

    pub fn title(self) -> &'static str {
        match self {
            Driver::Postgres => "PostgreSQL",
            Driver::Mysql => "MySQL",
            Driver::Sqlite => "SQLite",
        }
    }
    pub fn default_port(self) -> u16 {
        match self {
            Driver::Postgres => 5432,
            Driver::Mysql => 3306,
            Driver::Sqlite => 0,
        }
    }
    fn default_user(self) -> &'static str {
        match self {
            Driver::Postgres => "postgres",
            Driver::Mysql => "root",
            Driver::Sqlite => "",
        }
    }
}

/// TLS mode, libpq semantics: `Prefer` tries TLS and falls back; `Require` insists on TLS.
/// Like libpq's `require`, certificates are not verified (self-signed dev servers work).
#[derive(Clone, Copy, Debug, PartialEq, Eq, Default, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum Ssl {
    Disable,
    #[default]
    Prefer,
    Require,
}

impl Ssl {
    pub fn title(self) -> &'static str {
        match self {
            Ssl::Disable => "disable",
            Ssl::Prefer => "prefer",
            Ssl::Require => "require",
        }
    }
    pub fn next(self) -> Self {
        match self {
            Ssl::Disable => Ssl::Prefer,
            Ssl::Prefer => Ssl::Require,
            Ssl::Require => Ssl::Disable,
        }
    }
}

/// A configured connection. Stored in `.brisk/dataSources.json` without the password.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct DataSource {
    pub id: String,
    pub name: String,
    pub driver: Driver,
    #[serde(default)]
    pub host: String,
    #[serde(default)]
    pub port: u16,
    #[serde(default)]
    pub user: String,
    #[serde(default)]
    pub database: String,
    /// SQLite database file; relative paths are relative to the project root.
    #[serde(default)]
    pub file: String,
    #[serde(default)]
    pub ssl: Ssl,
    /// PyCharm "Save: Forever" (true) or "Never" (false, asked once per session).
    #[serde(default = "yes")]
    pub save_password: bool,
}

fn yes() -> bool {
    true
}

impl DataSource {
    pub fn new(driver: Driver) -> Self {
        let mut ds = Self {
            id: new_id(),
            name: String::new(),
            driver,
            host: "localhost".into(),
            port: driver.default_port(),
            user: driver.default_user().into(),
            database: String::new(),
            file: String::new(),
            ssl: Ssl::default(),
            save_password: true,
        };
        if driver == Driver::Postgres {
            ds.database = "postgres".into();
        }
        ds
    }

    /// Switch driver, swapping in that driver's default port/user if the old ones were defaults.
    pub fn set_driver(&mut self, driver: Driver) {
        if self.port == 0 || self.port == self.driver.default_port() {
            self.port = driver.default_port();
        }
        if self.user.is_empty() || self.user == self.driver.default_user() {
            self.user = driver.default_user().into();
        }
        if driver == Driver::Postgres && self.database.is_empty() {
            self.database = "postgres".into();
        } else if self.driver == Driver::Postgres && self.database == "postgres" {
            self.database.clear();
        }
        if driver != Driver::Sqlite && self.host.is_empty() {
            self.host = "localhost".into();
        }
        self.driver = driver;
    }

    /// The name shown in the tree when the user left Name empty: `postgres@localhost`, `app.db`.
    pub fn display_name(&self) -> String {
        if !self.name.trim().is_empty() {
            return self.name.trim().to_string();
        }
        match self.driver {
            Driver::Sqlite => Path::new(&self.file).file_name().map(|n| n.to_string_lossy().into_owned()).unwrap_or_else(|| "sqlite".into()),
            _ => {
                let db = if self.database.is_empty() { self.user.as_str() } else { self.database.as_str() };
                format!("{db}@{}", if self.host.is_empty() { "localhost" } else { &self.host })
            }
        }
    }

    /// Connection URL without the password, shown under the fields like PyCharm's URL line.
    pub fn url(&self) -> String {
        match self.driver {
            Driver::Sqlite => format!("sqlite:{}", self.file),
            d => {
                let scheme = if d == Driver::Postgres { "postgresql" } else { "mysql" };
                let user = if self.user.is_empty() { String::new() } else { format!("{}@", self.user) };
                format!("{scheme}://{user}{}:{}/{}", self.host, self.port, self.database)
            }
        }
    }

    /// Fill fields from a pasted URL (`postgresql://u:p@host:5432/db?sslmode=require`,
    /// `jdbc:mysql://host/db`, `sqlite:path`). Returns the password if the URL has one.
    pub fn apply_url(&mut self, url: &str) -> Option<Option<String>> {
        let u = url.trim();
        let u = u.strip_prefix("jdbc:").unwrap_or(u);
        let (scheme, rest) = u.split_once(':')?;
        let driver = match scheme.to_ascii_lowercase().as_str() {
            "postgres" | "postgresql" => Driver::Postgres,
            "mysql" | "mariadb" => Driver::Mysql,
            "sqlite" | "sqlite3" => Driver::Sqlite,
            _ => return None,
        };
        self.set_driver(driver);
        if driver == Driver::Sqlite {
            let f = rest.trim_start_matches("//");
            self.file = f.replace("$PROJECT_DIR$/", "").replace("$PROJECT_DIR$\\", "");
            return Some(None);
        }
        let rest = rest.strip_prefix("//")?;
        let (main, query) = rest.split_once('?').unwrap_or((rest, ""));
        let (auth, hostpart) = match main.rsplit_once('@') {
            Some((a, h)) => (Some(a), h),
            None => (None, main),
        };
        let (hostport, db) = hostpart.split_once('/').unwrap_or((hostpart, ""));
        let (host, port) = match hostport.rsplit_once(':') {
            Some((h, p)) if p.parse::<u16>().is_ok() => (h, p.parse().unwrap()),
            _ => (hostport, driver.default_port()),
        };
        self.host = if host.is_empty() { "localhost".into() } else { host.to_string() };
        self.port = port;
        self.database = percent_decode(db);
        let mut password = None;
        if let Some(a) = auth {
            let (user, pass) = a.split_once(':').unwrap_or((a, ""));
            self.user = percent_decode(user);
            if a.contains(':') {
                password = Some(percent_decode(pass));
            }
        }
        for kv in query.split('&') {
            let (k, v) = kv.split_once('=').unwrap_or((kv, ""));
            match k {
                "sslmode" | "ssl-mode" => {
                    self.ssl = match v.to_ascii_lowercase().as_str() {
                        "disable" | "disabled" => Ssl::Disable,
                        "require" | "required" | "verify-ca" | "verify-full" | "verify_ca" | "verify_identity" => Ssl::Require,
                        _ => Ssl::Prefer,
                    }
                }
                "user" => self.user = percent_decode(v),
                "password" => password = Some(percent_decode(v)),
                _ => {}
            }
        }
        Some(password)
    }

    pub fn sqlite_path(&self, project_root: &Path) -> std::path::PathBuf {
        let p = Path::new(self.file.trim());
        if p.is_absolute() {
            p.to_path_buf()
        } else {
            project_root.join(p)
        }
    }
}

fn percent_decode(s: &str) -> String {
    let b = s.as_bytes();
    let mut out = Vec::with_capacity(b.len());
    let mut i = 0;
    while i < b.len() {
        if b[i] == b'%' && i + 2 < b.len() {
            if let Ok(v) = u8::from_str_radix(std::str::from_utf8(&b[i + 1..i + 3]).unwrap_or("zz"), 16) {
                out.push(v);
                i += 3;
                continue;
            }
        }
        out.push(b[i]);
        i += 1;
    }
    String::from_utf8_lossy(&out).into_owned()
}

pub fn new_id() -> String {
    use std::sync::atomic::{AtomicU64, Ordering};
    static N: AtomicU64 = AtomicU64::new(0);
    let t = std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).map(|d| d.as_nanos() as u64).unwrap_or(0);
    let n = N.fetch_add(1, Ordering::Relaxed);
    let x = t ^ (std::process::id() as u64).rotate_left(32) ^ n.wrapping_mul(0x9e37_79b9_7f4a_7c15);
    format!("{:016x}", x)
}

/// Output of one statement.
#[derive(Clone, Debug, Default)]
pub struct QueryOutput {
    pub statement: String,
    /// Empty for statements that return no rows (DDL, DML).
    pub columns: Vec<String>,
    /// `None` is SQL NULL.
    pub rows: Vec<Vec<Option<String>>>,
    pub affected: Option<u64>,
    /// More rows exist beyond the limit.
    pub truncated: bool,
    pub elapsed: Duration,
}

impl QueryOutput {
    pub fn has_rows(&self) -> bool {
        !self.columns.is_empty()
    }

    /// PyCharm-style summary for the output pane.
    pub fn summary(&self) -> String {
        let ms = self.elapsed.as_millis();
        if self.has_rows() {
            let n = self.rows.len();
            let more = if self.truncated { "+" } else { "" };
            format!("{n}{more} row{} retrieved starting from 1 in {ms} ms", if n == 1 && !self.truncated { "" } else { "s" })
        } else {
            match self.affected {
                Some(n) => format!("{n} row{} affected in {ms} ms", if n == 1 { "" } else { "s" }),
                None => format!("completed in {ms} ms"),
            }
        }
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum TableKind {
    Table,
    View,
}

#[derive(Clone, Debug, PartialEq)]
pub struct Column {
    pub name: String,
    pub type_name: String,
    pub nullable: bool,
    pub primary_key: bool,
}

#[derive(Clone, Debug, PartialEq)]
pub struct Table {
    pub name: String,
    pub kind: TableKind,
    pub columns: Vec<Column>,
}

#[derive(Clone, Debug, PartialEq)]
pub struct Schema {
    pub name: String,
    pub tables: Vec<Table>,
}

/// Cancels the statement currently running on a connection, from any thread (Ctrl+F2).
#[derive(Clone)]
pub struct Canceller(Arc<dyn Fn() + Send + Sync>);

impl Canceller {
    pub fn cancel(&self) {
        (self.0)()
    }
}

/// A live connection. Calls block; one statement at a time per connection.
pub trait Connection: Send {
    fn driver(&self) -> Driver;
    /// Run one statement and return at most `limit` rows.
    fn run(&mut self, sql: &str, limit: usize) -> Result<QueryOutput, String>;
    fn canceller(&self) -> Canceller;
    /// e.g. "PostgreSQL 16.2".
    fn server_version(&mut self) -> String;
}

/// Open a connection. `project_root` resolves relative SQLite paths.
pub fn connect(ds: &DataSource, password: &str, project_root: &Path) -> Result<Box<dyn Connection>, String> {
    match ds.driver {
        Driver::Sqlite => sqlite::connect(&ds.sqlite_path(project_root)).map(|c| Box::new(c) as Box<dyn Connection>),
        Driver::Postgres => pg::connect(ds, password).map(|c| Box::new(c) as Box<dyn Connection>),
        Driver::Mysql => my::connect(ds, password).map(|c| Box::new(c) as Box<dyn Connection>),
    }
}

/// Run a script: every statement in order, stopping at the first error.
/// Returns the outputs so far and the error, if any.
pub fn execute(conn: &mut dyn Connection, script: &str, limit: usize) -> (Vec<QueryOutput>, Option<String>) {
    let mut out = Vec::new();
    for r in split_statements(script, conn.driver()) {
        let stmt = &script[r];
        let t0 = Instant::now();
        match conn.run(stmt, limit) {
            Ok(mut o) => {
                o.statement = stmt.to_string();
                o.elapsed = t0.elapsed();
                out.push(o);
            }
            Err(e) => return (out, Some(e)),
        }
    }
    (out, None)
}

/// Does running this statement change the schema (so the tree should be refreshed)?
pub fn is_ddl(stmt: &str) -> bool {
    let first = first_keyword(stmt);
    matches!(first.as_str(), "create" | "alter" | "drop" | "rename" | "truncate" | "comment") || (first == "attach" || first == "detach")
}

/// First keyword of a statement, lowercase, skipping comments and parentheses.
pub fn first_keyword(stmt: &str) -> String {
    let mut s = stmt.trim_start();
    loop {
        if let Some(r) = s.strip_prefix("--") {
            s = r.split_once('\n').map(|x| x.1).unwrap_or("").trim_start();
        } else if let Some(r) = s.strip_prefix("/*") {
            s = r.split_once("*/").map(|x| x.1).unwrap_or("").trim_start();
        } else if let Some(r) = s.strip_prefix('(') {
            s = r.trim_start();
        } else {
            break;
        }
    }
    s.chars().take_while(|c| c.is_alphanumeric() || *c == '_').collect::<String>().to_ascii_lowercase()
}

/// Quote an identifier for this driver.
pub fn quote_ident(driver: Driver, name: &str) -> String {
    match driver {
        Driver::Mysql => format!("`{}`", name.replace('`', "``")),
        _ => format!("\"{}\"", name.replace('"', "\"\"")),
    }
}

/// The query behind "Edit Data" (F4 on a table).
pub fn select_all(driver: Driver, schema: &str, table: &str) -> String {
    let t = quote_ident(driver, table);
    if schema.is_empty() || (driver == Driver::Sqlite && schema == "main") {
        format!("SELECT * FROM {t}")
    } else {
        format!("SELECT * FROM {}.{t}", quote_ident(driver, schema))
    }
}

/// Read the schema tree: schemas, tables/views, columns.
pub fn introspect(conn: &mut dyn Connection) -> Result<Vec<Schema>, String> {
    let rows = match conn.driver() {
        Driver::Sqlite => return sqlite_introspect(conn),
        Driver::Postgres => conn.run(pg::INTROSPECT, usize::MAX)?,
        Driver::Mysql => conn.run(my::INTROSPECT, usize::MAX)?,
    };
    let empty_schemas = match conn.driver() {
        Driver::Postgres => conn.run(pg::SCHEMAS, usize::MAX)?.rows,
        Driver::Mysql => conn.run(my::SCHEMAS, usize::MAX)?.rows,
        Driver::Sqlite => Vec::new(),
    };
    let s = |r: &Vec<Option<String>>, i: usize| r.get(i).cloned().flatten().unwrap_or_default();
    let mut schemas: Vec<Schema> = Vec::new();
    for r in &empty_schemas {
        schemas.push(Schema { name: s(r, 0), tables: Vec::new() });
    }
    for r in &rows.rows {
        let (sn, tn, tt) = (s(r, 0), s(r, 1), s(r, 2));
        let col = Column {
            name: s(r, 3),
            type_name: s(r, 4),
            nullable: s(r, 5).eq_ignore_ascii_case("yes"),
            primary_key: matches!(s(r, 6).as_str(), "t" | "true" | "1"),
        };
        let si = match schemas.iter().position(|x| x.name == sn) {
            Some(i) => i,
            None => {
                schemas.push(Schema { name: sn, tables: Vec::new() });
                schemas.len() - 1
            }
        };
        let tables = &mut schemas[si].tables;
        if tables.last().is_none_or(|t| t.name != tn) {
            let kind = if tt.to_ascii_uppercase().contains("VIEW") { TableKind::View } else { TableKind::Table };
            tables.push(Table { name: tn, kind, columns: Vec::new() });
        }
        tables.last_mut().unwrap().columns.push(col);
    }
    // Tables first, then views, alphabetical, as in PyCharm's tree.
    for sc in &mut schemas {
        sc.tables.sort_by(|a, b| (a.kind == TableKind::View).cmp(&(b.kind == TableKind::View)).then_with(|| a.name.to_lowercase().cmp(&b.name.to_lowercase())));
    }
    schemas.sort_by(|a, b| a.name.cmp(&b.name));
    Ok(schemas)
}

fn sqlite_introspect(conn: &mut dyn Connection) -> Result<Vec<Schema>, String> {
    let list = conn.run("SELECT name, type FROM sqlite_master WHERE type IN ('table','view') AND name NOT LIKE 'sqlite_%' ORDER BY type, name", usize::MAX)?;
    let mut tables = Vec::new();
    for r in list.rows {
        let name = r.first().cloned().flatten().unwrap_or_default();
        let kind = if r.get(1).cloned().flatten().as_deref() == Some("view") { TableKind::View } else { TableKind::Table };
        let info = conn.run(&format!("PRAGMA table_info({})", quote_ident(Driver::Sqlite, &name)), usize::MAX)?;
        // cid, name, type, notnull, dflt_value, pk
        let columns = info
            .rows
            .iter()
            .map(|c| {
                let g = |i: usize| c.get(i).cloned().flatten().unwrap_or_default();
                Column { name: g(1), type_name: g(2).to_lowercase(), nullable: g(3) == "0", primary_key: g(5) != "0" }
            })
            .collect();
        tables.push(Table { name, kind, columns });
    }
    tables.sort_by(|a, b| (a.kind == TableKind::View).cmp(&(b.kind == TableKind::View)).then_with(|| a.name.to_lowercase().cmp(&b.name.to_lowercase())));
    Ok(vec![Schema { name: "main".into(), tables }])
}

/// Connect, read the server version and close: the "Test Connection" button.
pub fn test_connection(ds: &DataSource, password: &str, project_root: &Path) -> Result<String, String> {
    let t0 = Instant::now();
    let mut c = connect(ds, password, project_root)?;
    let v = c.server_version();
    Ok(format!("{v} · {} ms", t0.elapsed().as_millis()))
}
