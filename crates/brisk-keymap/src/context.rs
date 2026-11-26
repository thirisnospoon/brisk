//! Where a shortcut applies: the part of the window that has the keyboard focus.

use std::fmt;

/// A focus context of the keymap. A binding with a context fires only while that context is
/// focused; `Workspace::contexts` lists the focused ones, innermost first.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum KeyContext {
    Editor,
    /// An editor on a database console (also `Editor`).
    SqlConsole,
    ProjectView,
    CommitView,
    GitLog,
    FindView,
    ProblemsView,
    DatabaseView,
    DataGrid,
    DiffView,
    RunConsole,
    Terminal,
    /// Any tool window (also its own context, such as `ProjectView`).
    ToolWindow,
}

impl KeyContext {
    pub const ALL: [KeyContext; 13] = [
        KeyContext::Editor,
        KeyContext::SqlConsole,
        KeyContext::ProjectView,
        KeyContext::CommitView,
        KeyContext::GitLog,
        KeyContext::FindView,
        KeyContext::ProblemsView,
        KeyContext::DatabaseView,
        KeyContext::DataGrid,
        KeyContext::DiffView,
        KeyContext::RunConsole,
        KeyContext::Terminal,
        KeyContext::ToolWindow,
    ];

    /// The name in keymap files and PyCharm's: "Editor", "ProjectView".
    pub fn as_str(self) -> &'static str {
        match self {
            KeyContext::Editor => "Editor",
            KeyContext::SqlConsole => "SqlConsole",
            KeyContext::ProjectView => "ProjectView",
            KeyContext::CommitView => "CommitView",
            KeyContext::GitLog => "GitLog",
            KeyContext::FindView => "FindView",
            KeyContext::ProblemsView => "ProblemsView",
            KeyContext::DatabaseView => "DatabaseView",
            KeyContext::DataGrid => "DataGrid",
            KeyContext::DiffView => "DiffView",
            KeyContext::RunConsole => "RunConsole",
            KeyContext::Terminal => "Terminal",
            KeyContext::ToolWindow => "ToolWindow",
        }
    }

    /// The context named `s`; `None` for a name Brisk does not know, which a keymap file
    /// skips instead of failing.
    pub fn parse(s: &str) -> Option<Self> {
        Self::ALL.into_iter().find(|c| c.as_str() == s.trim())
    }
}

impl fmt::Display for KeyContext {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(self.as_str())
    }
}

#[cfg(test)]
mod tests {
    use super::KeyContext;

    #[test]
    fn names_round_trip() {
        for c in KeyContext::ALL {
            assert_eq!(KeyContext::parse(c.as_str()), Some(c));
            assert_eq!(KeyContext::parse(&c.to_string()), Some(c));
        }
        let mut names: Vec<&str> = KeyContext::ALL.iter().map(|c| c.as_str()).collect();
        names.sort_unstable();
        names.dedup();
        assert_eq!(names.len(), KeyContext::ALL.len());
    }

    #[test]
    fn unknown_names_are_none() {
        assert_eq!(KeyContext::parse("NoSuchView"), None);
        assert_eq!(KeyContext::parse(""), None);
        assert_eq!(KeyContext::parse("editor"), None);
    }
}
