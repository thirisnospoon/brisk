//! Database tool window (right stripe), Data Source dialog, query consoles and result grids,
//! following PyCharm Professional / DataGrip.
//!
//! Connections live for the session, one per data source, opened lazily (on expand, console
//! or query). Every driver call runs on the background executor; a connection is a mutex,
//! so statements on the same data source queue up as in a single JDBC session.
//!
//! [`DbModel`] owns all of it and draws the tool window. `Workspace` holds it in [`Db`],
//! creates it on first use, and turns its [`DbEvent`]s into tabs, toasts, status and focus.

use super::*;
use crate::grid_view::{GridEvent, GridView};
use crate::theme::ui_px;
use brisk_db::{Canceller, Connection, DataSource, Driver, Schema, Ssl, TableKind};
use gpui::{EventEmitter, WeakEntity};
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
    /// The attempt's token: a connect that finishes after Disconnect, Delete or a newer
    /// attempt finds another state (or token) and is dropped.
    Connecting(u64),
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

#[derive(Clone)]
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

/// The Database tool window as `Workspace` holds it: where it is (open, focus) and its model,
/// which is created, and the data sources read, the first time something uses it.
pub struct Db {
    pub open: bool,
    pub focus: FocusHandle,
    model: Option<Entity<DbModel>>,
}

impl Db {
    pub fn new(cx: &mut Context<Workspace>) -> Self {
        Self { open: false, focus: cx.focus_handle(), model: None }
    }

    /// Whether the Data Source dialog is open.
    pub fn dialog_open(&self, cx: &App) -> bool {
        self.model.as_ref().is_some_and(|m| m.read(cx).dialog.is_some())
    }

    /// The canceller of the statement running now (Ctrl+F2 / ■).
    pub fn running(&self, cx: &App) -> Option<Canceller> {
        self.model.as_ref().and_then(|m| m.read(cx).running.clone())
    }

    /// The grid with the output of console statements, shown in Services (Alt+8).
    pub fn results(&self, cx: &App) -> Option<Entity<GridView>> {
        self.model.as_ref().and_then(|m| m.read(cx).results.clone())
    }
}

/// What the database model asks of the workspace.
pub enum DbEvent {
    Toast(String, bool),
    Status(String),
    /// A query console, opened from the tree.
    OpenFile(PathBuf),
    /// A table's data: its tab if one is open, else a new one; focused.
    ShowGrid(Entity<GridView>),
    /// The console output, in Services.
    ShowResults,
    /// Open the tool window and focus it.
    ShowWindow,
    /// The Data Source dialog opened: its keys go through the workspace.
    FocusKeys,
    /// Esc closed the dialog.
    DialogClosed,
    /// Cancel closed the dialog.
    FocusEditor,
    /// A header button of the tool window.
    Dispatch(Action),
}

/// The data sources, their connections and the tree of the Database tool window, the Data
/// Source dialog, the console's result grid and the table grids it opened.
pub struct DbModel {
    root: PathBuf,
    focus: FocusHandle,
    sources: Vec<DataSource>,
    states: HashMap<String, ConnState>,
    session_passwords: HashMap<String, String>,
    expanded: HashSet<String>,
    rows: Vec<Row>,
    selected: usize,
    scroll: UniformListScrollHandle,
    dialog: Option<Dialog>,
    /// Output of console statements, shown in Services (Alt+8).
    results: Option<Entity<GridView>>,
    /// The tables' data grids, to bring back a tab instead of opening a second one.
    tables: Vec<WeakEntity<GridView>>,
    /// The canceller of the statement running now (Ctrl+F2 / ■).
    running: Option<Canceller>,
    pending: HashMap<String, Vec<After>>,
    delete_armed: Option<String>,
    last_used: Option<String>,
    /// The last connect attempt's token.
    connect_seq: u64,
}

impl EventEmitter<DbEvent> for DbModel {}

impl DbModel {
    /// The model with the project's data sources, and whether they were just imported from
    /// `.idea/dataSources.xml`. `focus` is the tool window's.
    pub fn new(root: PathBuf, focus: FocusHandle) -> (Self, bool) {
        let (sources, imported) = brisk_db::load_sources(&root);
        let mut db = Self {
            root,
            focus,
            sources,
            states: HashMap::new(),
            session_passwords: HashMap::new(),
            expanded: HashSet::new(),
            rows: Vec::new(),
            selected: 0,
            scroll: UniformListScrollHandle::new(),
            dialog: None,
            results: None,
            tables: Vec::new(),
            running: None,
            pending: HashMap::new(),
            delete_armed: None,
            last_used: None,
            connect_seq: 0,
        };
        db.rebuild();
        (db, imported)
    }

    pub fn source_count(&self) -> usize {
        self.sources.len()
    }

    pub fn driver(&self, di: usize) -> Option<Driver> {
        self.sources.get(di).map(|s| s.driver)
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
                Some(ConnState::Connecting(_)) => {
                    rows.push(Row { depth: 1, kind: RowKind::Message(di, "Connecting…".into(), false), key: String::new(), expandable: false })
                }
                Some(ConnState::Failed(e)) => {
                    rows.push(Row { depth: 1, kind: RowKind::Message(di, first_line(e), true), key: String::new(), expandable: false })
                }
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

    pub fn selected_source(&self) -> Option<usize> {
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

    fn save(&self) -> Result<(), String> {
        brisk_db::save_sources(&self.root, &self.sources).map_err(|e| format!("Cannot save {}: {e}", brisk_db::CONFIG_FILE))
    }

    fn toast(&mut self, msg: String, error: bool, cx: &mut Context<Self>) {
        cx.emit(DbEvent::Toast(msg, error));
    }

    fn status(&mut self, msg: String, cx: &mut Context<Self>) {
        cx.emit(DbEvent::Status(msg));
    }

    // --- Connections ------------------------------------------------------------------

    fn password_for(&self, ds: &DataSource) -> Option<String> {
        if ds.driver == Driver::Sqlite {
            return Some(String::new());
        }
        if let Some(p) = self.session_passwords.get(&ds.id) {
            return Some(p.clone());
        }
        if ds.save_password {
            return Some(brisk_db::load_password(&ds.id).unwrap_or_default());
        }
        None
    }

    /// Connect if needed, then do `after`.
    pub fn connect(&mut self, di: usize, after: After, cx: &mut Context<Self>) {
        let Some(ds) = self.sources.get(di).cloned() else { return };
        match self.states.get(&ds.id) {
            Some(ConnState::Live(_)) => return self.after(di, after, cx),
            Some(ConnState::Connecting(_)) => {
                self.pending.entry(ds.id.clone()).or_default().push(after);
                return;
            }
            _ => {}
        }
        let Some(password) = self.password_for(&ds) else {
            // "Save: Never" and nothing entered this session: ask, like PyCharm's password prompt.
            self.open_dialog(Some(di), Some("Enter the password to connect".into()), Some(after), cx);
            return;
        };
        self.pending.entry(ds.id.clone()).or_default().push(after);
        self.connect_seq += 1;
        let token = self.connect_seq;
        self.states.insert(ds.id.clone(), ConnState::Connecting(token));
        self.expanded.insert(format!("ds:{}", ds.id));
        self.rebuild();
        let root = self.root.clone();
        let id = ds.id.clone();
        let task = cx.background_executor().spawn(async move {
            let mut c = brisk_db::connect(&ds, &password, &root)?;
            let version = c.server_version();
            let schemas = brisk_db::introspect(c.as_mut())?;
            Ok::<_, String>((c, version, schemas))
        });
        cx.spawn(async move |this, cx| {
            let res = task.await;
            let _ = this.update(cx, |this, cx| {
                match this.states.get(&id) {
                    Some(ConnState::Connecting(t)) if *t == token => {}
                    // A newer attempt is under way: the waiting work is its.
                    Some(ConnState::Connecting(_)) => return,
                    // Disconnected or deleted meanwhile: the connection (if any) is dropped
                    // here, which closes it, and the work that waited for it is told.
                    _ => {
                        for a in this.pending.remove(&id).unwrap_or_default() {
                            if let After::Run { grid, .. } = a {
                                grid.update(cx, |g, cx| g.set_error("Disconnected before the connection was made".into(), cx));
                            }
                        }
                        return;
                    }
                }
                let afters = this.pending.remove(&id).unwrap_or_default();
                match res {
                    Ok((c, version, schemas)) => {
                        let cancel = c.canceller();
                        // Open the default schema: public / main / the database itself.
                        let ds = this.sources.iter().find(|s| s.id == id).cloned();
                        if let Some(ds) = &ds {
                            let def = schemas
                                .iter()
                                .find(|s| s.name == "public" || s.name == "main" || s.name == ds.database)
                                .or_else(|| (schemas.len() == 1).then(|| &schemas[0]));
                            if let Some(s) = def {
                                this.expanded.insert(format!("ds:{}/s:{}", ds.id, s.name));
                            }
                        }
                        let n: usize = schemas.iter().map(|s| s.tables.len()).sum();
                        this.status(format!("Connected: {version}, {n} tables"), cx);
                        this.states.insert(id.clone(), ConnState::Live(Live { conn: Arc::new(Mutex::new(c)), cancel, schemas, version }));
                        this.rebuild();
                        if let Some(di) = this.sources.iter().position(|s| s.id == id) {
                            for a in afters {
                                this.after(di, a, cx);
                            }
                        }
                    }
                    Err(e) => {
                        this.toast(format!("Connection failed: {}", first_line(&e)), true, cx);
                        for a in afters {
                            if let After::Run { grid, .. } = a {
                                grid.update(cx, |g, cx| g.set_error(e.clone(), cx));
                            }
                        }
                        this.states.insert(id.clone(), ConnState::Failed(e));
                        this.rebuild();
                    }
                }
                cx.notify();
            });
        })
        .detach();
        cx.notify();
    }

    fn after(&mut self, di: usize, after: After, cx: &mut Context<Self>) {
        match after {
            After::Nothing => {}
            After::Run { grid, sql } => self.run(di, grid, sql, cx),
        }
    }

    /// Run a script on a data source and show the output in `grid`.
    fn run(&mut self, di: usize, grid: Entity<GridView>, sql: String, cx: &mut Context<Self>) {
        let Some(live) = self.live(di) else { return };
        let conn = live.conn.clone();
        self.running = Some(live.cancel.clone());
        self.last_used = Some(self.sources[di].id.clone());
        let id = self.sources[di].id.clone();
        grid.update(cx, |g, cx| g.set_running(&sql, cx));
        let script = sql.clone();
        let task = cx.background_executor().spawn(async move {
            let mut c = conn.lock().unwrap_or_else(|p| p.into_inner());
            let r = brisk_db::execute(c.as_mut(), &script, brisk_db::DEFAULT_LIMIT);
            let ddl = brisk_db::split_statements(&script, c.driver()).into_iter().any(|r| brisk_db::is_ddl(&script[r]));
            let schemas = if ddl { brisk_db::introspect(c.as_mut()).ok() } else { None };
            (r, schemas)
        });
        cx.spawn(async move |this, cx| {
            let ((outs, err), schemas) = task.await;
            let _ = this.update(cx, |this, cx| {
                this.running = None;
                if let Some(s) = schemas {
                    if let Some(ConnState::Live(l)) = this.states.get_mut(&id) {
                        l.schemas = s;
                    }
                    this.rebuild();
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

    pub fn disconnect(&mut self, di: usize, cx: &mut Context<Self>) {
        let Some(ds) = self.sources.get(di).cloned() else { return };
        if self.states.remove(&ds.id).is_some() {
            self.status(format!("Disconnected from {}", ds.display_name()), cx);
        }
        self.expanded.retain(|k| !k.starts_with(&format!("ds:{}", ds.id)));
        self.rebuild();
        cx.notify();
    }

    pub fn refresh(&mut self, di: usize, cx: &mut Context<Self>) {
        let Some(live) = self.live(di) else {
            self.states.remove(&self.sources[di].id);
            return self.connect(di, After::Nothing, cx);
        };
        let conn = live.conn.clone();
        let id = self.sources[di].id.clone();
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
                        if let Some(ConnState::Live(l)) = this.states.get_mut(&id) {
                            l.schemas = s;
                        }
                        this.status(format!("Database refreshed in {} ms", t0.elapsed().as_millis()), cx);
                    }
                    Err(e) => this.toast(format!("Refresh failed: {}", first_line(&e)), true, cx),
                }
                this.rebuild();
                cx.notify();
            });
        })
        .detach();
    }

    /// Run a grid's statement again (Refresh in a data grid).
    pub fn reload(&mut self, grid: Entity<GridView>, cx: &mut Context<Self>) {
        let Some((id, sql)) = grid.read(cx).source.clone() else { return };
        if let Some(di) = self.sources.iter().position(|s| s.id == id) {
            self.connect(di, After::Run { grid, sql }, cx);
        }
    }

    fn new_grid(&mut self, title: String, cx: &mut Context<Self>) -> Entity<GridView> {
        let g = cx.new(|cx| GridView::new(title, cx));
        cx.subscribe(&g, |this: &mut Self, g, ev: &GridEvent, cx| match ev {
            GridEvent::Cancel => {
                if let Some(c) = &this.running {
                    c.cancel();
                }
            }
            GridEvent::Reload => {
                let Some((id, sql)) = g.read(cx).source.clone() else { return };
                if let Some(di) = this.sources.iter().position(|s| s.id == id) {
                    if this.live(di).is_some() {
                        this.run(di, g, sql, cx);
                    }
                }
            }
        })
        .detach();
        g
    }

    /// Run `sql` from a console of data source `di`; the output goes to Services.
    pub fn run_console(&mut self, di: usize, sql: String, cx: &mut Context<Self>) {
        let grid = match &self.results {
            Some(g) => g.clone(),
            None => {
                let g = self.new_grid("Output".into(), cx);
                self.results = Some(g.clone());
                g
            }
        };
        let id = self.sources[di].id.clone();
        let title = self.sources[di].display_name();
        grid.update(cx, |g, _| {
            g.title = title;
            g.source = Some((id, sql.clone()));
        });
        cx.emit(DbEvent::ShowResults);
        self.connect(di, After::Run { grid, sql }, cx);
    }

    /// The console file of data source `di`, created if missing; it connects in the
    /// background, like PyCharm does when a console opens. `None` (with a toast) when the
    /// file cannot be created.
    pub fn console(&mut self, di: usize, cx: &mut Context<Self>) -> Option<PathBuf> {
        let ds = self.sources.get(di)?.clone();
        let path = console_path(&ds);
        if let Some(d) = path.parent() {
            let _ = std::fs::create_dir_all(d);
        }
        if !path.exists() {
            if let Err(e) = std::fs::write(&path, "") {
                self.toast(format!("Cannot create console: {e}"), true, cx);
                return None;
            }
        }
        self.last_used = Some(ds.id.clone());
        self.connect(di, After::Nothing, cx);
        Some(path)
    }

    /// F4 / double-click: tables and views open their data; data sources open a console;
    /// the rest expand.
    pub fn open_selected(&mut self, cx: &mut Context<Self>) {
        let Some(row) = self.rows.get(self.selected).cloned() else { return };
        let (di, si, ti) = match row.kind {
            RowKind::Table(d, s, t) | RowKind::Column(d, s, t, _) => (d, s, t),
            RowKind::Source(d) => {
                if let Some(path) = self.console(d, cx) {
                    cx.emit(DbEvent::OpenFile(path));
                }
                return;
            }
            _ => return self.toggle(self.selected, cx),
        };
        let Some(live) = self.live(di) else { return };
        let (Some(schema), driver) = (live.schemas.get(si), self.sources[di].driver) else { return };
        let Some(table) = schema.tables.get(ti) else { return };
        let sql = brisk_db::select_all(driver, &schema.name, &table.name);
        let source = (self.sources[di].id.clone(), sql.clone());
        let title = table.name.clone();
        // Bring back the tab of the same table.
        self.tables.retain(|g| g.upgrade().is_some());
        if let Some(g) = self.tables.iter().filter_map(|g| g.upgrade()).find(|g| g.read(cx).source.as_ref() == Some(&source)) {
            cx.emit(DbEvent::ShowGrid(g));
            return;
        }
        let g = self.new_grid(title, cx);
        g.update(cx, |g, _| g.source = Some(source));
        self.tables.push(g.downgrade());
        cx.emit(DbEvent::ShowGrid(g.clone()));
        self.connect(di, After::Run { grid: g, sql }, cx);
    }

    fn toggle(&mut self, ix: usize, cx: &mut Context<Self>) {
        let Some(row) = self.rows.get(ix).cloned() else { return };
        if !row.expandable {
            return;
        }
        if self.expanded.contains(&row.key) {
            self.expanded.remove(&row.key);
        } else {
            self.expanded.insert(row.key.clone());
            if let RowKind::Source(di) = row.kind {
                if !matches!(self.states.get(&self.sources[di].id), Some(ConnState::Live(_) | ConnState::Connecting(_))) {
                    self.states.remove(&self.sources[di].id);
                    self.connect(di, After::Nothing, cx);
                }
            }
        }
        self.rebuild();
        cx.notify();
    }

    /// Keys in the Database tree that the keymap doesn't bind.
    pub fn tree_key(&mut self, k: &gpui::Keystroke, cx: &mut Context<Self>) -> bool {
        let n = self.rows.len();
        if k.key != "delete" {
            self.delete_armed = None;
        }
        match k.key.as_str() {
            "up" => self.selected = self.selected.saturating_sub(1),
            "down" => self.selected = (self.selected + 1).min(n.saturating_sub(1)),
            "pageup" => self.selected = self.selected.saturating_sub(15),
            "pagedown" => self.selected = (self.selected + 15).min(n.saturating_sub(1)),
            "home" => self.selected = 0,
            "end" => self.selected = n.saturating_sub(1),
            "right" => {
                let Some(r) = self.rows.get(self.selected).cloned() else { return true };
                if r.expandable && !self.expanded.contains(&r.key) {
                    self.toggle(self.selected, cx);
                } else if self.rows.get(self.selected + 1).is_some_and(|x| x.depth > r.depth) {
                    self.selected += 1;
                }
            }
            "left" => {
                let Some(r) = self.rows.get(self.selected).cloned() else { return true };
                if r.expandable && self.expanded.contains(&r.key) {
                    self.toggle(self.selected, cx);
                } else if let Some(p) = (0..self.selected).rev().find(|&i| self.rows[i].depth < r.depth) {
                    self.selected = p;
                }
            }
            "enter" => match self.rows.get(self.selected).map(|r| r.kind.clone()) {
                Some(RowKind::Table(..) | RowKind::Column(..)) => self.open_selected(cx),
                _ => self.toggle(self.selected, cx),
            },
            "delete" => {
                let Some(di) = self.selected_source() else { return true };
                if !matches!(self.rows.get(self.selected).map(|r| &r.kind), Some(RowKind::Source(_))) {
                    return true;
                }
                let id = self.sources[di].id.clone();
                if self.delete_armed.as_deref() == Some(id.as_str()) {
                    self.delete_armed = None;
                    self.disconnect(di, cx);
                    let ds = self.sources.remove(di);
                    brisk_db::forget_password(&ds.id);
                    if let Err(e) = self.save() {
                        self.toast(e, true, cx);
                    }
                    self.rebuild();
                    self.status(format!("Removed data source {}", ds.display_name()), cx);
                } else {
                    let name = self.sources[di].display_name();
                    self.delete_armed = Some(id);
                    self.toast(format!("Press Delete again to remove data source '{name}'"), false, cx);
                }
            }
            _ => return false,
        }
        self.scroll.scroll_to_item(self.selected, gpui::ScrollStrategy::Top);
        cx.notify();
        true
    }

    // --- Data Source dialog --------------------------------------------------------------

    pub fn open_dialog(&mut self, editing: Option<usize>, note: Option<String>, after: Option<After>, cx: &mut Context<Self>) {
        let ds = editing.and_then(|i| self.sources.get(i).cloned()).unwrap_or_else(|| DataSource::new(Driver::Postgres));
        let password = if editing.is_some() { self.password_for(&ds).unwrap_or_default() } else { String::new() };
        let field = if note.is_some() {
            Field::Password
        } else if editing.is_some() {
            Field::Name
        } else {
            Field::Host
        };
        self.dialog = Some(Dialog {
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
        cx.emit(DbEvent::FocusKeys);
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
        let Some(d) = self.dialog.as_mut() else { return };
        Self::dialog_commit_fields(d);
        d.testing = true;
        d.test = None;
        let (ds, pw, root) = (d.ds.clone(), d.password.clone(), self.root.clone());
        let task = cx.background_executor().spawn(async move { brisk_db::test_connection(&ds, &pw, &root) });
        cx.spawn(async move |this, cx| {
            let r = task.await;
            let _ = this.update(cx, |this, cx| {
                if let Some(d) = this.dialog.as_mut() {
                    d.testing = false;
                    d.test = Some(r);
                }
                cx.notify();
            });
        })
        .detach();
        cx.notify();
    }

    fn dialog_ok(&mut self, cx: &mut Context<Self>) {
        let Some(mut d) = self.dialog.take() else { return };
        Self::dialog_commit_fields(&mut d);
        let missing = match d.ds.driver {
            Driver::Sqlite => d.ds.file.is_empty().then_some("Specify the database file"),
            _ => d.ds.host.is_empty().then_some("Specify the host"),
        };
        if let Some(m) = missing {
            d.test = Some(Err(m.into()));
            self.dialog = Some(d);
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
                        self.toast(format!("Cannot save password: {e}"), true, cx);
                    }
                }
            } else {
                brisk_db::forget_password(&id);
            }
            self.session_passwords.insert(id.clone(), d.password.clone());
        }
        let di = match d.editing {
            Some(i) if i < self.sources.len() => {
                if self.sources[i] != d.ds {
                    self.states.remove(&id);
                }
                self.sources[i] = d.ds.clone();
                i
            }
            _ => {
                self.sources.push(d.ds.clone());
                self.sources.len() - 1
            }
        };
        if let Err(e) = self.save() {
            self.toast(e, true, cx);
        }
        if matches!(self.states.get(&id), Some(ConnState::Failed(_))) {
            self.states.remove(&id);
        }
        self.rebuild();
        self.selected = self.rows.iter().position(|r| r.kind == RowKind::Source(di)).unwrap_or(0);
        cx.emit(DbEvent::ShowWindow);
        self.connect(di, d.after.unwrap_or(After::Nothing), cx);
        cx.notify();
    }

    pub fn dialog_key(&mut self, e: &KeyDownEvent, cx: &mut Context<Self>) {
        let k = &e.keystroke;
        let m = k.modifiers;
        let Some(d) = self.dialog.as_mut() else { return };
        let order = fields(d.ds.driver);
        let pos = order.iter().position(|f| *f == d.field).unwrap_or(0);
        match k.key.as_str() {
            "escape" => {
                self.dialog = None;
                cx.emit(DbEvent::DialogClosed);
            }
            "enter" => return self.dialog_ok(cx),
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
                    if let Some(ch) = crate::keys::typed_text(k) {
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

    /// What a tree row shows.
    fn row_line(&self, ix: usize, row: &Row) -> RowLine {
        let (icon, icon_color, label, detail, label_color): (String, gpui::Hsla, String, String, gpui::Hsla) = match &row.kind {
            RowKind::Source(di) => {
                let ds = &self.sources[*di];
                let (b, c) = driver_badge(ds.driver);
                let detail = match self.states.get(&ds.id) {
                    Some(ConnState::Live(l)) => l.version.clone(),
                    Some(ConnState::Connecting(_)) => "connecting…".into(),
                    Some(ConnState::Failed(_)) => "failed".into(),
                    None => match ds.driver {
                        Driver::Sqlite => ds.file.clone(),
                        _ => format!("{}:{}", ds.host, ds.port),
                    },
                };
                let live = matches!(self.states.get(&ds.id), Some(ConnState::Live(_)));
                (b.into(), c, ds.display_name(), detail, if live { Theme::text() } else { Theme::text_muted() })
            }
            // Schema, table and column rows exist only while their data source is live.
            RowKind::Schema(di, si) => match self.live(*di).and_then(|l| l.schemas.get(*si)) {
                Some(s) => ("◇".into(), Theme::text_muted(), s.name.clone(), format!("{}", s.tables.len()), Theme::text()),
                None => (String::new(), Theme::text_muted(), String::new(), String::new(), Theme::text()),
            },
            RowKind::Table(di, si, ti) => match self.live(*di).and_then(|l| l.schemas.get(*si)?.tables.get(*ti)) {
                Some(t) => {
                    let (g, c) = if t.kind == TableKind::View { ("◫", 0x9876aa) } else { ("▦", 0x6c9be8) };
                    (g.into(), gpui::rgb(c).into(), t.name.clone(), String::new(), Theme::text())
                }
                None => (String::new(), Theme::text_muted(), String::new(), String::new(), Theme::text()),
            },
            RowKind::Column(di, si, ti, ci) => match self.live(*di).and_then(|l| l.schemas.get(*si)?.tables.get(*ti)?.columns.get(*ci)) {
                Some(c) => {
                    let icon = if c.primary_key { "⚷" } else { "▪" };
                    let color = if c.primary_key { gpui::rgb(0xe0a85a).into() } else { Theme::text_dim() };
                    let null = if c.nullable { "" } else { " not null" };
                    (icon.into(), color, c.name.clone(), format!("{}{null}", c.type_name), Theme::text())
                }
                None => (String::new(), Theme::text_muted(), String::new(), String::new(), Theme::text()),
            },
            RowKind::Message(_, m, err) => (
                if *err { "!".into() } else { "…".into() },
                Theme::text_muted(),
                m.clone(),
                String::new(),
                if *err { Theme::error() } else { Theme::text_muted() },
            ),
        };
        RowLine { ix, row: row.clone(), selected: ix == self.selected, open: self.expanded.contains(&row.key), icon, icon_color, label, detail, label_color }
    }
}

/// A tree row as it is drawn.
struct RowLine {
    ix: usize,
    row: Row,
    selected: bool,
    open: bool,
    icon: String,
    icon_color: gpui::Hsla,
    label: String,
    detail: String,
    label_color: gpui::Hsla,
}

/// The Database tool window.
impl Render for DbModel {
    fn render(&mut self, _window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let n = self.rows.len();
        // The header buttons run the workspace's actions, with the tool window focused.
        let tool = |id: &'static str, glyph: &'static str, action: Action, cx: &mut Context<Self>| {
            crate::ui_kit::header_action(id)
                .on_click(cx.listener(move |this, _, w, cx| {
                    w.focus(&this.focus);
                    cx.emit(DbEvent::Dispatch(action));
                }))
                .child(crate::icons::glyph(glyph, 14., Theme::text_muted()))
        };
        let header = crate::ui_kit::tool_header()
            .gap(px(2.))
            .child(crate::ui_kit::tool_title("Database"))
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
                .text_size(ui_px(13.))
                .text_color(Theme::text_muted())
                .child("No data sources")
                .child(
                    div()
                        .id("db-empty-new")
                        .text_color(Theme::accent())
                        .cursor_pointer()
                        .child("+ New Data Source  (Alt+Insert)")
                        .on_click(cx.listener(|_, _, _, cx| cx.emit(DbEvent::Dispatch(Action::NewDataSource)))),
                )
                .child(div().text_size(ui_px(12.)).child("PostgreSQL, MySQL / MariaDB, SQLite. Paste a connection URL into Host to fill the form."))
                .into_any_element()
        } else {
            uniform_list(
                "db-tree",
                n,
                cx.processor(|this, range: std::ops::Range<usize>, window, cx| {
                    let focused = this.focus.is_focused(window);
                    let lines: Vec<RowLine> = range.filter_map(|ix| this.rows.get(ix).map(|row| this.row_line(ix, row))).collect();
                    lines
                        .into_iter()
                        .map(|RowLine { ix, row, selected, open, icon, icon_color, label, detail, label_color }| {
                            // A click on the arrow folds the node; the row's own click only selects.
                            let chevron = div()
                                .id(("db-fold", ix))
                                .relative()
                                .child(crate::icons::tree_arrow(row.expandable, open, Theme::text_muted()))
                                .when(ix == 0, |d| d.child(track_fold()))
                                .on_click(cx.listener(move |this, _: &ClickEvent, window, cx| {
                                    cx.stop_propagation();
                                    window.focus(&this.focus);
                                    this.selected = ix;
                                    this.toggle(ix, cx);
                                    cx.notify();
                                }));
                            let is_source = matches!(row.kind, RowKind::Source(_));
                            div()
                                .id(("db-row", ix))
                                .h(ui_px(24.))
                                .flex()
                                .items_center()
                                .gap(px(4.))
                                .pl(px(8. + row.depth as f32 * 16.))
                                .pr(px(8.))
                                .text_size(ui_px(13.))
                                .w_full()
                                .when(selected, |d| d.bg(if focused { Theme::selection_row() } else { Theme::hover_row() }))
                                .when(!selected, |d| d.hover(|s| s.bg(Theme::hover_row())))
                                .on_click(cx.listener(move |this, e: &ClickEvent, window, cx| {
                                    window.focus(&this.focus);
                                    this.selected = ix;
                                    if e.click_count() >= 2 {
                                        this.open_selected(cx);
                                    }
                                    cx.notify();
                                }))
                                .child(chevron)
                                .child(if is_source {
                                    div()
                                        .w(px(20.))
                                        .h(ui_px(16.))
                                        .rounded(px(4.))
                                        .bg(icon_color)
                                        .text_color(Theme::bg())
                                        .text_size(ui_px(9.))
                                        .font_weight(gpui::FontWeight::BOLD)
                                        .flex()
                                        .items_center()
                                        .justify_center()
                                        .child(icon)
                                } else {
                                    div().w(px(16.)).text_color(icon_color).child(icon)
                                })
                                .child(div().text_color(label_color).whitespace_nowrap().child(label))
                                .child(div().text_color(Theme::text_dim()).text_size(ui_px(12.)).whitespace_nowrap().overflow_hidden().child(detail))
                        })
                        .collect()
                }),
            )
            .track_scroll(self.scroll.clone())
            .flex_1()
            .into_any_element()
        };
        div().id("database-panel").key_context("DatabaseView").track_focus(&self.focus).flex().flex_col().size_full().child(header).child(body)
    }
}

impl DbModel {
    /// The Data Source dialog, drawn by the workspace over everything.
    pub fn render_dialog(&mut self, cx: &mut Context<Self>) -> Option<gpui::AnyElement> {
        let d = self.dialog.as_ref()?;
        let title = if d.editing.is_some() { "Data Source Properties" } else { "New Data Source" };
        let field_row = |f: Field, value: gpui::AnyElement, active: bool, cx: &mut Context<Self>| {
            div()
                .id(("ds-field", f as usize))
                .h(ui_px(32.))
                .flex()
                .items_center()
                .gap(px(10.))
                .child(div().w(px(90.)).text_color(Theme::text_muted()).child(f.label()))
                .child(
                    crate::ui_kit::input_frame(active, false, crate::ui_kit::CONTROL_H)
                        .flex_1()
                        .flex_shrink()
                        .px(px(8.))
                        .cursor(if f.is_text() { gpui::CursorStyle::IBeam } else { gpui::CursorStyle::Arrow })
                        .child(value)
                        .when(active, |d| d.child(crate::ui_kit::focus_ring(crate::ui_kit::RADIUS, 0.))),
                )
                .on_click(cx.listener(move |this, _, _, cx| {
                    if let Some(d) = this.dialog.as_mut() {
                        if d.field == f && !f.is_text() {
                            Self::dialog_toggle(d, f, false);
                        }
                        d.field = f;
                        d.select_all = false;
                    }
                    cx.notify();
                }))
        };
        let caret = || div().w(px(1.5)).h(ui_px(16.)).bg(Theme::caret());
        // One option of a choice field: a button that picks this value.
        fn choice(
            id: (&'static str, usize),
            on: bool,
            label: &'static str,
            cx: &mut Context<DbModel>,
            pick: impl Fn(&mut Dialog) + 'static,
        ) -> gpui::Stateful<gpui::Div> {
            div()
                .id(id)
                .px(px(8.))
                .rounded(px(4.))
                .cursor_pointer()
                .when(on, |x| x.bg(Theme::selection_row()))
                .when(!on, |x| x.hover(|s| s.bg(Theme::hover_row())))
                .text_color(if on { Theme::text() } else { Theme::text_muted() })
                .child(label)
                .on_click(cx.listener(move |this, _, _, cx| {
                    cx.stop_propagation();
                    if let Some(d) = this.dialog.as_mut() {
                        pick(d);
                        d.test = None;
                    }
                    cx.notify();
                }))
        }
        let mut rows: Vec<gpui::AnyElement> = Vec::new();
        for &f in fields(d.ds.driver) {
            let active = d.field == f;
            let value: gpui::AnyElement = match f {
                Field::Driver => div()
                    .flex()
                    .gap(px(4.))
                    .children(Driver::ALL.iter().enumerate().map(|(i, &dr)| {
                        choice(("ds-driver", i), dr == d.ds.driver, dr.title(), cx, move |d| {
                            d.ds.set_driver(dr);
                            d.port = if d.ds.port == 0 { String::new() } else { d.ds.port.to_string() };
                        })
                    }))
                    .into_any_element(),
                Field::Ssl => div()
                    .flex()
                    .gap(px(4.))
                    .children(
                        [Ssl::Disable, Ssl::Prefer, Ssl::Require]
                            .into_iter()
                            .enumerate()
                            .map(|(i, s)| choice(("ds-ssl", i), s == d.ds.ssl, s.title(), cx, move |d| d.ds.ssl = s)),
                    )
                    .into_any_element(),
                Field::SavePassword => div()
                    .flex()
                    .gap(px(4.))
                    .children(
                        [(true, "Forever"), (false, "Never")]
                            .into_iter()
                            .enumerate()
                            .map(|(i, (v, l))| choice(("ds-savepw", i), v == d.ds.save_password, l, cx, move |d| d.ds.save_password = v)),
                    )
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
                        div()
                            .flex()
                            .items_center()
                            .child(div().when(sel, |x| x.bg(Theme::selection())).child(text))
                            .when(active, |x| x.child(caret()))
                            .into_any_element()
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
                .h(ui_px(28.))
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
                .items_start()
                .pt(px(80.))
                // A modal dialog with a form: clicks outside do nothing (no lost input), Esc closes it.
                .occlude()
                .child(
                    div()
                        .id("ds-dialog")
                        .w(px(560.))
                        .max_w(gpui::relative(0.95))
                        .max_h(gpui::relative(0.92))
                        .flex()
                        .flex_col()
                        .gap(px(4.))
                        .p(px(16.))
                        .bg(Theme::panel())
                        .border_1()
                        .border_color(Theme::divider())
                        .rounded(px(10.))
                        .map(crate::ui_kit::floating)
                        .text_size(ui_px(13.))
                        .text_color(Theme::text())
                        .child(div().font_weight(gpui::FontWeight::SEMIBOLD).pb(px(6.)).child(title))
                        .when_some(d.note.clone(), |x, n| x.child(div().pb(px(4.)).text_color(Theme::modified()).child(n)))
                        .children(rows)
                        .child(div().pt(px(6.)).text_size(ui_px(12.)).child(test_line))
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
                                .child(button("ds-cancel", "Cancel", false).on_click(cx.listener(|this, _, _, cx| {
                                    this.dialog = None;
                                    cx.emit(DbEvent::FocusEditor);
                                    cx.notify();
                                })))
                                .child(button("ds-ok", "OK", true).on_click(cx.listener(|this, _, _, cx| this.dialog_ok(cx)))),
                        )
                        .child(
                            div()
                                .pt(px(6.))
                                .text_size(ui_px(11.))
                                .text_color(Theme::text_dim())
                                .child("Tab / ↑↓ fields · ←→ or Space switch options · Enter OK · Esc Cancel"),
                        ),
                )
                .into_any_element(),
        )
    }
}

impl Workspace {
    /// The database model, created (and the project's data sources read) on first use.
    pub(super) fn db_model(&mut self, window: &mut Window, cx: &mut Context<Self>) -> Entity<DbModel> {
        if let Some(m) = &self.db.model {
            return m.clone();
        }
        let (model, imported) = DbModel::new(self.root.clone(), self.db.focus.clone());
        let n = model.source_count();
        let m = cx.new(|_| model);
        // The dialog and the Services results are drawn by the workspace.
        cx.observe(&m, |_, _, cx| cx.notify()).detach();
        cx.subscribe_in(&m, window, Self::on_db_event).detach();
        self.db.model = Some(m.clone());
        if imported {
            self.show_toast(format!("Imported {n} data source(s) from .idea/dataSources.xml"), false, cx);
        }
        m
    }

    fn on_db_event(&mut self, _: &Entity<DbModel>, ev: &DbEvent, window: &mut Window, cx: &mut Context<Self>) {
        match ev {
            DbEvent::Toast(msg, error) => self.show_toast(msg.clone(), *error, cx),
            DbEvent::Status(msg) => self.status = msg.clone(),
            DbEvent::OpenFile(path) => self.open_file(path, window, cx),
            DbEvent::ShowGrid(g) => {
                if let Some(i) = self.tabs.iter().position(|t| matches!(t, Tab::Grid(x) if x == g)) {
                    self.active = Some(i);
                    self.focus_editor(window, cx);
                } else {
                    self.add_tab(Tab::Grid(g.clone()));
                    let f = g.read(cx).focus_handle(cx);
                    window.focus(&f);
                }
            }
            DbEvent::ShowResults => {
                self.bottom = Some(BottomTool::Services);
                self.services.selected = Some(super::services_ui::Item::Database);
            }
            DbEvent::ShowWindow => {
                self.db.open = true;
                window.focus(&self.db.focus);
            }
            DbEvent::FocusKeys => window.focus(&self.focus),
            DbEvent::DialogClosed => {
                if self.db.open {
                    window.focus(&self.db.focus);
                } else {
                    self.focus_editor(window, cx);
                }
            }
            DbEvent::FocusEditor => self.focus_editor(window, cx),
            DbEvent::Dispatch(action) => self.dispatch(*action, window, cx),
        }
        cx.notify();
    }

    pub(super) fn db_action(&mut self, action: Action, window: &mut Window, cx: &mut Context<Self>) {
        use Action as A;
        let m = self.db_model(window, cx);
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
            A::NewDataSource => m.update(cx, |m, cx| m.open_dialog(None, None, None, cx)),
            A::DataSourceProperties => m.update(cx, |m, cx| {
                if let Some(di) = m.selected_source() {
                    m.open_dialog(Some(di), None, None, cx);
                }
            }),
            A::OpenQueryConsole => {
                let di = if self.db.focus.is_focused(window) {
                    m.read(cx).selected_source()
                } else {
                    let path = self.active_editor().and_then(|e| e.read(cx).path.clone());
                    let db = m.read(cx);
                    path.and_then(|p| db.source_for_file(&p)).or_else(|| db.selected_source())
                };
                match di {
                    Some(di) => self.open_console(di, window, cx),
                    None => self.show_toast("No data sources. Add one in the Database tool window (Alt+Insert).".into(), true, cx),
                }
            }
            A::EditTableData => m.update(cx, |m, cx| m.open_selected(cx)),
            A::RefreshDatabase => match self.focused_grid(window, cx) {
                Some(g) => m.update(cx, |m, cx| m.reload(g, cx)),
                None => m.update(cx, |m, cx| {
                    if let Some(di) = m.selected_source() {
                        m.refresh(di, cx);
                    }
                }),
            },
            A::Disconnect => m.update(cx, |m, cx| {
                if let Some(di) = m.selected_source() {
                    m.disconnect(di, cx);
                }
            }),
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
        self.db.results(cx).filter(|g| g.read(cx).focus_handle(cx).is_focused(window))
    }

    /// Ctrl+Enter in a SQL file: the selection or the statement at the caret.
    fn execute_console(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let Some(ed) = self.focused_editor(window, cx) else { return };
        let path = ed.read(cx).path.clone();
        let m = self.db_model(window, cx);
        let Some(di) = path.as_deref().and_then(|p| m.read(cx).source_for_file(p)) else {
            self.show_toast("No data source for this file. Add one in the Database tool window (Alt+Insert).".into(), true, cx);
            return;
        };
        let Some(driver) = m.read(cx).driver(di) else { return };
        let (sel, text, caret) = ed.update(cx, |e, _| e.sql_to_run());
        let sql = match sel {
            Some(s) => s,
            None => match brisk_db::statement_at(&text, caret, driver) {
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
        m.update(cx, |m, cx| m.run_console(di, sql, cx));
    }

    /// Open the query console of data source `di` in an editor tab.
    fn open_console(&mut self, di: usize, window: &mut Window, cx: &mut Context<Self>) {
        let m = self.db_model(window, cx);
        if let Some(path) = m.update(cx, |m, cx| m.console(di, cx)) {
            self.open_file(&path, window, cx);
        }
    }

    /// Keys in the Database tree that the keymap doesn't bind.
    pub(super) fn db_tree_key(&mut self, k: &gpui::Keystroke, window: &mut Window, cx: &mut Context<Self>) -> bool {
        let m = self.db_model(window, cx);
        m.update(cx, |m, cx| m.tree_key(k, cx))
    }

    pub(super) fn db_dialog_key(&mut self, e: &KeyDownEvent, window: &mut Window, cx: &mut Context<Self>) {
        let m = self.db_model(window, cx);
        m.update(cx, |m, cx| m.dialog_key(e, cx));
    }

    pub(super) fn render_database(&mut self, _cx: &mut Context<Self>) -> gpui::AnyElement {
        match &self.db.model {
            Some(m) => m.clone().into_any_element(),
            None => div().into_any_element(),
        }
    }

    /// The database console's results, on the right of Services.
    pub(super) fn render_db_results(&mut self, cx: &mut Context<Self>) -> gpui::AnyElement {
        match self.db.results(cx) {
            Some(g) => {
                let title = g.read(cx).title.clone();
                div()
                    .flex_1()
                    .min_w_0()
                    .flex()
                    .flex_col()
                    .h_full()
                    .child(Self::panel_header(&format!("{title} · console")))
                    .child(div().flex_1().min_h_0().child(g))
                    .into_any_element()
            }
            None => Self::render_placeholder(
                "Services",
                "Query results appear here. Open a console from the Database tool window (Ctrl+Shift+F10) and run a statement with Ctrl+Enter.",
            ),
        }
    }

    pub(super) fn render_ds_dialog(&mut self, cx: &mut Context<Self>) -> Option<gpui::AnyElement> {
        let m = self.db.model.clone()?;
        m.update(cx, |m, cx| m.render_dialog(cx))
    }

    /// Smoke hook: run `sql` in a console of the first data source, print the outcome, open
    /// the first table's data.
    #[cfg(feature = "smoke")]
    pub(super) fn db_smoke(&mut self, sql: String, window: &mut Window, cx: &mut Context<Self>) {
        let m = self.db_model(window, cx);
        if m.read(cx).sources.is_empty() {
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
                let done =
                    this.update(cx, |this, cx| this.db.results(cx).is_some_and(|g| !g.read(cx).is_running()) && this.db.running(cx).is_none()).unwrap_or(true);
                if done {
                    break;
                }
            }
            let _ = m.update(cx, |m, cx| {
                let msg = match m.results.as_ref().map(|g| &g.read(cx).state) {
                    Some(crate::grid_view::GridState::Done { message, out }) => format!("{message}; columns {:?}", out.columns),
                    Some(crate::grid_view::GridState::Failed(e)) => format!("error: {e}"),
                    _ => "no result".into(),
                };
                let tables: usize = m.live(0).map_or(0, |l| l.schemas.iter().map(|s| s.tables.len()).sum());
                eprintln!("smoke-db: {msg} (since start {:.1?}, tables {tables}, tree rows {})", t0.elapsed(), m.rows.len());
                // Open the first table's data, as F4 would.
                if let Some(ix) = m.rows.iter().position(|r| matches!(r.kind, RowKind::Table(..))) {
                    m.selected = ix;
                    m.open_selected(cx);
                }
                cx.notify();
            });
            // A real click on the data source's fold arrow folds it, a second one unfolds it.
            for step in ["fold", "unfold"] {
                bg.timer(std::time::Duration::from_millis(400)).await;
                let first = m.read_with(cx, |m, _| m.rows.first().map(|r| r.key.clone())).ok().flatten();
                let _ = this.update_in(cx, |_, window, _| match FOLD_BOUNDS.with(|c| c.get()) {
                    Some(b) => {
                        let s = window.scale_factor();
                        super::smoke::win_input::click(f32::from(b.center().x) * s, f32::from(b.center().y) * s);
                    }
                    None => eprintln!("smoke-db: FAIL the fold arrow of {first:?} was not painted"),
                });
                bg.timer(std::time::Duration::from_millis(400)).await;
                let _ = m.read_with(cx, |m, _| {
                    let open = m.rows.first().is_some_and(|r| m.expanded.contains(&r.key));
                    eprintln!("smoke-db: click the fold arrow ({step}) -> expanded {open}, tree rows {}", m.rows.len());
                });
            }
        })
        .detach();
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

thread_local! {
    /// Where the first tree row's fold arrow was painted: the smoke clicks it.
    static FOLD_BOUNDS: std::cell::Cell<Option<gpui::Bounds<gpui::Pixels>>> = const { std::cell::Cell::new(None) };
}

/// A canvas filling its parent that records the parent's bounds in `FOLD_BOUNDS`.
fn track_fold() -> impl IntoElement {
    gpui::canvas(|b, _, _| FOLD_BOUNDS.with(|c| c.set(Some(b))), |_, _, _, _| {}).absolute().top_0().left_0().size_full()
}
