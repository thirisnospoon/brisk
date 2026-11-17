//! Default PyCharm keymap for Windows / Linux ("Windows" keymap in IntelliJ terms).

use crate::{Action as A, Keymap};

const ED: Option<&str> = Some("Editor");
const PV: Option<&str> = Some("ProjectView");

pub fn pycharm_keymap() -> Keymap {
    let mut k = Keymap::default();

    // --- Navigation ---------------------------------------------------------
    k.bind_double_tap("shift", A::SearchEverywhere);
    k.bind("ctrl-shift-a", A::FindAction, None);
    k.bind("ctrl-n", A::GotoClass, None);
    k.bind("ctrl-shift-n", A::GotoFile, None);
    k.bind("ctrl-alt-shift-n", A::GotoSymbol, None);
    k.bind("ctrl-e", A::RecentFiles, None);
    k.bind("ctrl-shift-e", A::RecentLocations, None);
    k.bind("ctrl-g", A::GotoLine, ED);
    k.bind("ctrl-b", A::GotoDeclaration, ED);
    k.bind("ctrl-alt-b", A::GotoImplementation, ED);
    k.bind("ctrl-shift-b", A::GotoTypeDeclaration, ED);
    k.bind("ctrl-u", A::GotoSuperMethod, ED);
    k.bind("ctrl-alt-left", A::NavigateBack, None);
    k.bind("ctrl-alt-right", A::NavigateForward, None);
    k.bind("alt-down", A::NextMethod, ED);
    k.bind("alt-up", A::PreviousMethod, ED);
    k.bind("f2", A::NextError, ED);
    k.bind("shift-f2", A::PreviousError, ED);
    k.bind("ctrl-f12", A::FileStructurePopup, ED);
    k.bind("alt-f1", A::SelectIn, None);
    k.bind("ctrl-shift-i", A::QuickDefinition, ED);
    k.bind("ctrl-q", A::QuickDocumentation, None);
    k.bind("ctrl-p", A::ParameterInfo, ED);
    k.bind("alt-f7", A::FindUsages, None);
    k.bind("ctrl-alt-f7", A::ShowUsages, ED);
    k.bind("f11", A::ToggleBookmark, None);
    k.bind("ctrl-f11", A::ToggleBookmarkWithMnemonic, None);
    k.bind("shift-f11", A::ShowBookmarks, None);

    // --- Find / replace -----------------------------------------------------
    k.bind("ctrl-f", A::Find, ED);
    k.bind("ctrl-r", A::Replace, ED);
    k.bind("ctrl-shift-f", A::FindInFiles, None);
    k.bind("ctrl-shift-r", A::ReplaceInFiles, None);
    k.bind("f3", A::FindNext, ED);
    k.bind("shift-f3", A::FindPrevious, ED);

    // --- Code ---------------------------------------------------------------
    k.bind("alt-enter", A::ShowIntentionActions, None);
    k.bind("ctrl-space", A::CodeCompletion, ED);
    k.bind("ctrl-shift-space", A::SmartCompletion, ED);
    k.bind("ctrl-shift-enter", A::CompleteStatement, ED);
    k.bind("ctrl-j", A::InsertLiveTemplate, ED);
    k.bind("ctrl-alt-t", A::SurroundWith, ED);
    k.bind("alt-insert", A::Generate, ED);
    k.bind("alt-insert", A::NewElement, PV);
    k.bind("ctrl-/", A::CommentLine, ED);
    k.bind("ctrl-shift-/", A::CommentBlock, ED);
    k.bind("ctrl-alt-l", A::ReformatCode, ED);
    k.bind("ctrl-alt-o", A::OptimizeImports, None);
    k.bind("ctrl-=", A::ExpandRegion, ED);
    k.bind("ctrl--", A::CollapseRegion, ED);
    k.bind("ctrl-shift-=", A::ExpandAll, ED);
    k.bind("ctrl-shift--", A::CollapseAll, ED);

    // --- Refactor -----------------------------------------------------------
    k.bind("ctrl-alt-shift-t", A::RefactorThis, None);
    k.bind("shift-f6", A::Rename, None);
    k.bind("ctrl-f6", A::ChangeSignature, ED);
    k.bind("ctrl-alt-v", A::ExtractVariable, ED);
    k.bind("ctrl-alt-m", A::ExtractMethod, ED);
    k.bind("ctrl-alt-c", A::ExtractConstant, ED);
    k.bind("ctrl-alt-n", A::Inline, ED);
    k.bind("f6", A::Move, None);
    k.bind("alt-delete", A::SafeDelete, None);

    // --- Editing ------------------------------------------------------------
    k.bind("ctrl-z", A::Undo, None);
    k.bind("ctrl-shift-z", A::Redo, None);
    k.bind("ctrl-x", A::Cut, ED);
    k.bind("ctrl-c", A::Copy, None);
    k.bind("ctrl-v", A::Paste, None);
    k.bind("ctrl-shift-v", A::PasteFromHistory, ED);
    k.bind("ctrl-shift-c", A::CopyPath, None);
    k.bind("ctrl-a", A::SelectAll, None);
    k.bind("ctrl-d", A::DuplicateLine, ED);
    k.bind("ctrl-y", A::DeleteLine, ED);
    k.bind("ctrl-shift-j", A::JoinLines, ED);
    k.bind("ctrl-shift-u", A::ToggleCase, ED);
    k.bind("alt-shift-up", A::MoveLineUp, ED);
    k.bind("alt-shift-down", A::MoveLineDown, ED);
    k.bind("ctrl-shift-up", A::MoveStatementUp, ED);
    k.bind("ctrl-shift-down", A::MoveStatementDown, ED);
    k.bind("ctrl-w", A::ExtendSelection, ED);
    k.bind("ctrl-shift-w", A::ShrinkSelection, ED);
    k.bind("alt-j", A::AddCaretNextOccurrence, ED);
    k.bind("alt-shift-j", A::UnselectOccurrence, ED);
    k.bind("ctrl-alt-shift-j", A::SelectAllOccurrences, ED);
    // CloneCaretAbove/Below: "Ctrl, Ctrl+Up/Down" (double Ctrl, held) is detected by the workspace.
    k.bind("shift-enter", A::StartNewLine, ED);
    k.bind("ctrl-alt-enter", A::StartNewLineBefore, ED);
    k.bind("enter", A::EditorNewline, ED);
    k.bind("backspace", A::EditorBackspace, ED);
    k.bind("shift-backspace", A::EditorBackspace, ED);
    k.bind("delete", A::EditorDelete, ED);
    k.bind("tab", A::EditorTab, ED);
    k.bind("shift-tab", A::EditorUnindent, ED);
    k.bind("ctrl-backspace", A::EditorDeleteWordBack, ED);
    k.bind("ctrl-delete", A::EditorDeleteWordForward, ED);
    k.bind("escape", A::EditorEscape, ED);

    // --- Caret --------------------------------------------------------------
    k.bind("left", A::MoveLeft, ED);
    k.bind("right", A::MoveRight, ED);
    k.bind("up", A::MoveUp, ED);
    k.bind("down", A::MoveDown, ED);
    k.bind("ctrl-left", A::MoveWordLeft, ED);
    k.bind("ctrl-right", A::MoveWordRight, ED);
    k.bind("home", A::MoveLineStart, ED);
    k.bind("end", A::MoveLineEnd, ED);
    k.bind("ctrl-home", A::MoveDocStart, ED);
    k.bind("ctrl-end", A::MoveDocEnd, ED);
    k.bind("pageup", A::PageUp, ED);
    k.bind("pagedown", A::PageDown, ED);
    k.bind("shift-left", A::SelectLeft, ED);
    k.bind("shift-right", A::SelectRight, ED);
    k.bind("shift-up", A::SelectUp, ED);
    k.bind("shift-down", A::SelectDown, ED);
    k.bind("ctrl-shift-left", A::SelectWordLeft, ED);
    k.bind("ctrl-shift-right", A::SelectWordRight, ED);
    k.bind("shift-home", A::SelectLineStart, ED);
    k.bind("shift-end", A::SelectLineEnd, ED);
    k.bind("ctrl-shift-home", A::SelectDocStart, ED);
    k.bind("ctrl-shift-end", A::SelectDocEnd, ED);
    k.bind("ctrl-up", A::ScrollUp, ED);
    k.bind("ctrl-down", A::ScrollDown, ED);

    // --- Files and tabs -----------------------------------------------------
    k.bind("ctrl-s", A::SaveAll, None);
    k.bind("ctrl-f4", A::CloseTab, None);
    k.bind("alt-right", A::NextTab, None);
    k.bind("alt-left", A::PreviousTab, None);
    k.bind("ctrl-alt-s", A::Settings, None);
    k.bind("alt-shift-.", A::IncreaseFontSize, None);
    k.bind("alt-shift-,", A::DecreaseFontSize, None);
    k.bind("ctrl-alt-shift-s", A::ProjectStructure, None);
    k.bind("ctrl-alt-y", A::ReloadFromDisk, None);
    k.bind("enter", A::OpenSelected, PV);
    k.bind("f4", A::OpenSelected, PV);

    // --- Run / Debug --------------------------------------------------------
    k.bind("shift-f10", A::Run, None);
    k.bind("shift-f9", A::Debug, None);
    k.bind("ctrl-shift-f10", A::RunContext, None);
    k.bind("ctrl-shift-f9", A::DebugContext, None);
    k.bind("ctrl-f2", A::Stop, None);
    k.bind("alt-shift-f10", A::SelectRunConfiguration, None);
    k.bind("ctrl-f5", A::Rerun, Some("RunConsole"));
    k.bind("ctrl-f8", A::ToggleBreakpoint, ED);
    k.bind("ctrl-shift-f8", A::ViewBreakpoints, None);
    k.bind("f8", A::StepOver, None);
    k.bind("f7", A::StepInto, None);
    k.bind("shift-f8", A::StepOut, None);
    k.bind("f9", A::Resume, None);
    k.bind("alt-f9", A::RunToCursor, None);
    k.bind("alt-f8", A::EvaluateExpression, None);

    // --- Git ----------------------------------------------------------------
    k.bind("ctrl-k", A::Commit, None);
    k.bind("ctrl-shift-k", A::Push, None);
    k.bind("ctrl-t", A::UpdateProject, None);
    k.bind("alt-`", A::VcsQuickList, None);
    k.bind("ctrl-shift-`", A::Branches, None);
    k.bind("ctrl-alt-z", A::RollbackChanges, None);
    k.bind("ctrl-d", A::ShowDiff, Some("CommitView"));
    k.bind("ctrl-d", A::ShowDiff, Some("GitLog"));

    // --- Database (PyCharm Professional / DataGrip) ----------------------------
    const DB: Option<&str> = Some("DatabaseView");
    const SQL: Option<&str> = Some("SqlConsole");
    const GRID: Option<&str> = Some("DataGrid");
    k.bind("ctrl-enter", A::ExecuteQuery, SQL);
    k.bind("ctrl-shift-f10", A::OpenQueryConsole, DB);
    k.bind("ctrl-shift-f10", A::OpenQueryConsole, SQL);
    k.bind("alt-insert", A::NewDataSource, DB);
    k.bind("shift-enter", A::DataSourceProperties, DB);
    k.bind("ctrl-alt-shift-s", A::DataSourceProperties, DB);
    k.bind("f4", A::EditTableData, DB);
    k.bind("ctrl-f5", A::RefreshDatabase, DB);
    k.bind("ctrl-f5", A::RefreshDatabase, GRID);
    k.bind("ctrl-f2", A::Disconnect, DB);

    // --- Tool windows -------------------------------------------------------
    k.bind("alt-1", A::ToggleProjectWindow, None);
    k.bind("alt-2", A::ToggleBookmarksWindow, None);
    k.bind("alt-3", A::ToggleFindWindow, None);
    k.bind("alt-4", A::ToggleRunWindow, None);
    k.bind("alt-5", A::ToggleDebugWindow, None);
    k.bind("alt-6", A::ToggleProblemsWindow, None);
    k.bind("alt-7", A::ToggleStructureWindow, None);
    k.bind("alt-8", A::ToggleServicesWindow, None);
    k.bind("alt-9", A::ToggleGitWindow, None);
    k.bind("alt-0", A::ToggleCommitWindow, None);
    k.bind("alt-f12", A::ToggleTerminalWindow, None);
    k.bind("ctrl-shift-f12", A::HideAllToolWindows, None);
    k.bind("shift-escape", A::HideActiveToolWindow, None);
    k.bind("escape", A::FocusEditor, Some("ToolWindow"));

    k
}
