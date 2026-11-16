//! Database tool window (right stripe), Data Source dialog, query consoles and result grids,
//! following PyCharm Professional / DataGrip.
//!
//! Connections live for the session, one per data source, opened lazily (on expand, console
//! or query). Every driver call runs on the background executor; a connection is a mutex,
//! so statements on the same data source queue up as in a single JDBC session.

use super::*;
use crate::grid_view::{GridEvent, GridView};
use brisk_db::{Canceller, Connection, DataSource, Driver, Schema, Ssl, TableKind};
use std::collections::{HashMap, HashSet};
use std::sync::Mutex;

type Conn = Arc<Mutex<Box<dyn Connection>>>;

pub struct Live {
    conn: Conn,
    cancel: Canceller,
    pub schemas: Vec<Schema>,
    pub version: String,
}

pub enum ConnState {
    Connecting,
    Live(Live),
    Failed(String),
}

/// What to do once a data source is connected.
#[derive(Clone)]
pub enum After {
    Nothing,
    Run { grid: Entity<GridView>, sql: String },
}

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum Field {
    Name,
    Driver,
    Host,
    Port,
    User,
    Password,
    SavePassword,
    Database,
    Ssl,
    File,
}

impl Field {
    fn label(self) -> &'static str {
        match self {
            Field::Name => "Name:",
            Field::Driver => "Driver:",
            Field::Host => "Host:",
            Field::Port => "Port:",
            Field::User => "User:",
            Field::Password => "Password:",
            Field::SavePassword => "Save:",
            Field::Database => "Database:",
            Field::Ssl => "SSL mode:",
            Field::File => "File:",
        }
    }
    fn is_text(self) -> bool {
        !matches!(self, Field::Driver | Field::SavePassword | Field::Ssl)
    }
}

fn fields(d: Driver) -> &'static [Field] {
    use Field as F;
    match d {
        Driver::Sqlite => &[F::Name, F::Driver, F::File],
        _ => &[F::Name, F::Driver, F::Host, F::Port, F::User, F::Password, F::SavePassword, F::Database, F::Ssl],
    }
}

pub struct Dialog {
    pub ds: DataSource,
    /// Index into `sources` when editing; None for a new data source.
    editing: Option<usize>,
    port: String,
    password: String,
    password_dirty: bool,
    field: Field,
    select_all: bool,
    testing: bool,
    test: Option<Result<String, String>>,
    note: Option<String>,
    after: Option<After>,
}

#[derive(Clone, Debug, PartialEq)]
enum RowKind {
    Source(usize),
    Schema(usize, usize),
    Table(usize, usize, usize),
    Column(usize, usize, usize, usize),
    Message(usize, String, bool),
}

#[derive(Clone)]
struct Row {
    depth: usize,
    kind: RowKind,
    key: String,
    expandable: bool,
}

pub struct Db {
    pub sources: Vec<DataSource>,
    pub states: HashMap<String, ConnState>,
    session_passwords: HashMap<String, String>,
    expanded: HashSet<String>,
    rows: Vec<Row>,
    selected: usize,
    scroll: UniformListScrollHandle,
    pub focus: FocusHandle,
    pub open: bool,
    pub dialog: Option<Dialog>,
    /// Output of console statements, shown in Services (Alt+8).
    pub results: Option<Entity<GridView>>,
    /// The canceller of the statement running now (Ctrl+F2 / ■).
    pub running: Option<Canceller>,
    pending: HashMap<String, Vec<After>>,
    delete_armed: Option<String>,
    last_used: Option<String>,
}

impl Db {
    pub fn new(root: &Path, cx: &mut Context<Workspace>) -> (Self, bool) {
        let (sources, imported) = brisk_db::load_sources(root);
        let mut db = Self {
            sources,
            states: HashMap::new(),
            session_passwords: HashMap::new(),
            expanded: HashSet::new(),
            rows: Vec::new(),
            selected: 0,
            scroll: UniformListScrollHandle::new(),
            focus: cx.focus_handle(),
            open: false,
            dialog: None,
            results: None,
            running: None,
            pending: HashMap::new(),
            delete_armed: None,
            last_used: None,
        };
        db.rebuild();
        (db, imported)
    }

    fn rebuild(&mut self) {
        let mut rows = Vec::new();
        for (di, ds) in self.sources.iter().enumerate() {
            let dk = format!("ds:{}", ds.id);
            rows.push(Row { depth: 0, kind: RowKind::Source(di), key: dk.clone(), expandable: true });
            if !self.expanded.contains(&dk) {
                continue;
            }
            match self.states.get(&ds.id) {
                None => {}
                Some(ConnState::Connecting) => rows.push(Row { depth: 1, kind: RowKind::Message(di, "Connecting…".into(), false), key: String::new(), expandable: false }),
                Some(ConnState::Failed(e)) => rows.push(Row { depth: 1, kind: RowKind::Message(di, first_line(e), true), key: String::new(), expandable: false }),
                Some(ConnState::Live(live)) => {
                    if live.schemas.is_empty() {
                        rows.push(Row { depth: 1, kind: RowKind::Message(di, "No schemas".into(), false), key: String::new(), expandable: false });
                    }
                    for (si, s) in live.schemas.iter().enumerate() {
                        let sk = format!("{dk}/s:{}", s.name);
                        rows.push(Row { depth: 1, kind: RowKind::Schema(di, si), key: sk.clone(), expandable: true });
                        if !self.expanded.contains(&sk) {
                            continue;
                        }
                        for (ti, t) in s.tables.iter().enumerate() {
                            let tk = format!("{sk}/t:{}", t.name);
                            rows.push(Row { depth: 2, kind: RowKind::Table(di, si, ti), key: tk.clone(), expandable: !t.columns.is_empty() });
                            if !self.expanded.contains(&tk) {
                                continue;
                            }
                            for ci in 0..t.columns.len() {
                                rows.push(Row { depth: 3, kind: RowKind::Column(di, si, ti, ci), key: String::new(), expandable: false });
                            }
                        }
                    }
                }
            }
        }
        self.rows = rows;
        self.selected = self.selected.min(self.rows.len().saturating_sub(1));
    }

    fn selected_source(&self) -> Option<usize> {
        match self.rows.get(self.selected)?.kind {
            RowKind::Source(d) | RowKind::Schema(d, _) | RowKind::Table(d, ..) | RowKind::Column(d, ..) | RowKind::Message(d, ..) => Some(d),
        }
    }

    fn live(&self, di: usize) -> Option<&Live> {
        match self.states.get(&self.sources.get(di)?.id)? {
            ConnState::Live(l) => Some(l),
            _ => None,
        }
    }

    /// Data source of a SQL file: consoles live in `<config>/consoles/<id>/`; other .sql files
    /// use the last data source used (PyCharm asks to attach one; we remember the last).
    pub fn source_for_file(&self, path: &Path) -> Option<usize> {
        let consoles = brisk_db::config_dir().join("consoles");
        if let Ok(rel) = path.strip_prefix(&consoles) {
            let id = rel.components().next()?.as_os_str().to_string_lossy().into_owned();
            return self.sources.iter().position(|s| s.id == id);
        }
        let last = self.last_used.as_ref().and_then(|id| self.sources.iter().position(|s| &s.id == id));
        last.or_else(|| (self.sources.len() == 1).then_some(0)).or_else(|| self.selected_source())
    }

    fn save(&self, root: &Path) -> Result<(), String> {
        brisk_db::save_sources(root, &self.sources).map_err(|e| format!("Cannot save {}: {e}", brisk_db::CONFIG_FILE))
    }
}

fn first_line(s: &str) -> String {
    s.lines().next().unwrap_or("").chars().take(200).collect()
}

fn console_path(ds: &DataSource) -> PathBuf {
    let name: String = ds.display_name().chars().map(|c| if c.is_alphanumeric() || "-_.@ ".contains(c) { c } else { '_' }).collect();
    brisk_db::config_dir().join("consoles").join(&ds.id).join(format!("{}.sql", name.trim()))
}

pub fn driver_badge(d: Driver) -> (&'static str, gpui::Hsla) {
    match d {
        Driver::Postgres => ("Pg", gpui::rgb(0x6c9be8).into()),
        Driver::Mysql => ("My", gpui::rgb(0xe0a85a).into()),
        Driver::Sqlite => ("Lt", gpui::rgb(0x5fb865).into()),
    }
}

impl Workspace {
    // --- Connections ------------------------------------------------------------------

    fn password_for(&self, ds: &DataSource) -> Option<String> {
        if ds.driver == Driver::Sqlite {
            return Some(String::new());
        }
        if let Some(p) = self.db.session_passwords.get(&ds.id) {
            return Some(p.clone());
        }
        if ds.save_password {
            return Some(brisk_db::load_password(&ds.id).unwrap_or_default());
        }
        None
    }

    /// Connect if needed, then do `after`.
    pub(super) fn db_connect(&mut self, di: usize, after: After, window: &mut Window, cx: &mut Context<Self>) {
        let Some(ds) = self.db.sources.get(di).cloned() else { return };
        match self.db.states.get(&ds.id) {
            Some(ConnState::Live(_)) => return self.db_after(di, after, cx),
            Some(ConnState::Connecting) => {
                self.db.pending.entry(ds.id.clone()).or_default().push(after);
                return;
            }
            _ => {}
        }
        let Some(password) = self.password_for(&ds) else {
            // "Save: Never" and nothing entered this session: ask, like PyCharm's password prompt.
            self.open_ds_dialog(Some(di), Some("Enter the password to connect".into()), Some(after), window, cx);
            return;
        };
        self.db.pending.entry(ds.id.clone()).or_default().push(after);
        self.db.states.insert(ds.id.clone(), ConnState::Connecting);
        self.db.expanded.insert(format!("ds:{}", ds.id));
        self.db.rebuild();
        let root = self.root.clone();
        let task = cx.background_executor().spawn(async move {
            let mut c = brisk_db::connect(&ds, &password, &root)?;
            let version = c.server_version();
            let schemas = brisk_db::introspect(c.as_mut())?;
            Ok::<_, String>((c, version, schemas))
        });
        let id = self.db.sources[di].id.clone();
        cx.spawn(async move |this, cx| {
            let res = task.await;
            let _ = this.update(cx, |this, cx| {
                let afters = this.db.pending.remove(&id).unwrap_or_default();
                match res {
                    Ok((c, version, schemas)) => {
                        let cancel = c.canceller();
                        // Open the default schema: public / main / the database itself.
                        let ds = this.db.sources.iter().find(|s| s.id == id).cloned();
                        if let Some(ds) = &ds {
                            let def = schemas
                                .iter()
                                .find(|s| s.name == "public" || s.name == "main" || s.name == ds.database)
                                .or_else(|| (schemas.len() == 1).then(|| &schemas[0]));
                            if let Some(s) = def {
                                this.db.expanded.insert(format!("ds:{}/s:{}", ds.id, s.name));
                            }
                        }
                        let n: usize = schemas.iter().map(|s| s.tables.len()).sum();
                        this.status = format!("Connected: {version}, {n} tables");
                        this.db.states.insert(id.clone(), ConnState::Live(Live { conn: Arc::new(Mutex::new(c)), cancel, schemas, version }));
                        this.db.rebuild();
                        if let Some(di) = this.db.sources.iter().position(|s| s.id == id) {
                            for a in afters {
                                this.db_after(di, a, cx);
                            }
                        }
                    }
                    Err(e) => {
                        this.show_toast(format!("Connection failed: {}", first_line(&e)), true, cx);
                        for a in afters {
                            if let After::Run { grid, .. } = a {
                                grid.update(cx, |g, cx| g.set_error(e.clone(), cx));
                            }
                        }
                        this.db.states.insert(id.clone(), ConnState::Failed(e));
                        this.db.rebuild();
                    }
                }
                cx.notify();
            });
        })
        .detach();
        cx.notify();
    }

    fn db_after(&mut self, di: usize, after: After, cx: &mut Context<Self>) {
        match after {
            After::Nothing => {}
            After::Run { grid, sql } => self.db_run(di, grid, sql, cx),
        }
    }

    /// Run a script on a data source and show the output in `grid`.
    fn db_run(&mut self, di: usize, grid: Entity<GridView>, sql: String, cx: &mut Context<Self>) {
        let Some(live) = self.db.live(di) else { return };
        let conn = live.conn.clone();
        self.db.running = Some(live.cancel.clone());
        self.db.last_used = Some(self.db.sources[di].id.clone());
        let id = self.db.sources[di].id.clone();
        grid.update(cx, |g, cx| g.set_running(&sql, cx));
        let script = sql.clone();
        let task = cx.background_executor().spawn(async move {
            let mut c = conn.lock().unwrap_or_else(|p| p.into_inner());
            let r = brisk_db::execute(c.as_mut(), &script, brisk_db::DEFAULT_LIMIT);
            let ddl = brisk_db::split_statements(&script).into_iter().any(|r| brisk_db::is_ddl(&script[r]));
            let schemas = if ddl { brisk_db::introspect(c.as_mut()).ok() } else { None };
            (r, schemas)
        });
        cx.spawn(async move |this, cx| {
            let ((outs, err), schemas) = task.await;
            let _ = this.update(cx, |this, cx| {
                this.db.running = None;
                if let Some(s) = schemas {
                    if let Some(ConnState::Live(l)) = this.db.states.get_mut(&id) {
                        l.schemas = s;
                    }
                    this.db.rebuild();
                }
                let n = outs.len();
                // The last result set wins; a row-less script shows its last statement's summary.
                let shown = match outs.iter().rposition(|o| o.has_rows()) {
                    Some(i) => outs.into_iter().nth(i),
                    None => outs.into_iter().last(),
                };
                grid.update(cx, |g, cx| match (err, shown) {
                    (Some(e), _) => g.set_error(if n > 0 { format!("{e}  ({n} statement(s) succeeded before the error)") } else { e }, cx),
                    (None, Some(o)) => {
                        let m = if n > 1 { format!("{} · {n} statements", o.summary()) } else { o.summary() };
                        g.set_output(o, m, cx)
                    }
                    (None, None) => g.set_error("Nothing to execute".into(), cx),
                });
                cx.notify();
            });
        })
        .detach();
    }

    pub(super) fn db_disconnect(&mut self, di: usize, cx: &mut Context<Self>) {
        if let Some(ds) = self.db.sources.get(di) {
            if self.db.states.remove(&ds.id).is_some() {
                self.status = format!("Disconnected from {}", ds.display_name());
            }
            self.db.expanded.retain(|k| !k.starts_with(&format!("ds:{}", ds.id)));
            self.db.rebuild();
            cx.notify();
        }
    }

    fn db_refresh(&mut self, di: usize, window: &mut Window, cx: &mut Context<Self>) {
        let Some(live) = self.db.live(di) else {
            self.db.states.remove(&self.db.sources[di].id);
            return self.db_connect(di, After::Nothing, window, cx);
        };
        let conn = live.conn.clone();
        let id = self.db.sources[di].id.clone();
        let t0 = Instant::now();
        let task = cx.background_executor().spawn(async move {
            let mut c = conn.lock().unwrap_or_else(|p| p.into_inner());
            brisk_db::introspect(c.as_mut())
        });
        cx.spawn(async move |this, cx| {
            let r = task.await;
            let _ = this.update(cx, |this, cx| {
                match r {
                    Ok(s) => {
                        if let Some(ConnState::Live(l)) = this.db.states.get_mut(&id) {
                            l.schemas = s;
                        }
                        this.status = format!("Database refreshed in {} ms", t0.elapsed().as_millis());
                    }
                    Err(e) => this.show_toast(format!("Refresh failed: {}", first_line(&e)), true, cx),
                }
                this.db.rebuild();
                cx.notify();
            });
        })
        .detach();
    }

    // --- Actions ----------------------------------------------------------------------

    pub(super) fn db_action(&mut self, action: Action, window: &mut Window, cx: &mut Context<Self>) {
        use Action as A;
        match action {
            A::ToggleDatabaseWindow => {
                if self.db.open && self.db.focus.is_focused(window) {
                    self.db.open = false;
                    self.focus_editor(window, cx);
                } else {
                    self.db.open = true;
                    window.focus(&self.db.focus);
                }
            }
            A::NewDataSource => self.open_ds_dialog(None, None, None, window, cx),
            A::DataSourceProperties => {
                if let Some(di) = self.db.selected_source() {
                    self.open_ds_dialog(Some(di), None, None, window, cx);
                }
            }
            A::OpenQueryConsole => {
                let di = if self.db.focus.is_focused(window) {
                    self.db.selected_source()
                } else {
                    self.active_editor().and_then(|e| e.read(cx).path.clone()).and_then(|p| self.db.source_for_file(&p)).or_else(|| self.db.selected_source())
                };
                match di {
                    Some(di) => self.open_console(di, window, cx),
                    None => self.show_toast("No data sources. Add one in the Database tool window (Alt+Insert).".into(), true, cx),
                }
            }
            A::EditTableData => self.db_open_selected(window, cx),
            A::RefreshDatabase => {
                if let Some(g) = self.focused_grid(window, cx) {
                    self.grid_reload(g, window, cx);
                } else if let Some(di) = self.db.selected_source() {
                    self.db_refresh(di, window, cx);
                }
            }
            A::Disconnect => {
                if let Some(di) = self.db.selected_source() {
                    self.db_disconnect(di, cx);
                }
            }
            A::ExecuteQuery => self.execute_console(window, cx),
            _ => {}
        }
        cx.notify();
    }

    pub(super) fn focused_grid(&self, window: &Window, cx: &App) -> Option<Entity<GridView>> {
        if let Some(Tab::Grid(g)) = self.active.and_then(|i| self.tabs.get(i)) {
            if g.read(cx).focus_handle(cx).is_focused(window) {
                return Some(g.clone());
            }
        }
        self.db.results.clone().filter(|g| g.read(cx).focus_handle(cx).is_focused(window))
    }

    fn grid_reload(&mut self, grid: Entity<GridView>, window: &mut Window, cx: &mut Context<Self>) {
        let Some((id, sql)) = grid.read(cx).source.clone() else { return };
        if let Some(di) = self.db.sources.iter().position(|s| s.id == id) {
            self.db_connect(di, After::Run { grid, sql }, window, cx);
        }
    }

    fn new_grid(&mut self, title: String, cx: &mut Context<Self>) -> Entity<GridView> {
        let g = cx.new(|cx| GridView::new(title, cx));
        cx.subscribe(&g, |this: &mut Self, g, ev: &GridEvent, cx| match ev {
            GridEvent::Cancel => {
                if let Some(c) = &this.db.running {
                    c.cancel();
                }
            }
            GridEvent::Reload => {
                let Some((id, sql)) = g.read(cx).source.clone() else { return };
                if let Some(di) = this.db.sources.iter().position(|s| s.id == id) {
                    if this.db.live(di).is_some() {
                        this.db_run(di, g, sql, cx);
                    }
                }
            }
        })
        .detach();
        g
    }

    /// Ctrl+Enter in a SQL file: the selection or the statement at the caret.
    fn execute_console(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let Some(ed) = self.focused_editor(window, cx) else { return };
        let path = ed.read(cx).path.clone();
        let Some(di) = path.as_deref().and_then(|p| self.db.source_for_file(p)) else {
            self.show_toast("No data source for this file. Add one in the Database tool window (Alt+Insert).".into(), true, cx);
            return;
        };
        let (sel, text, caret) = ed.update(cx, |e, _| e.sql_to_run());
        let sql = match sel {
            Some(s) => s,
            None => match brisk_db::statement_at(&text, caret) {
                Some(r) => {
                    // Flash the statement as a selection, as PyCharm highlights what it runs.
                    text[r].to_string()
                }
                None => {
                    self.status = "Nothing to execute".into();
                    return;
                }
            },
        };
        let _ = ed.update(cx, |e, _| e.save());
        let grid = match &self.db.results {
            Some(g) => g.clone(),
            None => {
                let g = self.new_grid("Output".into(), cx);
                self.db.results = Some(g.clone());
                g
            }
        };
        let id = self.db.sources[di].id.clone();
        grid.update(cx, |g, _| {
            g.title = self.db.sources[di].display_name();
            g.source = Some((id, sql.clone()));
        });
        self.bottom = Some(BottomTool::Services);
        self.db_connect(di, After::Run { grid, sql }, window, cx);
    }

    pub(super) fn open_console(&mut self, di: usize, window: &mut Window, cx: &mut Context<Self>) {
        let ds = self.db.sources[di].clone();
        let path = console_path(&ds);
        if let Some(d) = path.parent() {
            let _ = std::fs::create_dir_all(d);
        }
        if !path.exists() {
            if let Err(e) = std::fs::write(&path, "") {
                return self.show_toast(format!("Cannot create console: {e}"), true, cx);
            }
        }
        self.db.last_used = Some(ds.id.clone());
        self.open_file(&path, window, cx);
        // Connect in the background, like PyCharm does when a console opens.
        self.db_connect(di, After::Nothing, window, cx);
    }

    /// F4 / double-click: tables and views open their data; data sources expand.
    fn db_open_selected(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let Some(row) = self.db.rows.get(self.db.selected).cloned() else { return };
        let (di, si, ti) = match row.kind {
            RowKind::Table(d, s, t) | RowKind::Column(d, s, t, _) => (d, s, t),
            RowKind::Source(d) => return self.open_console(d, window, cx),
            _ => return self.db_toggle(self.db.selected, window, cx),
        };
        let Some(live) = self.db.live(di) else { return };
        let schema = &live.schemas[si];
        let table = &schema.tables[ti];
        let driver = self.db.sources[di].driver;
        let sql = brisk_db::select_all(driver, &schema.name, &table.name);
        let id = self.db.sources[di].id.clone();
        let title = table.name.clone();
        // Reuse an open tab for the same table.
        if let Some(i) = self.tabs.iter().position(|t| matches!(t, Tab::Grid(g) if g.read(cx).source.as_ref() == Some(&(id.clone(), sql.clone())))) {
            self.active = Some(i);
            self.focus_editor(window, cx);
            return;
        }
        let g = self.new_grid(title, cx);
        g.update(cx, |g, _| g.source = Some((id, sql.clone())));
        self.add_tab(Tab::Grid(g.clone()));
        let f = g.read(cx).focus_handle(cx);
        window.focus(&f);
        self.db_connect(di, After::Run { grid: g, sql }, window, cx);
    }

    fn db_toggle(&mut self, ix: usize, window: &mut Window, cx: &mut Context<Self>) {
        let Some(row) = self.db.rows.get(ix).cloned() else { return };
        if !row.expandable {
            return;
        }
        if self.db.expanded.contains(&row.key) {
            self.db.expanded.remove(&row.key);
        } else {
            self.db.expanded.insert(row.key.clone());
            if let RowKind::Source(di) = row.kind {
                if !matches!(self.db.states.get(&self.db.sources[di].id), Some(ConnState::Live(_) | ConnState::Connecting)) {
                    self.db.states.remove(&self.db.sources[di].id);
                    self.db_connect(di, After::Nothing, window, cx);
                }
            }
        }
        self.db.rebuild();
        cx.notify();
    }

    /// Keys in the Database tree that the keymap doesn't bind.
    pub(super) fn db_tree_key(&mut self, k: &gpui::Keystroke, window: &mut Window, cx: &mut Context<Self>) -> bool {
        let n = self.db.rows.len();
        if k.key != "delete" {
            self.db.delete_armed = None;
        }
        match k.key.as_str() {
            "up" => self.db.selected = self.db.selected.saturating_sub(1),
            "down" => self.db.selected = (self.db.selected + 1).min(n.saturating_sub(1)),
            "pageup" => self.db.selected = self.db.selected.saturating_sub(15),
            "pagedown" => self.db.selected = (self.db.selected + 15).min(n.saturating_sub(1)),
            "home" => self.db.selected = 0,
            "end" => self.db.selected = n.saturating_sub(1),
            "right" => {
                let Some(r) = self.db.rows.get(self.db.selected).cloned() else { return true };
                if r.expandable && !self.db.expanded.contains(&r.key) {
                    self.db_toggle(self.db.selected, window, cx);
                } else if self.db.rows.get(self.db.selected + 1).is_some_and(|x| x.depth > r.depth) {
                    self.db.selected += 1;
                }
            }
            "left" => {
                let Some(r) = self.db.rows.get(self.db.selected).cloned() else { return true };
                if r.expandable && self.db.expanded.contains(&r.key) {
                    self.db_toggle(self.db.selected, window, cx);
                } else if let Some(p) = (0..self.db.selected).rev().find(|&i| self.db.rows[i].depth < r.depth) {
                    self.db.selected = p;
                }
            }
            "enter" => {
                match self.db.rows.get(self.db.selected).map(|r| r.kind.clone()) {
                    Some(RowKind::Table(..) | RowKind::Column(..)) => self.db_open_selected(window, cx),
                    _ => self.db_toggle(self.db.selected, window, cx),
                }
            }
            "delete" => {
                let Some(di) = self.db.selected_source() else { return true };
                if !matches!(self.db.rows.get(self.db.selected).map(|r| &r.kind), Some(RowKind::Source(_))) {
                    return true;
                }
                let id = self.db.sources[di].id.clone();
                if self.db.delete_armed.as_deref() == Some(id.as_str()) {
                    self.db.delete_armed = None;
                    self.db_disconnect(di, cx);
                    let ds = self.db.sources.remove(di);
                    brisk_db::forget_password(&ds.id);
                    if let Err(e) = self.db.save(&self.root) {
                        self.show_toast(e, true, cx);
                    }
                    self.db.rebuild();
                    self.status = format!("Removed data source {}", ds.display_name());
                } else {
                    self.db.delete_armed = Some(id);
                    self.show_toast(format!("Press Delete again to remove data source '{}'", self.db.sources[di].display_name()), false, cx);
                }
            }
            _ => return false,
        }
        self.db.scroll.scroll_to_item(self.db.selected, gpui::ScrollStrategy::Top);
        cx.notify();
        true
    }

    // --- Data Source dialog --------------------------------------------------------------

    pub(super) fn open_ds_dialog(&mut self, editing: Option<usize>, note: Option<String>, after: Option<After>, window: &mut Window, cx: &mut Context<Self>) {
        let ds = editing.and_then(|i| self.db.sources.get(i).cloned()).unwrap_or_else(|| DataSource::new(Driver::Postgres));
        let password = if editing.is_some() { self.password_for(&ds).unwrap_or_default() } else { String::new() };
        let field = if note.is_some() { Field::Password } else if editing.is_some() { Field::Name } else { Field::Host };
        self.db.dialog = Some(Dialog {
            port: if ds.port == 0 { String::new() } else { ds.port.to_string() },
            ds,
            editing,
            password,
            password_dirty: false,
            field,
            select_all: false,
            testing: false,
            test: None,
            note,
            after,
        });
        window.focus(&self.focus);
        cx.notify();
    }

    fn dialog_text(d: &mut Dialog, f: Field) -> Option<&mut String> {
        Some(match f {
            Field::Name => &mut d.ds.name,
            Field::Host => &mut d.ds.host,
            Field::Port => &mut d.port,
            Field::User => &mut d.ds.user,
            Field::Password => {
                d.password_dirty = true;
                &mut d.password
            }
            Field::Database => &mut d.ds.database,
            Field::File => &mut d.ds.file,
            _ => return None,
        })
    }

    fn dialog_toggle(d: &mut Dialog, f: Field, back: bool) {
        match f {
            Field::Driver => {
                let all = Driver::ALL;
                let i = all.iter().position(|x| *x == d.ds.driver).unwrap_or(0);
                let next = all[if back { (i + all.len() - 1) % all.len() } else { (i + 1) % all.len() }];
                d.ds.set_driver(next);
                d.port = if d.ds.port == 0 { String::new() } else { d.ds.port.to_string() };
            }
            Field::Ssl => d.ds.ssl = if back { d.ds.ssl.next().next() } else { d.ds.ssl.next() },
            Field::SavePassword => d.ds.save_password = !d.ds.save_password,
            _ => {}
        }
        d.test = None;
    }

    fn dialog_commit_fields(d: &mut Dialog) {
        d.ds.port = d.port.trim().parse().unwrap_or(d.ds.driver.default_port());
        d.ds.host = d.ds.host.trim().to_string();
        d.ds.user = d.ds.user.trim().to_string();
        d.ds.database = d.ds.database.trim().to_string();
        d.ds.file = d.ds.file.trim().trim_matches('"').to_string();
    }

    fn dialog_test(&mut self, cx: &mut Context<Self>) {
        let Some(d) = self.db.dialog.as_mut() else { return };
        Self::dialog_commit_fields(d);
        d.testing = true;
        d.test = None;
        let (ds, pw, root) = (d.ds.clone(), d.password.clone(), self.root.clone());
        let task = cx.background_executor().spawn(async move { brisk_db::test_connection(&ds, &pw, &root) });
        cx.spawn(async move |this, cx| {
            let r = task.await;
            let _ = this.update(cx, |this, cx| {
                if let Some(d) = this.db.dialog.as_mut() {
                    d.testing = false;
                    d.test = Some(r);
                }
                cx.notify();
            });
        })
        .detach();
        cx.notify();
    }

    fn dialog_ok(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let Some(mut d) = self.db.dialog.take() else { return };
        Self::dialog_commit_fields(&mut d);
        let missing = match d.ds.driver {
            Driver::Sqlite => d.ds.file.is_empty().then_some("Specify the database file"),
            _ => d.ds.host.is_empty().then_some("Specify the host"),
        };
        if let Some(m) = missing {
            d.test = Some(Err(m.into()));
            self.db.dialog = Some(d);
            cx.notify();
            return;
        }
        if d.ds.name.trim().is_empty() {
            d.ds.name = d.ds.display_name();
        }
        let id = d.ds.id.clone();
        // Password storage: "Forever" keeps it encrypted per user; "Never" only for this session.
        if d.ds.driver != Driver::Sqlite {
            if d.ds.save_password {
                if d.password_dirty || d.editing.is_none() || brisk_db::load_password(&id).is_none() {
                    if let Err(e) = brisk_db::store_password(&id, &d.password) {
                        self.show_toast(format!("Cannot save password: {e}"), true, cx);
                    }
                }
            } else {
                brisk_db::forget_password(&id);
            }
            self.db.session_passwords.insert(id.clone(), d.password.clone());
        }
        let di = match d.editing {
            Some(i) if i < self.db.sources.len() => {
                if self.db.sources[i] != d.ds {
                    self.db.states.remove(&id);
                }
                self.db.sources[i] = d.ds.clone();
                i
            }
            _ => {
                self.db.sources.push(d.ds.clone());
                self.db.sources.len() - 1
            }
        };
        if let Err(e) = self.db.save(&self.root) {
            self.show_toast(e, true, cx);
        }
        if matches!(self.db.states.get(&id), Some(ConnState::Failed(_))) {
            self.db.states.remove(&id);
        }
        self.db.open = true;
        self.db.rebuild();
        self.db.selected = self.db.rows.iter().position(|r| r.kind == RowKind::Source(di)).unwrap_or(0);
        window.focus(&self.db.focus);
        self.db_connect(di, d.after.unwrap_or(After::Nothing), window, cx);
        cx.notify();
    }

    pub(super) fn db_dialog_key(&mut self, e: &KeyDownEvent, window: &mut Window, cx: &mut Context<Self>) {
        let k = &e.keystroke;
        let m = k.modifiers;
        let Some(d) = self.db.dialog.as_mut() else { return };
        let order = fields(d.ds.driver);
        let pos = order.iter().position(|f| *f == d.field).unwrap_or(0);
        match k.key.as_str() {
            "escape" => {
                self.db.dialog = None;
                if self.db.open {
                    window.focus(&self.db.focus);
                } else {
                    self.focus_editor(window, cx);
                }
            }
            "enter" => return self.dialog_ok(window, cx),
            "t" if m.alt => return self.dialog_test(cx),
            "tab" | "down" | "up" => {
                let back = (k.key == "tab" && m.shift) || k.key == "up";
                d.field = order[if back { (pos + order.len() - 1) % order.len() } else { (pos + 1) % order.len() }];
                d.select_all = d.field.is_text();
            }
            "left" | "right" | "space" if !d.field.is_text() => Self::dialog_toggle(d, d.field, k.key == "left"),
            "backspace" => {
                let f = d.field;
                let all = d.select_all || m.control;
                d.select_all = false;
                if let Some(t) = Self::dialog_text(d, f) {
                    if all {
                        t.clear();
                    } else {
                        t.pop();
                    }
                }
                d.test = None;
            }
            "a" if m.control => d.select_all = true,
            "v" if m.control => {
                if let Some(t) = cx.read_from_clipboard().and_then(|c| c.text()) {
                    let t = t.trim().to_string();
                    let looks_url = t.contains("://") || t.starts_with("jdbc:") || t.starts_with("sqlite:");
                    if looks_url && matches!(d.field, Field::Host | Field::Name | Field::File) {
                        if let Some(pw) = d.ds.apply_url(&t) {
                            d.port = if d.ds.port == 0 { String::new() } else { d.ds.port.to_string() };
                            if let Some(pw) = pw {
                                d.password = pw;
                                d.password_dirty = true;
                            }
                            d.note = Some("Filled from the pasted URL".into());
                            if !fields(d.ds.driver).contains(&d.field) {
                                d.field = Field::Name;
                            }
                        }
                    } else {
                        let f = d.field;
                        let all = d.select_all;
                        d.select_all = false;
                        if let Some(s) = Self::dialog_text(d, f) {
                            if all {
                                s.clear();
                            }
                            s.push_str(t.lines().next().unwrap_or(""));
                        }
                    }
                    d.test = None;
                }
            }
            _ => {
                if !(m.control || m.alt || m.platform) {
                    if let Some(ch) = &k.key_char {
                        if !ch.chars().any(|c| c.is_control()) {
                            let f = d.field;
                            let all = d.select_all;
                            d.select_all = false;
                            if f == Field::Port && !ch.chars().all(|c| c.is_ascii_digit()) {
                                return;
                            }
                            if let Some(s) = Self::dialog_text(d, f) {
                                if all {
                                    s.clear();
                                }
                                s.push_str(ch);
                            }
                            d.test = None;
                        }
                    }
                }
            }
        }
        cx.notify();
    }

    // --- Rendering ---------------------------------------------------------------------

    pub(super) fn render_database(&mut self, cx: &mut Context<Self>) -> gpui::AnyElement {
        let n = self.db.rows.len();
        let tool = |id: &'static str, glyph: &'static str, action: Action, cx: &mut Context<Self>| {
            div()
                .id(id)
                .px(px(6.))
                .py(px(2.))
                .rounded(px(4.))
                .text_color(Theme::text_muted())
                .hover(|s| s.bg(Theme::hover_row()).text_color(Theme::text()))
                .cursor_pointer()
                .on_click(cx.listener(move |this, _, w, cx| {
                    window_focus_db(this, w);
                    this.dispatch(action, w, cx)
                }))
                .child(glyph)
        };
        let header = div()
            .h(px(34.))
            .flex()
            .items_center()
            .px(px(12.))
            .gap(px(2.))
            .child(div().text_size(px(13.)).font_weight(gpui::FontWeight::SEMIBOLD).text_color(Theme::text()).child("Database"))
            .child(div().flex_1())
            .child(tool("db-new", "+", Action::NewDataSource, cx))
            .child(tool("db-refresh", "⟳", Action::RefreshDatabase, cx))
            .child(tool("db-props", "⚙", Action::DataSourceProperties, cx))
            .child(tool("db-console", "▤", Action::OpenQueryConsole, cx))
            .child(tool("db-disconnect", "⏻", Action::Disconnect, cx));
        let body: gpui::AnyElement = if n == 0 {
            div()
                .p(px(14.))
                .flex()
                .flex_col()
                .gap(px(8.))
                .text_size(px(13.))
                .text_color(Theme::text_muted())
                .child("No data sources")
                .child(
                    div()
                        .id("db-empty-new")
                        .text_color(Theme::accent())
                        .cursor_pointer()
                        .child("+ New Data Source  (Alt+Insert)")
                        .on_click(cx.listener(|this, _, w, cx| this.dispatch(Action::NewDataSource, w, cx))),
                )
                .child(div().text_size(px(12.)).child("PostgreSQL, MySQL / MariaDB, SQLite. Paste a connection URL into Host to fill the form."))
                .into_any_element()
        } else {
            uniform_list(
                "db-tree",
                n,
                cx.processor(|this, range: std::ops::Range<usize>, window, cx| {
                    let focused = this.db.focus.is_focused(window);
                    range
                        .map(|ix| {
                            let row = this.db.rows[ix].clone();
                            let selected = ix == this.db.selected;
                            let open = this.db.expanded.contains(&row.key);
                            let chevron = if row.expandable { if open { "▾" } else { "▸" } } else { " " };
                            let (icon, icon_color, label, detail, label_color): (String, gpui::Hsla, String, String, gpui::Hsla) = match &row.kind {
                                RowKind::Source(di) => {
                                    let ds = &this.db.sources[*di];
                                    let (b, c) = driver_badge(ds.driver);
                                    let detail = match this.db.states.get(&ds.id) {
                                        Some(ConnState::Live(l)) => l.version.clone(),
                                        Some(ConnState::Connecting) => "connecting…".into(),
                                        Some(ConnState::Failed(_)) => "failed".into(),
                                        None => match ds.driver {
                                            Driver::Sqlite => ds.file.clone(),
                                            _ => format!("{}:{}", ds.host, ds.port),
                                        },
                                    };
                                    let live = matches!(this.db.states.get(&ds.id), Some(ConnState::Live(_)));
                                    (b.into(), c, ds.display_name(), detail, if live { Theme::text() } else { Theme::text_muted() })
                                }
                                RowKind::Schema(di, si) => {
                                    let s = &this.db.live(*di).unwrap().schemas[*si];
                                    ("◇".into(), Theme::text_muted(), s.name.clone(), format!("{}", s.tables.len()), Theme::text())
                                }
                                RowKind::Table(di, si, ti) => {
                                    let t = &this.db.live(*di).unwrap().schemas[*si].tables[*ti];
                                    let (g, c) = if t.kind == TableKind::View { ("◫", 0x9876aa) } else { ("▦", 0x6c9be8) };
                                    (g.into(), gpui::rgb(c).into(), t.name.clone(), String::new(), Theme::text())
                                }
                                RowKind::Column(di, si, ti, ci) => {
                                    let c = &this.db.live(*di).unwrap().schemas[*si].tables[*ti].columns[*ci];
                                    let icon = if c.primary_key { "⚷" } else { "▪" };
                                    let color = if c.primary_key { gpui::rgb(0xe0a85a).into() } else { Theme::text_dim() };
                                    let null = if c.nullable { "" } else { " not null" };
                                    (icon.into(), color, c.name.clone(), format!("{}{null}", c.type_name), Theme::text())
                                }
                                RowKind::Message(_, m, err) => (if *err { "!".into() } else { "…".into() }, Theme::text_muted(), m.clone(), String::new(), if *err { Theme::error() } else { Theme::text_muted() }),
                            };
                            let is_source = matches!(row.kind, RowKind::Source(_));
                            div()
                                .id(("db-row", ix))
                                .h(px(24.))
                                .flex()
                                .items_center()
                                .gap(px(4.))
                                .pl(px(8. + row.depth as f32 * 16.))
                                .pr(px(8.))
                                .text_size(px(13.))
                                .when(selected, |d| d.bg(if focused { Theme::selection_row() } else { Theme::hover_row() }))
                                .hover(|s| s.bg(Theme::hover_row()))
                                .on_click(cx.listener(move |this, e: &ClickEvent, window, cx| {
                                    window.focus(&this.db.focus);
                                    this.db.selected = ix;
                                    if e.click_count() >= 2 {
                                        this.db_open_selected(window, cx);
                                    } else if e.position().x < px(8. + row.depth as f32 * 16. + 14.) {
                                        this.db_toggle(ix, window, cx);
                                    }
                                    cx.notify();
                                }))
                                .child(div().w(px(12.)).text_color(Theme::text_muted()).child(chevron))
                                .child(if is_source {
                                    div()
                                        .w(px(20.))
                                        .h(px(16.))
                                        .rounded(px(4.))
                                        .bg(icon_color)
                                        .text_color(Theme::bg())
                                        .text_size(px(9.))
                                        .font_weight(gpui::FontWeight::BOLD)
                                        .flex()
                                        .items_center()
                                        .justify_center()
                                        .child(icon)
                                } else {
                                    div().w(px(16.)).text_color(icon_color).child(icon)
                                })
                                .child(div().text_color(label_color).whitespace_nowrap().child(label))
                                .child(div().text_color(Theme::text_dim()).text_size(px(12.)).whitespace_nowrap().overflow_hidden().child(detail))
                        })
                        .collect()
                }),
            )
            .track_scroll(self.db.scroll.clone())
            .flex_1()
            .into_any_element()
        };
        div()
            .id("database-panel")
            .key_context("DatabaseView")
            .track_focus(&self.db.focus)
            .flex()
            .flex_col()
            .size_full()
            .child(header)
            .child(body)
            .into_any_element()
    }

    pub(super) fn render_services(&mut self, _cx: &mut Context<Self>) -> gpui::AnyElement {
        match &self.db.results {
            Some(g) => {
                let title = g.read(_cx).title.clone();
                div()
                    .flex()
                    .flex_col()
                    .size_full()
                    .child(Self::panel_header(&format!("Services · {title} · console")))
                    .child(div().flex_1().min_h_0().child(g.clone()))
                    .into_any_element()
            }
            None => Self::render_placeholder("Services", "Query results appear here. Open a console from the Database tool window (Ctrl+Shift+F10) and run a statement with Ctrl+Enter."),
        }
    }

    pub(super) fn render_ds_dialog(&mut self, cx: &mut Context<Self>) -> Option<gpui::AnyElement> {
        let d = self.db.dialog.as_ref()?;
        let title = if d.editing.is_some() { "Data Source Properties" } else { "New Data Source" };
        let field_row = |f: Field, value: gpui::AnyElement, active: bool, cx: &mut Context<Self>| {
            div()
                .id(("ds-field", f as usize))
                .h(px(32.))
                .flex()
                .items_center()
                .gap(px(10.))
                .child(div().w(px(90.)).text_color(Theme::text_muted()).child(f.label()))
                .child(
                    div()
                        .flex_1()
                        .h(px(28.))
                        .px(px(8.))
                        .flex()
                        .items_center()
                        .rounded(px(5.))
                        .border_1()
                        .border_color(if active { Theme::accent() } else { Theme::divider() })
                        .bg(Theme::bg())
                        .child(value),
                )
                .on_click(cx.listener(move |this, _, _, cx| {
                    if let Some(d) = this.db.dialog.as_mut() {
                        if d.field == f && !f.is_text() {
                            Self::dialog_toggle(d, f, false);
                        }
                        d.field = f;
                        d.select_all = false;
                    }
                    cx.notify();
                }))
        };
        let caret = || div().w(px(1.5)).h(px(16.)).bg(Theme::caret());
        let mut rows: Vec<gpui::AnyElement> = Vec::new();
        for &f in fields(d.ds.driver) {
            let active = d.field == f;
            let value: gpui::AnyElement = match f {
                Field::Driver => div()
                    .flex()
                    .gap(px(4.))
                    .children(Driver::ALL.iter().map(|&dr| {
                        let on = dr == d.ds.driver;
                        div().px(px(8.)).rounded(px(4.)).when(on, |x| x.bg(Theme::selection_row())).text_color(if on { Theme::text() } else { Theme::text_muted() }).child(dr.title())
                    }))
                    .into_any_element(),
                Field::Ssl => div()
                    .flex()
                    .gap(px(4.))
                    .children([Ssl::Disable, Ssl::Prefer, Ssl::Require].into_iter().map(|s| {
                        let on = s == d.ds.ssl;
                        div().px(px(8.)).rounded(px(4.)).when(on, |x| x.bg(Theme::selection_row())).text_color(if on { Theme::text() } else { Theme::text_muted() }).child(s.title())
                    }))
                    .into_any_element(),
                Field::SavePassword => div()
                    .flex()
                    .gap(px(4.))
                    .children([(true, "Forever"), (false, "Never")].into_iter().map(|(v, l)| {
                        let on = v == d.ds.save_password;
                        div().px(px(8.)).rounded(px(4.)).when(on, |x| x.bg(Theme::selection_row())).text_color(if on { Theme::text() } else { Theme::text_muted() }).child(l)
                    }))
                    .into_any_element(),
                _ => {
                    let text = match f {
                        Field::Name => d.ds.name.clone(),
                        Field::Host => d.ds.host.clone(),
                        Field::Port => d.port.clone(),
                        Field::User => d.ds.user.clone(),
                        Field::Password => "•".repeat(d.password.chars().count()),
                        Field::Database => d.ds.database.clone(),
                        Field::File => d.ds.file.clone(),
                        _ => String::new(),
                    };
                    let placeholder = match f {
                        Field::Name => d.ds.display_name(),
                        Field::Host => "localhost, or paste a URL".into(),
                        Field::File => "path/to/db.sqlite3 (relative to the project)".into(),
                        Field::Password if !d.password_dirty && d.editing.is_some() && d.ds.save_password => "<saved>".into(),
                        _ => String::new(),
                    };
                    let sel = active && d.select_all && !text.is_empty();
                    if text.is_empty() {
                        div().flex().when(active, |x| x.child(caret())).child(div().text_color(Theme::text_dim()).child(placeholder)).into_any_element()
                    } else {
                        div().flex().items_center().child(div().when(sel, |x| x.bg(Theme::selection())).child(text)).when(active, |x| x.child(caret())).into_any_element()
                    }
                }
            };
            rows.push(field_row(f, value, active, cx).into_any_element());
        }
        let test_line: gpui::AnyElement = match (&d.test, d.testing) {
            (_, true) => div().text_color(Theme::text_muted()).child("Testing connection…").into_any_element(),
            (Some(Ok(v)), _) => div().text_color(Theme::file_added()).child(format!("✓ Succeeded: {v}")).into_any_element(),
            (Some(Err(e)), _) => div().text_color(Theme::error()).child(format!("✗ {}", first_line(e))).into_any_element(),
            (None, _) => div().text_color(Theme::text_dim()).child(d.ds.url()).into_any_element(),
        };
        let button = |id: &'static str, label: &'static str, primary: bool| {
            div()
                .id(id)
                .px(px(14.))
                .h(px(28.))
                .flex()
                .items_center()
                .rounded(px(5.))
                .cursor_pointer()
                .when(primary, |b| b.bg(Theme::accent()).text_color(gpui::white()))
                .when(!primary, |b| b.border_1().border_color(Theme::divider()).hover(|s| s.bg(Theme::hover_row())))
                .child(label)
        };
        Some(
            div()
                .absolute()
                .top_0()
                .left_0()
                .size_full()
                .flex()
                .justify_center()
                .pt(px(80.))
                .child(
                    div()
                        .id("ds-dialog")
                        .w(px(560.))
                        .flex()
                        .flex_col()
                        .gap(px(4.))
                        .p(px(16.))
                        .bg(Theme::panel())
                        .border_1()
                        .border_color(Theme::divider())
                        .rounded(px(10.))
                        .shadow_lg()
                        .text_size(px(13.))
                        .text_color(Theme::text())
                        .child(div().font_weight(gpui::FontWeight::SEMIBOLD).pb(px(6.)).child(title))
                        .when_some(d.note.clone(), |x, n| x.child(div().pb(px(4.)).text_color(Theme::modified()).child(n)))
                        .children(rows)
                        .child(div().pt(px(6.)).text_size(px(12.)).child(test_line))
                        .child(
                            div()
                                .pt(px(10.))
                                .flex()
                                .items_center()
                                .gap(px(8.))
                                .child(
                                    div()
                                        .id("ds-test")
                                        .text_color(Theme::accent())
                                        .cursor_pointer()
                                        .child("Test Connection  Alt+T")
                                        .on_click(cx.listener(|this, _, _, cx| this.dialog_test(cx))),
                                )
                                .child(div().flex_1())
                                .child(button("ds-cancel", "Cancel", false).on_click(cx.listener(|this, _, w, cx| {
                                    this.db.dialog = None;
                                    this.focus_editor(w, cx);
                                    cx.notify();
                                })))
                                .child(button("ds-ok", "OK", true).on_click(cx.listener(|this, _, w, cx| this.dialog_ok(w, cx)))),
                        )
                        .child(div().pt(px(6.)).text_size(px(11.)).text_color(Theme::text_dim()).child("Tab / ↑↓ fields · ←→ or Space switch options · Enter OK · Esc Cancel")),
                )
                .into_any_element(),
        )
    }

    /// Smoke hook: run `sql` in a console of the first data source, print the outcome, open
    /// the first table's data.
    pub(super) fn db_smoke(&mut self, sql: String, window: &mut Window, cx: &mut Context<Self>) {
        if self.db.sources.is_empty() {
            eprintln!("smoke-db: no data sources in {}", brisk_db::CONFIG_FILE);
            return;
        }
        self.db.open = true;
        self.open_console(0, window, cx);
        if let Some(ed) = self.active_editor().cloned() {
            ed.update(cx, |e, cx| e.set_initial_text(&sql, cx));
        }
        self.execute_console(window, cx);
        let bg = cx.background_executor().clone();
        let t0 = Instant::now();
        cx.spawn_in(window, async move |this, cx| {
            for _ in 0..120 {
                bg.timer(std::time::Duration::from_millis(250)).await;
                let done = this.update(cx, |this, cx| this.db.results.as_ref().is_some_and(|g| !g.read(cx).is_running()) && this.db.running.is_none()).unwrap_or(true);
                if done {
                    break;
                }
            }
            let _ = this.update_in(cx, |this, window, cx| {
                let msg = match this.db.results.as_ref().map(|g| &g.read(cx).state) {
                    Some(crate::grid_view::GridState::Done { message, out }) => format!("{message}; columns {:?}", out.columns),
                    Some(crate::grid_view::GridState::Failed(e)) => format!("error: {e}"),
                    _ => "no result".into(),
                };
                let tables: usize = this.db.live(0).map_or(0, |l| l.schemas.iter().map(|s| s.tables.len()).sum());
                eprintln!("smoke-db: {msg} (since start {:.1?}, tables {tables}, tree rows {})", t0.elapsed(), this.db.rows.len());
                // Open the first table's data, as F4 would.
                if let Some(ix) = this.db.rows.iter().position(|r| matches!(r.kind, RowKind::Table(..))) {
                    this.db.selected = ix;
                    this.db_open_selected(window, cx);
                }
            });
        })
        .detach();
    }
}

fn window_focus_db(this: &mut Workspace, w: &mut Window) {
    w.focus(&this.db.focus);
}
