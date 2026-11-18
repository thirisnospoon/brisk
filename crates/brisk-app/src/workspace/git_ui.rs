//! Git UI: Commit tool window (Alt+0), Git log tool window (Alt+9), branches popup, diffs,
//! notifications. All git calls run on the background executor; the UI never blocks on git
//! except for reading one file revision when a diff is opened.

use super::*;
use brisk_git::{Branch, Commit, FileChange, FileStatus, GraphRow, Repo, Status};
use gpui::{canvas, fill, point, size, Bounds, Hsla, PathBuilder};
use std::collections::{HashMap, HashSet};
use std::time::Duration;

const LOG_LIMIT: usize = 3000;
/// Rows the Commit window draws per group. The list is not virtualized, so a repository with
/// 24k unversioned files built 24k rows every frame (1.5 GB and a busy core). Check-box and
/// commit still act on every file; only drawing stops here.
const MAX_SHOWN_ROWS: usize = 1000;

fn more_row(n: usize) -> gpui::Div {
    div().h(px(24.)).flex().items_center().pl(px(28.)).text_size(px(12.)).text_color(Theme::text_dim()).child(format!("… {n} more file{} not shown", if n == 1 { "" } else { "s" }))
}
const LOG_ROW_H: f32 = 24.0;
const LANE_W: f32 = 14.0;

#[derive(Clone, Debug)]
pub enum Pending {
    Rollback(Vec<FileChange>),
    CreateBranch(String),
    Branch(super::git_branch_ui::BranchOp),
}

pub struct Details {
    hash: String,
    message: String,
    files: Vec<FileChange>,
}

pub struct Git {
    pub repo: Repo,
    pub(super) status: Status,
    by_path: HashMap<String, FileStatus>,
    pub(super) branches: Vec<Branch>,
    pub(super) log: Vec<Commit>,
    graph: Vec<GraphRow>,
    graph_w: f32,
    pub(super) log_selected: Option<usize>,
    details: Option<Details>,
    details_sel: Option<usize>,
    log_scroll: UniformListScrollHandle,
    pub log_focus: FocusHandle,
    busy: Option<&'static str>,
    /// PyCharm: tracked changes are included by default, unversioned files are not.
    excluded: HashSet<String>,
    included_untracked: HashSet<String>,
    commit_sel: Option<usize>,
    pub message: Entity<Editor>,
    amend: bool,
    head_seen: Option<String>,
    loaded: bool,
    /// Conflicts, Merge Revisions, Stash dialogs.
    pub dialog: Option<super::git_merge_ui::GitDialog>,
    /// Conflicted files at the last refresh: the Conflicts dialog opens when they appear.
    conflicts_seen: usize,
    /// Text / Branch / User filters of the log.
    pub(super) filter: super::git_log_filter::LogFilter,
}

impl Git {
    pub fn new(repo: Repo, cx: &mut Context<Workspace>) -> Self {
        Self {
            repo,
            status: Status::default(),
            by_path: HashMap::new(),
            branches: Vec::new(),
            log: Vec::new(),
            graph: Vec::new(),
            graph_w: LANE_W,
            log_selected: None,
            details: None,
            details_sel: None,
            log_scroll: UniformListScrollHandle::new(),
            log_focus: cx.focus_handle(),
            busy: None,
            excluded: HashSet::new(),
            included_untracked: HashSet::new(),
            commit_sel: None,
            message: cx.new(|cx| Editor::scratch("", cx)),
            amend: false,
            head_seen: None,
            loaded: false,
            dialog: None,
            conflicts_seen: 0,
            filter: super::git_log_filter::LogFilter::new(cx),
        }
    }

    /// Rows of the commit window: tracked changes, then unversioned files.
    fn rows(&self) -> Vec<FileChange> {
        self.status.tracked().chain(self.status.untracked()).cloned().collect()
    }

    fn included(&self, c: &FileChange) -> bool {
        if c.status == FileStatus::Untracked {
            self.included_untracked.contains(&c.path)
        } else {
            !self.excluded.contains(&c.path)
        }
    }

    fn toggle(&mut self, c: &FileChange) {
        let on = !self.included(c);
        self.set_included(c, on);
    }

    fn set_included(&mut self, c: &FileChange, on: bool) {
        if c.status == FileStatus::Untracked {
            if on {
                self.included_untracked.insert(c.path.clone());
            } else {
                self.included_untracked.remove(&c.path);
            }
        } else if on {
            self.excluded.remove(&c.path);
        } else {
            self.excluded.insert(c.path.clone());
        }
    }
}

pub fn status_color(s: FileStatus) -> Hsla {
    match s {
        FileStatus::Modified | FileStatus::Renamed => Theme::file_modified(),
        FileStatus::Added => Theme::file_added(),
        FileStatus::Deleted => Theme::file_deleted(),
        FileStatus::Untracked => Theme::file_untracked(),
        FileStatus::Conflict => Theme::file_conflict(),
    }
}

pub(super) fn file_name(rel: &str) -> &str {
    rel.rsplit('/').next().unwrap_or(rel)
}

pub(super) fn dir_name(rel: &str) -> &str {
    rel.rsplit_once('/').map(|(d, _)| d).unwrap_or("")
}

/// "5 minutes ago" for recent commits, otherwise a UTC date (no tz database needed).
pub(super) fn fmt_time(t: i64) -> String {
    let now = std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).map(|d| d.as_secs() as i64).unwrap_or(0);
    let d = now - t;
    if (0..60).contains(&d) {
        return "Moments ago".into();
    }
    if (0..3600).contains(&d) {
        return format!("{} minute{} ago", d / 60, if d / 60 == 1 { "" } else { "s" });
    }
    if (0..86400).contains(&d) {
        return format!("{} hour{} ago", d / 3600, if d / 3600 == 1 { "" } else { "s" });
    }
    // Civil date from days since epoch (Howard Hinnant's algorithm).
    let z = t.div_euclid(86400) + 719468;
    let era = z.div_euclid(146097);
    let doe = z - era * 146097;
    let yoe = (doe - doe / 1460 + doe / 36524 - doe / 146096) / 365;
    let doy = doe - (365 * yoe + yoe / 4 - yoe / 100);
    let mp = (5 * doy + 2) / 153;
    let day = doy - (153 * mp + 2) / 5 + 1;
    let month = if mp < 10 { mp + 3 } else { mp - 9 };
    let year = yoe + era * 400 + if month <= 2 { 1 } else { 0 };
    let secs = t.rem_euclid(86400);
    format!("{year:04}-{month:02}-{day:02} {:02}:{:02}", secs / 3600, secs % 3600 / 60)
}

fn checkbox(on: Option<bool>) -> gpui::Div {
    // Some(true) checked, Some(false) unchecked, None partial.
    let filled = on != Some(false);
    div()
        .w(px(14.))
        .h(px(14.))
        .flex_shrink_0()
        .rounded(px(3.))
        .border_1()
        .border_color(if filled { Theme::accent() } else { Theme::text_dim() })
        .when(filled, |d| d.bg(Theme::accent()))
        .flex()
        .items_center()
        .justify_center()
        .text_size(px(10.))
        .text_color(Theme::text())
        .child(match on {
            Some(true) => "✓",
            None => "–",
            Some(false) => "",
        })
}

impl Workspace {
    pub(super) fn branch_label(&self) -> String {
        match &self.git {
            Some(g) => match (&g.status.branch, &g.status.head) {
                (Some(b), _) => b.clone(),
                (None, Some(h)) => format!("{} (detached)", &h[..h.len().min(8)]),
                (None, None) if g.loaded => "main".into(),
                _ => self.branch.clone().unwrap_or_default(),
            },
            None => "Version Control".into(),
        }
    }

    pub(super) fn file_status(&self, path: &Path) -> Option<FileStatus> {
        let g = self.git.as_ref()?;
        if g.by_path.is_empty() {
            return None;
        }
        g.by_path.get(&g.repo.rel(path)?).copied()
    }

    pub(super) fn refresh_git(&mut self, cx: &mut Context<Self>) {
        let Some(g) = &self.git else { return };
        let repo = g.repo.clone();
        let want_log = self.bottom == Some(BottomTool::Git);
        let query = g.filter.query.clone();
        let task = cx.background_executor().spawn(async move {
            let st = repo.status();
            let br = repo.branches().unwrap_or_default();
            let log = want_log.then(|| repo.log_query(LOG_LIMIT, &query).unwrap_or_default());
            (st, br, log, query)
        });
        cx.spawn(async move |this, cx| {
            let (st, br, log, query) = task.await;
            let _ = this.update(cx, |this, cx| {
                // The filters changed while this ran: a newer refresh brings the right log.
                let log = log.filter(|_| this.git.as_ref().is_some_and(|g| g.filter.query == query));
                this.apply_git(st, br, log, cx)
            });
        })
        .detach();
    }

    fn apply_git(&mut self, st: brisk_git::Result<Status>, br: Vec<Branch>, log: Option<Vec<Commit>>, cx: &mut Context<Self>) {
        let Some(g) = self.git.as_mut() else { return };
        let mut head_changed = false;
        match st {
            Ok(st) => {
                g.by_path = st.by_path();
                head_changed = g.head_seen != st.head || !g.loaded;
                g.head_seen = st.head.clone();
                // Forget inclusion flags of files that are no longer changed.
                g.excluded.retain(|p| g.by_path.contains_key(p));
                g.included_untracked.retain(|p| g.by_path.get(p) == Some(&FileStatus::Untracked));
                g.status = st;
                g.loaded = true;
                let n = g.rows().len();
                g.commit_sel = g.commit_sel.filter(|&i| i < n).or((n > 0).then_some(0));
            }
            Err(e) => self.status = format!("git: {e}"),
        }
        g.branches = br;
        if let Some(log) = log {
            let sel_hash = g.log_selected.and_then(|i| g.log.get(i)).map(|c| c.hash.clone());
            g.graph = brisk_git::layout(log.iter().map(|c| (c.hash.as_str(), c.parents.as_slice())));
            let lanes = g.graph.iter().map(|r| r.width).max().unwrap_or(1).min(16);
            g.graph_w = lanes as f32 * LANE_W + 8.;
            g.log = log;
            g.log_selected = sel_hash.and_then(|h| g.log.iter().position(|c| c.hash == h));
        }
        if head_changed {
            let eds: Vec<_> = self.editors().cloned().collect();
            for e in eds {
                self.load_vcs_base(e, cx);
            }
        }
        self.refresh_annotations(head_changed, cx);
        // New conflicts (a merge, pull or rebase stopped): show them, like PyCharm.
        if let Some(g) = self.git.as_mut() {
            let n = g.status.changes.iter().filter(|c| c.status == FileStatus::Conflict).count();
            let appeared = n > 0 && g.conflicts_seen == 0;
            g.conflicts_seen = n;
            if appeared && g.dialog.is_none() {
                self.open_conflicts(cx);
            }
        }
        if self.git.as_ref().is_some_and(|g| g.log_selected.is_none() && !g.log.is_empty()) {
            self.select_log(0, cx);
        }
        cx.notify();
    }

    pub(super) fn load_vcs_base(&self, ed: Entity<Editor>, cx: &mut Context<Self>) {
        let Some(g) = &self.git else { return };
        let Some(path) = ed.read(cx).path.clone() else { return };
        let Some(rel) = g.repo.rel(&path) else { return };
        let repo = g.repo.clone();
        let task = cx.background_executor().spawn(async move { repo.file_at("HEAD", &rel).ok().flatten() });
        cx.spawn(async move |_, cx| {
            let base = task.await;
            let _ = ed.update(cx, |e, cx| {
                e.set_vcs_base(base);
                cx.notify();
            });
        })
        .detach();
    }

    pub(super) fn show_toast(&mut self, msg: String, error: bool, cx: &mut Context<Self>) {
        self.toast = Some((msg, error));
        self.toast_gen += 1;
        let gen = self.toast_gen;
        let timer = cx.background_executor().timer(Duration::from_secs(if error { 12 } else { 5 }));
        cx.spawn(async move |this, cx| {
            timer.await;
            let _ = this.update(cx, |this, cx| {
                if this.toast_gen == gen {
                    this.toast = None;
                    cx.notify();
                }
            });
        })
        .detach();
        cx.notify();
    }

    /// Run a git operation in the background, then notify, reload files and refresh state.
    pub(super) fn git_op(
        &mut self,
        title: &'static str,
        op: impl FnOnce(&Repo) -> brisk_git::Result<String> + Send + 'static,
        then: impl FnOnce(&mut Self, &mut Context<Self>) + 'static,
        cx: &mut Context<Self>,
    ) {
        let Some(g) = self.git.as_mut() else {
            return self.show_toast("Version control is not enabled for this project".into(), true, cx);
        };
        if let Some(b) = g.busy {
            return self.show_toast(format!("Wait for “{b}” to finish"), true, cx);
        }
        g.busy = Some(title);
        let repo = g.repo.clone();
        self.status = format!("{title}…");
        let task = cx.background_executor().spawn(async move { op(&repo) });
        cx.spawn(async move |this, cx| {
            let res = task.await;
            let _ = this.update(cx, |this, cx| {
                if let Some(g) = this.git.as_mut() {
                    g.busy = None;
                }
                this.status.clear();
                match res {
                    Ok(msg) => {
                        this.show_toast(msg, false, cx);
                        then(this, cx);
                    }
                    Err(e) => this.show_toast(format!("{title} failed: {e}"), true, cx),
                }
                this.reload_editors(cx);
                this.refresh_git(cx);
            });
        })
        .detach();
        cx.notify();
    }

    pub(super) fn checkout(&mut self, b: Branch, cx: &mut Context<Self>) {
        if b.current {
            return;
        }
        self.save_all(cx);
        let name = b.name.clone();
        self.git_op("Checkout", move |r| r.checkout(&b).map(|_| format!("Checked out {name}")), |_, _| {}, cx);
    }

    pub(super) fn run_pending(&mut self, p: Pending, cx: &mut Context<Self>) {
        match p {
            Pending::Rollback(files) => {
                self.save_all(cx);
                let n = files.len();
                self.git_op("Rollback", move |r| r.rollback(&files).map(|_| format!("Rolled back {n} file{}", if n == 1 { "" } else { "s" })), |_, _| {}, cx);
            }
            Pending::CreateBranch(name) => {
                let n2 = name.clone();
                self.git_op("New Branch", move |r| r.create_branch(&n2).map(|_| format!("Created and checked out {n2}")), |_, _| {}, cx);
                let _ = name;
            }
            Pending::Branch(op) => self.run_branch_op(op, cx),
        }
    }

    fn confirm(&mut self, label: String, detail: String, p: Pending) {
        let items = vec![
            PopupItem { label, detail, hint: "Enter".into(), target: Target::Confirm(p) },
            PopupItem { label: "Cancel".into(), detail: String::new(), hint: "Esc".into(), target: Target::Cancel },
        ];
        self.popup = Some(Popup { kind: PopupKind::Confirm, query: String::new(), selected: 0, items });
    }

    pub(super) fn git_action(&mut self, action: Action, window: &mut Window, cx: &mut Context<Self>) {
        use Action as A;
        if self.git.is_none() && action != A::Commit {
            self.left = Some(LeftTool::Commit);
            return self.show_toast("Version control is not enabled. Use “Create Git Repository” in the Commit window.".into(), true, cx);
        }
        match action {
            A::Commit => {
                self.left = Some(LeftTool::Commit);
                match &self.git {
                    Some(g) => {
                        let f = g.message.read(cx).focus_handle(cx);
                        window.focus(&f);
                    }
                    None => window.focus(&self.commit_focus),
                }
                self.refresh_git(cx);
            }
            A::Push => self.open_push_dialog(cx),
            A::Fetch => self.git_op("Fetch", |r| r.fetch_summary(), |_, _| {}, cx),
            A::UpdateProject => {
                self.save_all(cx);
                self.git_op(
                    "Update Project",
                    |r| r.update_project(),
                    |_, _| {},
                    cx,
                );
            }
            A::VcsQuickList => self.open_popup(PopupKind::VcsOperations),
            A::Branches => {
                self.refresh_git(cx);
                self.open_popup(PopupKind::Branches);
            }
            A::ShowDiff => {
                let ctx = self.contexts(window, cx);
                if ctx.first() == Some(&"GitLog") {
                    if let Some((hash, c)) = self.git.as_ref().and_then(|g| {
                        let d = g.details.as_ref()?;
                        Some((d.hash.clone(), d.files.get(g.details_sel.unwrap_or(0))?.clone()))
                    }) {
                        self.show_commit_diff(&hash, &c, window, cx);
                    }
                } else if ctx.first() == Some(&"CommitView") {
                    if let Some(c) = self.selected_change() {
                        self.show_change_diff(&c, window, cx);
                    }
                } else if let Some(c) = self.active_file_change(cx) {
                    self.show_change_diff(&c, window, cx);
                }
            }
            A::ResolveConflicts => self.open_conflicts(cx),
            A::StashChanges | A::UnstashChanges | A::ShowFileHistory | A::CompareWithBranch => self.git_more_action(action, window, cx),
            A::Annotate => match self.active_editor().cloned() {
                Some(ed) => self.toggle_annotate(ed, cx),
                None => self.status = "Annotate: open a file first".into(),
            },
            A::RollbackChanges => {
                let files: Vec<FileChange> = if self.commit_focus.is_focused(window) {
                    let checked: Vec<FileChange> = self.git.as_ref().map(|g| g.rows().into_iter().filter(|c| c.status != FileStatus::Untracked && g.included(c)).collect()).unwrap_or_default();
                    if checked.is_empty() {
                        self.selected_change().into_iter().collect()
                    } else {
                        checked
                    }
                } else {
                    self.active_file_change(cx).into_iter().collect()
                };
                let files: Vec<FileChange> = files.into_iter().filter(|c| c.status != FileStatus::Untracked).collect();
                if files.is_empty() {
                    return self.show_toast("No changes to roll back".into(), false, cx);
                }
                let names = files.iter().map(|c| file_name(&c.path)).collect::<Vec<_>>().join(", ");
                self.confirm(format!("Rollback {} file{}", files.len(), if files.len() == 1 { "" } else { "s" }), names, Pending::Rollback(files));
            }
            _ => {}
        }
    }

    fn selected_change(&self) -> Option<FileChange> {
        let g = self.git.as_ref()?;
        g.rows().get(g.commit_sel?).cloned()
    }

    /// The active editor's file as a change (Modified if git reports nothing: diff shows "identical").
    fn active_file_change(&self, cx: &App) -> Option<FileChange> {
        let g = self.git.as_ref()?;
        let path = self.active_editor()?.read(cx).path.clone()?;
        let rel = g.repo.rel(&path)?;
        let st = g.by_path.get(&rel).copied().unwrap_or(FileStatus::Modified);
        let old = g.status.changes.iter().find(|c| c.path == rel).and_then(|c| c.old_path.clone());
        Some(FileChange { path: rel, old_path: old, status: st })
    }

    pub(super) fn open_diff(&mut self, d: Entity<DiffView>, window: &mut Window, cx: &mut Context<Self>) {
        let title = d.read(cx).title.clone();
        if let Some(i) = self.tabs.iter().position(|t| matches!(t, Tab::Diff(x) if x.read(cx).title == title)) {
            self.tabs[i] = Tab::Diff(d);
            self.active = Some(i);
        } else {
            self.add_tab(Tab::Diff(d));
        }
        self.focus_editor(window, cx);
        cx.notify();
    }

    /// HEAD ↔ working copy (unsaved editor text included).
    fn show_change_diff(&mut self, c: &FileChange, window: &mut Window, cx: &mut Context<Self>) {
        let Some(g) = &self.git else { return };
        let abs = g.repo.abs(&c.path);
        let base_path = c.old_path.clone().unwrap_or_else(|| c.path.clone());
        let left = match c.status {
            FileStatus::Untracked | FileStatus::Added => String::new(),
            _ => g.repo.file_at("HEAD", &base_path).ok().flatten().unwrap_or_default(),
        };
        let right = self
            .editors()
            .find(|e| e.read(cx).path.as_deref().is_some_and(|p| g.repo.rel(p).as_deref() == Some(c.path.as_str())))
            .map(|e| e.read(cx).text_string())
            .or_else(|| std::fs::read_to_string(&abs).ok().map(|s| s.replace("\r\n", "\n")))
            .unwrap_or_default();
        let name = file_name(&c.path).to_string();
        let view = cx.new(|cx| DiffView::new(format!("{name} (HEAD ↔ Local)"), Some(abs), format!("HEAD: {base_path}"), &left, "Your version".into(), &right, cx));
        self.open_diff(view, window, cx);
    }

    pub(super) fn show_commit_diff(&mut self, hash: &str, c: &FileChange, window: &mut Window, cx: &mut Context<Self>) {
        let Some(g) = &self.git else { return };
        let short = &hash[..hash.len().min(8)];
        let base_path = c.old_path.clone().unwrap_or_else(|| c.path.clone());
        let left = if c.status == FileStatus::Added { String::new() } else { g.repo.file_at(&format!("{hash}^"), &base_path).ok().flatten().unwrap_or_default() };
        let right = if c.status == FileStatus::Deleted { String::new() } else { g.repo.file_at(hash, &c.path).ok().flatten().unwrap_or_default() };
        let name = file_name(&c.path).to_string();
        let abs = g.repo.abs(&c.path);
        let title = format!("{name} ({short})");
        // A new file (the root commit too) has no left side: no "hash^" label there.
        let left_label = if c.status == FileStatus::Added { String::new() } else { format!("{short}^") };
        let view = cx.new(|cx| DiffView::new(title, Some(abs), left_label, &left, short.to_string(), &right, cx));
        self.open_diff(view, window, cx);
    }

    fn select_log(&mut self, i: usize, cx: &mut Context<Self>) {
        let Some(g) = self.git.as_mut() else { return };
        let Some(c) = g.log.get(i) else { return };
        g.log_selected = Some(i);
        g.details_sel = None;
        let hash = c.hash.clone();
        if g.details.as_ref().is_some_and(|d| d.hash == hash) {
            return;
        }
        let repo = g.repo.clone();
        let task = cx.background_executor().spawn(async move {
            let msg = repo.commit_message(&hash).unwrap_or_default();
            let files = repo.commit_files(&hash).unwrap_or_default();
            Details { hash, message: msg, files }
        });
        cx.spawn(async move |this, cx| {
            let d = task.await;
            let _ = this.update(cx, |this, cx| {
                if let Some(g) = this.git.as_mut() {
                    if g.log_selected.and_then(|i| g.log.get(i)).is_some_and(|c| c.hash == d.hash) {
                        g.details = Some(d);
                        cx.notify();
                    }
                }
            });
        })
        .detach();
        cx.notify();
    }

    fn do_commit(&mut self, push: bool, window: &mut Window, cx: &mut Context<Self>) {
        let Some(g) = &self.git else { return };
        let msg = g.message.read(cx).text_string();
        let files: Vec<FileChange> = g.rows().into_iter().filter(|c| g.included(c)).collect();
        let amend = g.amend;
        if files.is_empty() && !amend {
            return self.show_toast("No changes selected for commit".into(), true, cx);
        }
        if msg.trim().is_empty() && !amend {
            let f = g.message.read(cx).focus_handle(cx);
            window.focus(&f);
            return self.show_toast("Commit message is empty".into(), true, cx);
        }
        self.save_all(cx);
        let paths: Vec<String> = files.iter().flat_map(|c| std::iter::once(c.path.clone()).chain(c.old_path.clone())).collect();
        let untracked: Vec<String> = files.iter().filter(|c| c.status == FileStatus::Untracked).map(|c| c.path.clone()).collect();
        let n = files.len();
        let subject = msg.lines().next().unwrap_or("").to_string();
        self.git_op(
            if push { "Commit and Push" } else { "Commit" },
            move |r| {
                let short = r.commit(&msg, &paths, amend, &untracked)?;
                let s = if n == 1 { "" } else { "s" };
                Ok(format!("{n} file{s} committed: {subject} ({short})"))
            },
            move |this, cx| {
                // Commit and Push…: the Push dialog follows, like PyCharm.
                if push {
                    this.open_push_dialog(cx);
                }
                if let Some(g) = this.git.as_mut() {
                    g.amend = false;
                    g.message.update(cx, |e, cx| {
                        e.buffer.set_text("");
                        e.buffer.mark_saved();
                        cx.notify();
                    });
                }
            },
            cx,
        );
    }

    pub(super) fn git_popup_items(&self, kind: PopupKind, q: &str) -> Vec<PopupItem> {
        let ql = q.to_lowercase();
        match kind {
            PopupKind::Branches => {
                let Some(g) = &self.git else { return Vec::new() };
                let mut v = Vec::new();
                if ql.is_empty() {
                    v.push(PopupItem { label: "New Branch…".into(), detail: String::new(), hint: String::new(), target: Target::NewBranch });
                }
                for b in g.branches.iter().filter(|b| b.name.to_lowercase().contains(&ql)) {
                    // "origin/<same name>" says nothing new: just the remote.
                    let upstream = b.upstream.as_ref().map(|u| match u.split_once('/') {
                        Some((remote, rest)) if rest == b.name => remote.to_string(),
                        _ => u.clone(),
                    });
                    let detail = match (b.current, b.remote, &upstream) {
                        (true, _, Some(u)) => format!("current · tracks {u}"),
                        (true, _, None) => "current".into(),
                        (false, true, _) => "remote".into(),
                        (false, false, Some(u)) => format!("tracks {u}"),
                        _ => String::new(),
                    };
                    v.push(PopupItem { label: b.name.clone(), detail, hint: format!("{}  →", &b.hash[..b.hash.len().min(8)]), target: Target::Branch(b.clone()) });
                }
                v
            }
            PopupKind::NewBranch => {
                let name = q.trim();
                if name.is_empty() {
                    Vec::new()
                } else if name.contains(char::is_whitespace) || name.contains("..") || name.starts_with('-') {
                    vec![PopupItem { label: "Invalid branch name".into(), detail: String::new(), hint: String::new(), target: Target::Cancel }]
                } else {
                    vec![PopupItem {
                        label: format!("Create branch “{name}” and check it out"),
                        detail: format!("from {}", self.branch_label()),
                        hint: "Enter".into(),
                        target: Target::Confirm(Pending::CreateBranch(name.to_string())),
                    }]
                }
            }
            PopupKind::VcsOperations => {
                use Action as A;
                [A::Commit, A::Push, A::UpdateProject, A::Fetch, A::Branches, A::ShowDiff, A::RollbackChanges, A::Annotate, A::ShowFileHistory, A::CompareWithBranch, A::ResolveConflicts, A::StashChanges, A::UnstashChanges, A::ToggleGitWindow, A::ToggleCommitWindow]
                    .into_iter()
                    .filter(|a| a.info().title.to_lowercase().contains(&ql))
                    .map(|a| PopupItem {
                        label: match a {
                            A::ToggleGitWindow => "Show Git Log".into(),
                            A::ToggleCommitWindow => "Commit Tool Window".into(),
                            _ => a.info().title.to_string(),
                        },
                        detail: String::new(),
                        hint: self.resolver.keymap().shortcut_label(a).unwrap_or_default(),
                        target: Target::Action(a),
                    })
                    .collect()
            }
            PopupKind::Confirm => self.popup.as_ref().map(|p| p.items.clone()).unwrap_or_default(),
            _ => Vec::new(),
        }
    }

    /// Keys handled before the keymap: commit message shortcuts and diff navigation.
    pub(super) fn git_pre_key(&mut self, e: &KeyDownEvent, window: &mut Window, cx: &mut Context<Self>) -> bool {
        if self.log_filter_key(e, window, cx) {
            return true;
        }
        let k = &e.keystroke;
        let m = &k.modifiers;
        if let Some(g) = &self.git {
            if g.message.read(cx).focus_handle(cx).is_focused(window) {
                if k.key == "enter" && m.control {
                    self.do_commit(false, window, cx);
                    return true;
                }
                if k.key == "k" && m.control && m.alt {
                    self.do_commit(true, window, cx);
                    return true;
                }
                if k.key == "escape" && !m.control && !m.alt && !m.shift {
                    if self.active.is_some() {
                        self.focus_editor(window, cx);
                    } else {
                        window.focus(&self.commit_focus);
                    }
                    cx.notify();
                    return true;
                }
            }
        }
        if let Some(d) = self.active_diff(window, cx) {
            match k.key.as_str() {
                "f7" if !m.control && !m.alt => {
                    d.update(cx, |d, cx| d.jump(!m.shift, cx));
                    return true;
                }
                "f4" => {
                    if let Some(p) = d.read(cx).path.clone() {
                        self.open_file(&p, window, cx);
                    }
                    return true;
                }
                _ => {}
            }
        }
        false
    }

    pub(super) fn commit_key(&mut self, k: &gpui::Keystroke, window: &mut Window, cx: &mut Context<Self>) -> bool {
        let Some(g) = self.git.as_mut() else { return false };
        let rows = g.rows();
        let n = rows.len();
        let cur = g.commit_sel.unwrap_or(0);
        match k.key.as_str() {
            "up" => g.commit_sel = Some(cur.saturating_sub(1)),
            "down" => g.commit_sel = Some((cur + 1).min(n.saturating_sub(1))),
            "space" => {
                if let Some(c) = rows.get(cur) {
                    g.toggle(c);
                }
            }
            "enter" | "f4" => {
                if let Some(c) = rows.get(cur) {
                    let p = g.repo.abs(&c.path);
                    if c.status != FileStatus::Deleted {
                        self.open_file(&p, window, cx);
                    }
                }
            }
            "tab" => {
                let f = g.message.read(cx).focus_handle(cx);
                window.focus(&f);
            }
            _ => return false,
        }
        cx.notify();
        true
    }

    pub(super) fn log_key(&mut self, k: &gpui::Keystroke, _window: &mut Window, cx: &mut Context<Self>) -> bool {
        let Some(g) = self.git.as_ref() else { return false };
        let n = g.log.len();
        let cur = g.log_selected.unwrap_or(0);
        let next = match k.key.as_str() {
            "up" => cur.saturating_sub(1),
            "down" => (cur + 1).min(n.saturating_sub(1)),
            "pageup" => cur.saturating_sub(10),
            "pagedown" => (cur + 10).min(n.saturating_sub(1)),
            "home" => 0,
            "end" => n.saturating_sub(1),
            _ => return false,
        };
        g.log_scroll.scroll_to_item(next, gpui::ScrollStrategy::Center);
        self.select_log(next, cx);
        true
    }

    // --- Rendering ---------------------------------------------------------------

    pub(super) fn render_toast(&self) -> Option<impl IntoElement> {
        let (msg, err) = self.toast.as_ref()?;
        Some(
            div()
                .absolute()
                .bottom(px(36.))
                .right(px(16.))
                .max_w(px(460.))
                .px(px(14.))
                .py(px(10.))
                .flex()
                .gap(px(8.))
                .rounded(px(8.))
                .bg(Theme::panel())
                .border_1()
                .border_color(if *err { Theme::error() } else { Theme::divider() })
                .shadow_lg()
                .text_size(px(13.))
                .text_color(Theme::text())
                .child(div().text_color(if *err { Theme::error() } else { Theme::file_added() }).child(if *err { "⚠" } else { "✓" }))
                .child(msg.clone()),
        )
    }

    pub(super) fn render_commit(&mut self, cx: &mut Context<Self>) -> gpui::AnyElement {
        let Some(g) = &self.git else {
            return div()
                .flex()
                .flex_col()
                .size_full()
                .child(Self::panel_header("Commit"))
                .child(div().px(px(12.)).text_size(px(13.)).text_color(Theme::text_muted()).child("Version control is not enabled for this project."))
                .child(
                    div()
                        .id("git-init")
                        .m(px(12.))
                        .px(px(12.))
                        .py(px(6.))
                        .rounded(px(5.))
                        .bg(Theme::accent())
                        .text_size(px(13.))
                        .cursor_pointer()
                        .child("Create Git Repository…")
                        .on_click(cx.listener(|this, _, _, cx| match brisk_git::Repo::init(&this.root) {
                            Ok(repo) => {
                                this.git = Some(Git::new(repo, cx));
                                this.show_toast("Created Git repository".into(), false, cx);
                                this.refresh_git(cx);
                            }
                            Err(e) => this.show_toast(format!("git init failed: {e}"), true, cx),
                        })),
                )
                .into_any_element();
        };
        let rows = g.rows();
        let tracked: Vec<(usize, FileChange)> = rows.iter().cloned().enumerate().filter(|(_, c)| c.status != FileStatus::Untracked).collect();
        let untracked: Vec<(usize, FileChange)> = rows.iter().cloned().enumerate().filter(|(_, c)| c.status == FileStatus::Untracked).collect();
        let focused_list = self.commit_focus.clone();
        let sel = g.commit_sel;
        let amend = g.amend;
        let message = g.message.clone();
        let included_n = rows.iter().filter(|c| g.included(c)).count();
        let busy = g.busy;

        let group_state = |items: &[(usize, FileChange)]| -> Option<bool> {
            let on = items.iter().filter(|(_, c)| g.included(c)).count();
            if on == 0 {
                Some(false)
            } else if on == items.len() {
                Some(true)
            } else {
                None
            }
        };
        let tracked_state = group_state(&tracked);
        let untracked_state = group_state(&untracked);

        let file_row = |i: usize, c: &FileChange, included: bool, cx: &mut Context<Self>| {
            let c2 = c.clone();
            let c3 = c.clone();
            let (icon, icon_color) = file_icon(file_name(&c.path), false);
            div()
                .id(("chg", i))
                .h(px(24.))
                .flex()
                .items_center()
                .gap(px(6.))
                .pl(px(28.))
                .text_size(px(13.))
                .when(sel == Some(i), |d| d.bg(Theme::selection_row()))
                .when(!(sel == Some(i)), |d| d.hover(|s| s.bg(Theme::hover_row())))
                .on_click(cx.listener(move |this, e: &ClickEvent, window, cx| {
                    if let Some(g) = this.git.as_mut() {
                        g.commit_sel = Some(i);
                    }
                    window.focus(&this.commit_focus);
                    if e.click_count() >= 2 {
                        this.show_change_diff(&c2, window, cx);
                    }
                    cx.notify();
                }))
                .child(div().id(("chk", i)).child(checkbox(Some(included))).on_click(cx.listener(move |this, _, _, cx| {
                    cx.stop_propagation();
                    if let Some(g) = this.git.as_mut() {
                        g.toggle(&c3);
                    }
                    cx.notify();
                })))
                .child(div().w(px(14.)).text_color(icon_color).child(icon))
                .child(div().text_color(status_color(c.status)).whitespace_nowrap().child(file_name(&c.path).to_string()))
                .child(div().text_color(Theme::text_dim()).text_size(px(12.)).whitespace_nowrap().child(dir_name(&c.path).to_string()))
        };
        let group = |id: &'static str, title: &'static str, state: Option<bool>, items: Vec<FileChange>, cx: &mut Context<Self>| {
            let n = items.len();
            div()
                .id(id)
                .h(px(24.))
                .flex()
                .items_center()
                .gap(px(6.))
                .pl(px(10.))
                .text_size(px(13.))
                .child(div().w(px(10.)).text_color(Theme::text_muted()).child("▾"))
                .child(div().id((id, 1usize)).child(checkbox(state)).on_click(cx.listener(move |this, _, _, cx| {
                    if let Some(g) = this.git.as_mut() {
                        let on = state != Some(true);
                        for c in &items {
                            g.set_included(c, on);
                        }
                    }
                    cx.notify();
                })))
                .child(div().font_weight(gpui::FontWeight::SEMIBOLD).child(title))
                .child(div().text_color(Theme::text_dim()).text_size(px(12.)).child(format!("{n} file{}", if n == 1 { "" } else { "s" })))
        };

        let mut list = div()
            .id("commit-list")
            .track_focus(&focused_list)
            .key_context("CommitView")
            .flex()
            .flex_col()
            .flex_1()
            .min_h_0()
            .overflow_y_scroll();
        if rows.is_empty() {
            list = list.child(div().px(px(12.)).py(px(8.)).text_size(px(13.)).text_color(Theme::text_muted()).child(if g.loaded { "No changes" } else { "Loading…" }));
        }
        if !tracked.is_empty() {
            let items: Vec<FileChange> = tracked.iter().map(|(_, c)| c.clone()).collect();
            list = list.child(group("grp-changes", "Changes", tracked_state, items, cx));
            for (i, c) in tracked.iter().take(MAX_SHOWN_ROWS) {
                let inc = g.included(c);
                list = list.child(file_row(*i, c, inc, cx));
            }
            if tracked.len() > MAX_SHOWN_ROWS {
                list = list.child(more_row(tracked.len() - MAX_SHOWN_ROWS));
            }
        }
        if !untracked.is_empty() {
            let items: Vec<FileChange> = untracked.iter().map(|(_, c)| c.clone()).collect();
            list = list.child(group("grp-unversioned", "Unversioned Files", untracked_state, items, cx));
            for (i, c) in untracked.iter().take(MAX_SHOWN_ROWS) {
                let inc = g.included(c);
                list = list.child(file_row(*i, c, inc, cx));
            }
            if untracked.len() > MAX_SHOWN_ROWS {
                list = list.child(more_row(untracked.len() - MAX_SHOWN_ROWS));
            }
        }

        let tool = |id: &'static str, glyph: &'static str, tip: &'static str| {
            let tip: SharedString = tip.into();
            div()
                .id(id)
                .px(px(5.))
                .py(px(2.))
                .rounded(px(4.))
                .cursor_pointer()
                .text_color(Theme::text_muted())
                .hover(|s| s.bg(Theme::hover_row()))
                .tooltip(move |_, cx| cx.new(|_| Tip(tip.clone())).into())
                .child(glyph)
        };
        let shortcut = |a: Action| self.resolver.keymap().shortcut_label(a).unwrap_or_default();
        let header = div()
            .h(px(34.))
            .flex()
            .items_center()
            .px(px(12.))
            .gap(px(4.))
            .text_size(px(13.))
            .child(div().font_weight(gpui::FontWeight::SEMIBOLD).text_color(Theme::text()).child("Commit"))
            .child(div().text_color(Theme::text_dim()).text_size(px(12.)).child(format!("  {}", self.branch_label())))
            .child(div().flex_1())
            .child(tool("cm-refresh", "⟳", "Refresh").on_click(cx.listener(|this, _, _, cx| this.refresh_git(cx))))
            .child(tool("cm-rollback", "↶", "Rollback  Ctrl+Alt+Z").on_click(cx.listener(|this, _, window, cx| {
                window.focus(&this.commit_focus);
                this.dispatch(Action::RollbackChanges, window, cx);
            })))
            .child(tool("cm-diff", "⇆", "Show Diff  Ctrl+D").on_click(cx.listener(|this, _, window, cx| {
                if let Some(c) = this.selected_change() {
                    this.show_change_diff(&c, window, cx);
                }
            })));

        let commit_btn = |id: &'static str, label: String, primary: bool, push: bool, cx: &mut Context<Self>| {
            div()
                .id(id)
                .px(px(12.))
                .py(px(5.))
                .rounded(px(5.))
                .text_size(px(13.))
                .cursor_pointer()
                .when(primary, |d| d.bg(Theme::accent()))
                .when(!primary, |d| d.border_1().border_color(Theme::divider()))
                .hover(|s| s.opacity(0.85))
                .child(label)
                .on_click(cx.listener(move |this, _, window, cx| this.do_commit(push, window, cx)))
        };

        div()
            .flex()
            .flex_col()
            .size_full()
            .child(header)
            .child(list)
            .child(
                div()
                    .flex_shrink_0()
                    .flex()
                    .flex_col()
                    .gap(px(6.))
                    .p(px(10.))
                    .border_t_1()
                    .border_color(Theme::divider())
                    .child(
                        div()
                            .id("amend")
                            .flex()
                            .items_center()
                            .gap(px(6.))
                            .text_size(px(13.))
                            .cursor_pointer()
                            .child(checkbox(Some(amend)))
                            .child("Amend")
                            .on_click(cx.listener(|this, _, _, cx| {
                                if let Some(g) = this.git.as_mut() {
                                    g.amend = !g.amend;
                                }
                                cx.notify();
                            })),
                    )
                    .child(
                        div()
                            .h(px(110.))
                            .rounded(px(5.))
                            .border_1()
                            .border_color(Theme::divider())
                            .bg(Theme::editor_bg())
                            .overflow_hidden()
                            .font_family(mono_font())
                            .child(message),
                    )
                    .child(
                        div()
                            .flex()
                            .items_center()
                            .gap(px(8.))
                            .child(commit_btn("btn-commit", if included_n > 0 { format!("Commit ({included_n})") } else { "Commit".into() }, true, false, cx))
                            .child(commit_btn("btn-commit-push", "Commit and Push…".into(), false, true, cx))
                            .child(div().flex_1())
                            .child(div().text_size(px(11.)).text_color(Theme::text_dim()).child(match busy {
                                Some(b) => format!("{b}…"),
                                None => format!("Ctrl+Enter · {}", shortcut(Action::Commit)),
                            })),
                    ),
            )
            .into_any_element()
    }

    pub(super) fn render_git_log(&mut self, cx: &mut Context<Self>) -> gpui::AnyElement {
        let Some(g) = &self.git else {
            return Self::render_placeholder("Git", "Version control is not enabled for this project. Use “Create Git Repository” in the Commit window (Alt+0).");
        };
        let log_focused = g.log_focus.clone();

        // Branches pane.
        let branch_row = |b: &Branch, cx: &mut Context<Self>| {
            let hash = b.hash.clone();
            let b2 = b.clone();
            div()
                .id(SharedString::from(format!("br-{}-{}", b.remote, b.name)))
                .h(px(22.))
                .flex()
                .items_center()
                .gap(px(6.))
                .pl(px(22.))
                .text_size(px(13.))
                .hover(|s| s.bg(Theme::hover_row()))
                .cursor_pointer()
                .on_mouse_down(gpui::MouseButton::Right, {
                    let b3 = b.clone();
                    cx.listener(move |this, e: &gpui::MouseDownEvent, _, cx| this.branch_menu(&b3, e.position, cx))
                })
                .on_click(cx.listener(move |this, e: &ClickEvent, window, cx| {
                    if e.click_count() >= 2 {
                        this.checkout(b2.clone(), cx);
                        return;
                    }
                    let Some(g) = this.git.as_ref() else { return };
                    if let Some(i) = g.log.iter().position(|c| c.hash == hash) {
                        g.log_scroll.scroll_to_item(i, gpui::ScrollStrategy::Center);
                        window.focus(&g.log_focus.clone());
                        this.select_log(i, cx);
                    }
                }))
                .child(div().text_color(if b.current { gpui::rgb(0xe0a85a).into() } else { Theme::text_muted() }).child(if b.current { "★" } else { "⎇" }))
                .child(div().whitespace_nowrap().child(b.name.clone()))
        };
        let mut branches = div().id("git-branches").w(px(200.)).flex_shrink_0().h_full().flex().flex_col().overflow_y_scroll().border_r_1().border_color(Theme::divider()).py(px(4.));
        let head = div().h(px(22.)).flex().items_center().pl(px(10.)).text_size(px(13.)).text_color(Theme::text()).child(format!("HEAD ({})", self.branch_label()));
        branches = branches.child(head);
        let local: Vec<Branch> = g.branches.iter().filter(|b| !b.remote).cloned().collect();
        let remote: Vec<Branch> = g.branches.iter().filter(|b| b.remote).cloned().collect();
        let section = |t: &'static str| div().h(px(22.)).flex().items_center().pl(px(10.)).text_size(px(12.)).text_color(Theme::text_muted()).child(t);
        if !local.is_empty() {
            branches = branches.child(section("▾ Local"));
            for b in &local {
                branches = branches.child(branch_row(b, cx));
            }
        }
        if !remote.is_empty() {
            branches = branches.child(section("▾ Remote"));
            for b in &remote {
                branches = branches.child(branch_row(b, cx));
            }
        }

        // Commit list.
        let n = g.log.len();
        let graph_w = g.graph_w;
        let commits = uniform_list(
            "git-log",
            n,
            cx.processor(move |this, range: std::ops::Range<usize>, window, cx| {
                let Some(g) = this.git.as_ref() else { return Vec::new() };
                let focused = g.log_focus.is_focused(window);
                range
                    .map(|i| {
                        let c = &g.log[i];
                        let row = g.graph.get(i).cloned().unwrap_or_default();
                        let selected = g.log_selected == Some(i);
                        let graph = canvas(
                            |_, _, _| {},
                            move |bounds, _, window, _| {
                                let x = |l: usize| bounds.left() + px(4. + LANE_W * l as f32 + LANE_W / 2.);
                                let top = bounds.top();
                                let mid = bounds.top() + bounds.size.height / 2.;
                                let bot = bounds.bottom();
                                let mut seg = |a: gpui::Point<gpui::Pixels>, b: gpui::Point<gpui::Pixels>, color: usize| {
                                    let mut p = PathBuilder::stroke(px(1.6));
                                    p.move_to(a);
                                    p.line_to(b);
                                    if let Ok(path) = p.build() {
                                        window.paint_path(path, Theme::graph_lane(color));
                                    }
                                };
                                for s in &row.top {
                                    seg(point(x(s.from), top), point(x(s.to), mid), s.color);
                                }
                                for s in &row.bottom {
                                    seg(point(x(s.from), mid), point(x(s.to), bot), s.color);
                                }
                                let cx_ = x(row.node);
                                window.paint_quad(
                                    fill(Bounds::new(point(cx_ - px(4.), mid - px(4.)), size(px(8.), px(8.))), Theme::graph_lane(row.node_color)).corner_radii(px(4.)),
                                );
                            },
                        )
                        .w(px(graph_w))
                        .h(px(LOG_ROW_H))
                        .flex_shrink_0();
                        let refs = c.refs.iter().map(|r| {
                            let (label, color) = if let Some(b) = r.strip_prefix("HEAD -> ") {
                                (b.to_string(), gpui::rgb(0xe0a85a))
                            } else if let Some(t) = r.strip_prefix("tag: ") {
                                (t.to_string(), gpui::rgb(0xb3ae60))
                            } else if r == "HEAD" {
                                ("HEAD".to_string(), gpui::rgb(0xe0a85a))
                            } else {
                                (r.clone(), gpui::rgb(0x6c9be8))
                            };
                            div().px(px(5.)).rounded(px(3.)).border_1().border_color(color).text_color(color).text_size(px(11.)).whitespace_nowrap().child(label)
                        });
                        div()
                            .id(("commit", i))
                            .h(px(LOG_ROW_H))
                            .flex()
                            .items_center()
                            .text_size(px(13.))
                            .w_full()
                            .when(selected, |d| d.bg(if focused { Theme::selection_row() } else { Theme::hover_row() }))
                            .when(!selected, |d| d.hover(|s| s.bg(Theme::hover_row())))
                            .on_click(cx.listener(move |this, _, window, cx| {
                                if let Some(g) = this.git.as_ref() {
                                    window.focus(&g.log_focus.clone());
                                }
                                this.select_log(i, cx);
                            }))
                            .child(graph)
                            .child(div().flex().gap(px(4.)).children(refs))
                            .child(div().pl(px(6.)).flex_1().min_w_0().overflow_hidden().whitespace_nowrap().child(c.subject.clone()))
                            .child(div().w(px(130.)).flex_shrink_0().overflow_hidden().whitespace_nowrap().text_color(Theme::text_muted()).child(c.author.clone()))
                            .child(div().w(px(130.)).flex_shrink_0().whitespace_nowrap().text_color(Theme::text_muted()).text_size(px(12.)).child(fmt_time(c.time)))
                    })
                    .collect()
            }),
        )
        .track_scroll(g.log_scroll.clone())
        .flex_1();
        let center = div()
            .id("git-log-pane")
            .track_focus(&log_focused)
            .key_context("GitLog")
            .flex_1()
            .min_w_0()
            .h_full()
            .flex()
            .flex_col()
            .when(n == 0, |d| {
                d.child(div().p(px(12.)).text_size(px(13.)).text_color(Theme::text_muted()).child(if !g.loaded {
                    "Loading…"
                } else if g.filter.query.is_empty() {
                    "No commits yet"
                } else {
                    "No commits match the filters"
                }))
            })
            .child(commits);

        // Details pane.
        let mut details = div().id("git-details").w(px(340.)).flex_shrink_0().h_full().flex().flex_col().overflow_y_scroll().border_l_1().border_color(Theme::divider()).p(px(10.)).gap(px(6.)).text_size(px(13.));
        if let (Some(d), Some(c)) = (&g.details, g.log_selected.and_then(|i| g.log.get(i))) {
            let hash = d.hash.clone();
            details = details
                .child(div().text_color(Theme::text()).child(d.message.clone()))
                .child(div().text_color(Theme::text_muted()).text_size(px(12.)).child(format!("{} · {} <{}> · {}", &c.hash[..8.min(c.hash.len())], c.author, c.email, fmt_time(c.time))))
                .child(div().pt(px(6.)).text_color(Theme::text_muted()).text_size(px(12.)).child(format!("{} file{} changed", d.files.len(), if d.files.len() == 1 { "" } else { "s" })));
            let dsel = g.details_sel;
            for (fi, f) in d.files.iter().enumerate() {
                let f2 = f.clone();
                let h2 = hash.clone();
                let (icon, icolor) = file_icon(file_name(&f.path), false);
                details = details.child(
                    div()
                        .id(("dfile", fi))
                        .h(px(22.))
                        .flex()
                        .items_center()
                        .gap(px(6.))
                        .rounded(px(4.))
                        .px(px(4.))
                        .when(dsel == Some(fi), |d| d.bg(Theme::selection_row()))
                        .when(!(dsel == Some(fi)), |d| d.hover(|s| s.bg(Theme::hover_row())))
                        .cursor_pointer()
                        .on_mouse_down(gpui::MouseButton::Right, {
                            let (h, f) = (hash.clone(), f.clone());
                            cx.listener(move |this, e: &gpui::MouseDownEvent, _, cx| {
                                if let Some(g) = this.git.as_mut() {
                                    g.details_sel = Some(fi);
                                }
                                this.log_file_menu(h.clone(), f.clone(), e.position, cx);
                            })
                        })
                        .on_click(cx.listener(move |this, e: &ClickEvent, window, cx| {
                            if let Some(g) = this.git.as_mut() {
                                g.details_sel = Some(fi);
                            }
                            if e.click_count() >= 2 {
                                this.show_commit_diff(&h2, &f2, window, cx);
                            }
                            cx.notify();
                        }))
                        .child(div().w(px(14.)).text_color(icolor).child(icon))
                        .child(div().text_color(status_color(f.status)).whitespace_nowrap().child(file_name(&f.path).to_string()))
                        .child(div().text_color(Theme::text_dim()).text_size(px(12.)).whitespace_nowrap().child(dir_name(&f.path).to_string())),
                );
            }
        }

        div()
            .size_full()
            .flex()
            .flex_col()
            .child(
                div()
                    .h(px(30.))
                    .flex()
                    .items_center()
                    .px(px(12.))
                    .gap(px(12.))
                    .text_size(px(13.))
                    .border_b_1()
                    .border_color(Theme::divider())
                    .child(div().font_weight(gpui::FontWeight::SEMIBOLD).child("Git"))
                    .child(div().text_color(Theme::text()).child("Log"))
                    .child(self.render_log_filters(cx))
                    .child(div().flex_1())
                    .child(div().text_color(Theme::text_dim()).text_size(px(12.)).child(format!("{n} commits")))
                    .child(
                        div().id("log-refresh").px(px(5.)).rounded(px(4.)).cursor_pointer().text_color(Theme::text_muted()).hover(|s| s.bg(Theme::hover_row())).child("⟳")
                            .on_click(cx.listener(|this, _, _, cx| this.refresh_git(cx))),
                    ),
            )
            .child(div().flex_1().min_h_0().flex().child(branches).child(center).child(details))
            .into_any_element()
    }
}
