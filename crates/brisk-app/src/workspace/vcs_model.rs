//! The version control state of a workspace (docs/architecture.md, step 17): the repository
//! and the state of the Git and Commit windows (`git_ui::Git`), the branch read when the
//! project opened, and the Commit window's focus. It is an entity created on first use. For
//! now the methods stay on `Workspace` and reach the state through the accessors below.

use super::git_ui::Git;
use super::Workspace;
use gpui::{App, AppContext, Entity, FocusHandle, GpuiBorrow, Window};

pub struct VcsModel {
    /// The repository, once it is found or created.
    pub git: Option<Git>,
    /// The branch in `.git/HEAD` when the project opened (the status bar shows a branch
    /// widget from the start).
    pub branch: Option<String>,
    /// Create Git Repository is running.
    pub init_running: bool,
    /// The Commit window's focus. It exists without a repository too: the window then offers
    /// Create Git Repository.
    pub commit_focus: FocusHandle,
}

impl VcsModel {
    pub fn new(branch: Option<String>, cx: &mut App) -> Entity<Self> {
        cx.new(|cx| VcsModel { git: None, branch, init_running: false, commit_focus: cx.focus_handle() })
    }
}

/// A part of the VCS state borrowed for writing (`Workspace::git_mut`, the dialog
/// accessors). Like any entity update, it holds the app context until it is dropped.
pub struct VcsMut<'a, T> {
    borrow: GpuiBorrow<'a, VcsModel>,
    get: fn(&VcsModel) -> Option<&T>,
    get_mut: fn(&mut VcsModel) -> Option<&mut T>,
}

pub type GitMut<'a> = VcsMut<'a, Git>;

impl<T> std::ops::Deref for VcsMut<'_, T> {
    type Target = T;
    fn deref(&self) -> &T {
        (self.get)(&self.borrow).expect("checked when borrowed")
    }
}

impl<T> std::ops::DerefMut for VcsMut<'_, T> {
    fn deref_mut(&mut self) -> &mut T {
        (self.get_mut)(&mut self.borrow).expect("checked when borrowed")
    }
}

impl Workspace {
    /// The VCS model, created on first use.
    pub(super) fn vcs(&mut self, cx: &mut App) -> Entity<VcsModel> {
        if let Some(v) = &self.vcs {
            return v.clone();
        }
        let v = VcsModel::new(None, cx);
        self.vcs = Some(v.clone());
        v
    }

    /// The repository state, if there is a repository.
    pub(super) fn git<'a>(&self, cx: &'a App) -> Option<&'a Git> {
        self.vcs.as_ref()?.read(cx).git.as_ref()
    }

    /// The repository state for writing, if there is a repository.
    pub(super) fn git_mut<'a>(&self, cx: &'a mut App) -> Option<GitMut<'a>> {
        self.vcs_mut(cx, |m| m.git.as_ref(), |m| m.git.as_mut())
    }

    /// The part of the VCS state that `get` / `get_mut` pick, for writing, if it is there.
    pub(super) fn vcs_mut<'a, T>(
        &self,
        cx: &'a mut App,
        get: fn(&VcsModel) -> Option<&T>,
        get_mut: fn(&mut VcsModel) -> Option<&mut T>,
    ) -> Option<VcsMut<'a, T>> {
        let v = self.vcs.as_ref()?;
        get(v.read(cx))?;
        Some(VcsMut { borrow: v.as_mut(cx), get, get_mut })
    }

    pub(super) fn set_git(&mut self, git: Option<Git>, cx: &mut App) {
        let v = self.vcs(cx);
        v.update(cx, |m, _| m.git = git);
    }

    /// The branch read from `.git/HEAD` when the project opened.
    pub(super) fn start_branch(&self, cx: &App) -> Option<String> {
        self.vcs.as_ref()?.read(cx).branch.clone()
    }

    pub(super) fn git_init_running(&self, cx: &App) -> bool {
        self.vcs.as_ref().is_some_and(|v| v.read(cx).init_running)
    }

    pub(super) fn set_git_init_running(&mut self, running: bool, cx: &mut App) {
        let v = self.vcs(cx);
        v.update(cx, |m, _| m.init_running = running);
    }

    pub(super) fn commit_focus(&mut self, cx: &mut App) -> FocusHandle {
        self.vcs(cx).read(cx).commit_focus.clone()
    }

    pub(super) fn commit_focused(&self, window: &Window, cx: &App) -> bool {
        self.vcs.as_ref().is_some_and(|v| v.read(cx).commit_focus.is_focused(window))
    }
}
