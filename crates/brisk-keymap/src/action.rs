/// Static description of an action, shown in Find Action (Ctrl+Shift+A) and menus.
#[derive(Clone, Copy, Debug)]
pub struct ActionInfo {
    pub id: &'static str,
    pub title: &'static str,
    pub group: &'static str,
}

macro_rules! actions {
    ($( $variant:ident => $title:literal, $group:literal; )*) => {
        #[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
        pub enum Action { $( $variant, )* }

        pub const ALL_ACTIONS: &[Action] = &[ $( Action::$variant, )* ];

        impl Action {
            pub fn info(self) -> ActionInfo {
                match self {
                    $( Action::$variant => ActionInfo {
                        id: stringify!($variant),
                        title: $title,
                        group: $group,
                    }, )*
                }
            }

            pub fn from_id(id: &str) -> Option<Self> {
                match id {
                    $( stringify!($variant) => Some(Action::$variant), )*
                    _ => None,
                }
            }
        }
    };
}

actions! {
    // Navigation
    SearchEverywhere => "Search Everywhere", "Navigate";
    FindAction => "Find Action", "Navigate";
    GotoClass => "Go to Class", "Navigate";
    GotoFile => "Go to File", "Navigate";
    GotoSymbol => "Go to Symbol", "Navigate";
    RecentFiles => "Recent Files", "Navigate";
    RecentLocations => "Recent Locations", "Navigate";
    GotoLine => "Go to Line:Column", "Navigate";
    GotoDeclaration => "Go to Declaration or Usages", "Navigate";
    GotoImplementation => "Go to Implementation(s)", "Navigate";
    GotoTypeDeclaration => "Go to Type Declaration", "Navigate";
    GotoSuperMethod => "Go to Super Method", "Navigate";
    NavigateBack => "Back", "Navigate";
    NavigateForward => "Forward", "Navigate";
    NextMethod => "Next Method", "Navigate";
    PreviousMethod => "Previous Method", "Navigate";
    NextError => "Next Highlighted Error", "Navigate";
    PreviousError => "Previous Highlighted Error", "Navigate";
    FileStructurePopup => "File Structure", "Navigate";
    SelectIn => "Select In...", "Navigate";
    QuickDefinition => "Quick Definition", "Navigate";
    QuickDocumentation => "Quick Documentation", "Navigate";
    ParameterInfo => "Parameter Info", "Navigate";
    FindUsages => "Find Usages", "Navigate";
    ShowUsages => "Show Usages", "Navigate";
    ToggleBookmark => "Toggle Bookmark", "Navigate";
    ToggleBookmarkWithMnemonic => "Toggle Bookmark with Mnemonic", "Navigate";
    ShowBookmarks => "Show Bookmarks", "Navigate";

    // Find / replace
    Find => "Find", "Find";
    Replace => "Replace", "Find";
    FindInFiles => "Find in Files", "Find";
    ReplaceInFiles => "Replace in Files", "Find";
    FindNext => "Find Next Occurrence", "Find";
    FindPrevious => "Find Previous Occurrence", "Find";

    // Code
    ShowIntentionActions => "Show Context Actions", "Code";
    CodeCompletion => "Basic Completion", "Code";
    SmartCompletion => "Type-Matching Completion", "Code";
    CompleteStatement => "Complete Current Statement", "Code";
    InsertLiveTemplate => "Insert Live Template", "Code";
    SurroundWith => "Surround With", "Code";
    Generate => "Generate...", "Code";
    CommentLine => "Comment with Line Comment", "Code";
    CommentBlock => "Comment with Block Comment", "Code";
    ReformatCode => "Reformat Code", "Code";
    OptimizeImports => "Optimize Imports", "Code";
    ExpandRegion => "Expand", "Code";
    CollapseRegion => "Collapse", "Code";
    ExpandAll => "Expand All", "Code";
    CollapseAll => "Collapse All", "Code";

    // Refactor
    RefactorThis => "Refactor This", "Refactor";
    Rename => "Rename", "Refactor";
    ChangeSignature => "Change Signature", "Refactor";
    ExtractVariable => "Extract Variable", "Refactor";
    ExtractMethod => "Extract Method", "Refactor";
    ExtractConstant => "Extract Constant", "Refactor";
    Inline => "Inline", "Refactor";
    Move => "Move", "Refactor";
    SafeDelete => "Safe Delete", "Refactor";

    // Editing
    Undo => "Undo", "Edit";
    Redo => "Redo", "Edit";
    Cut => "Cut", "Edit";
    Copy => "Copy", "Edit";
    Paste => "Paste", "Edit";
    PasteFromHistory => "Paste from History", "Edit";
    CopyPath => "Copy Path/Reference", "Edit";
    SelectAll => "Select All", "Edit";
    DuplicateLine => "Duplicate Line or Selection", "Edit";
    DeleteLine => "Delete Line", "Edit";
    JoinLines => "Join Lines", "Edit";
    ToggleCase => "Toggle Case", "Edit";
    MoveLineUp => "Move Line Up", "Edit";
    MoveLineDown => "Move Line Down", "Edit";
    MoveStatementUp => "Move Statement Up", "Edit";
    MoveStatementDown => "Move Statement Down", "Edit";
    ExtendSelection => "Extend Selection", "Edit";
    ShrinkSelection => "Shrink Selection", "Edit";
    AddCaretNextOccurrence => "Add Selection for Next Occurrence", "Edit";
    UnselectOccurrence => "Unselect Occurrence", "Edit";
    SelectAllOccurrences => "Select All Occurrences", "Edit";
    CloneCaretAbove => "Clone Caret Above", "Edit";
    CloneCaretBelow => "Clone Caret Below", "Edit";
    StartNewLine => "Start New Line", "Edit";
    StartNewLineBefore => "Start New Line Before Current", "Edit";
    EditorNewline => "Enter", "Edit";
    EditorBackspace => "Backspace", "Edit";
    EditorDelete => "Delete", "Edit";
    EditorTab => "Tab", "Edit";
    EditorUnindent => "Unindent", "Edit";
    EditorDeleteWordBack => "Delete to Word Start", "Edit";
    EditorDeleteWordForward => "Delete to Word End", "Edit";
    EditorEscape => "Escape", "Edit";

    // Caret movement
    MoveLeft => "Left", "Caret";
    MoveRight => "Right", "Caret";
    MoveUp => "Up", "Caret";
    MoveDown => "Down", "Caret";
    MoveWordLeft => "Move Caret to Previous Word", "Caret";
    MoveWordRight => "Move Caret to Next Word", "Caret";
    MoveLineStart => "Move Caret to Line Start", "Caret";
    MoveLineEnd => "Move Caret to Line End", "Caret";
    MoveDocStart => "Move Caret to Text Start", "Caret";
    MoveDocEnd => "Move Caret to Text End", "Caret";
    PageUp => "Page Up", "Caret";
    PageDown => "Page Down", "Caret";
    SelectLeft => "Left with Selection", "Caret";
    SelectRight => "Right with Selection", "Caret";
    SelectUp => "Up with Selection", "Caret";
    SelectDown => "Down with Selection", "Caret";
    SelectWordLeft => "Previous Word with Selection", "Caret";
    SelectWordRight => "Next Word with Selection", "Caret";
    SelectLineStart => "Line Start with Selection", "Caret";
    SelectLineEnd => "Line End with Selection", "Caret";
    SelectDocStart => "Text Start with Selection", "Caret";
    SelectDocEnd => "Text End with Selection", "Caret";
    ScrollUp => "Scroll Up", "Caret";
    ScrollDown => "Scroll Down", "Caret";

    // Files and tabs
    SaveAll => "Save All", "File";
    OpenProject => "Open...", "File";
    NewProject => "New Project...", "File";
    RecentProjects => "Recent Projects", "File";
    NewElement => "New...", "File";
    CloseTab => "Close Tab", "File";
    NextTab => "Select Next Tab", "File";
    PreviousTab => "Select Previous Tab", "File";
    CloseOtherTabs => "Close Other Tabs", "File";
    CloseAllTabs => "Close All Tabs", "File";
    CloseTabsToRight => "Close Tabs to the Right", "File";
    CloseTabsToLeft => "Close Tabs to the Left", "File";
    ReopenClosedTab => "Reopen Closed Tab", "File";
    SplitVertically => "Split Right", "Window";
    SplitHorizontally => "Split Down", "Window";
    Unsplit => "Unsplit", "Window";
    MoveToOppositeGroup => "Move to Opposite Group", "Window";
    NextSplitter => "Go to Next Splitter", "Window";
    Settings => "Settings", "File";
    IncreaseFontSize => "Increase Font Size in All Editors", "View";
    DecreaseFontSize => "Decrease Font Size in All Editors", "View";
    ResetFontSize => "Reset Font Size", "View";
    QuickSwitchTheme => "Switch Light / Dark Theme", "View";
    ProjectStructure => "Project Structure", "File";
    ReloadFromDisk => "Reload All from Disk", "File";
    OpenSelected => "Open / Jump to Source", "File";

    // Run / Debug
    Run => "Run", "Run";
    Debug => "Debug", "Run";
    RunContext => "Run Context Configuration", "Run";
    DebugContext => "Debug Context Configuration", "Run";
    Stop => "Stop", "Run";
    Rerun => "Rerun", "Run";
    SelectRunConfiguration => "Run...", "Run";
    EditRunConfigurations => "Edit Configurations", "Run";
    ToggleBreakpoint => "Toggle Line Breakpoint", "Run";
    ViewBreakpoints => "View Breakpoints", "Run";
    StepOver => "Step Over", "Run";
    StepInto => "Step Into", "Run";
    StepOut => "Step Out", "Run";
    Resume => "Resume Program", "Run";
    RunToCursor => "Run to Cursor", "Run";
    EvaluateExpression => "Evaluate Expression", "Run";
    PauseProgram => "Pause Program", "Run";
    RerunDebug => "Rerun Debug Session", "Run";

    // VCS
    Commit => "Commit", "Git";
    Push => "Push", "Git";
    UpdateProject => "Update Project", "Git";
    VcsQuickList => "VCS Operations", "Git";
    Branches => "Branches", "Git";
    ShowDiff => "Show Diff", "Git";
    RollbackChanges => "Rollback", "Git";
    Annotate => "Annotate with Git Blame", "Git";
    ShowLocalHistory => "Show Local History", "Git";

    // Database
    ExecuteQuery => "Execute", "Database";
    NewDataSource => "New Data Source", "Database";
    DataSourceProperties => "Data Source Properties", "Database";
    OpenQueryConsole => "Jump to Query Console", "Database";
    EditTableData => "Edit Data", "Database";
    RefreshDatabase => "Refresh Database", "Database";
    Disconnect => "Disconnect", "Database";

    // Tool windows
    ToggleDatabaseWindow => "Database", "Tool Windows";
    ToggleProjectWindow => "Project", "Tool Windows";
    ToggleBookmarksWindow => "Bookmarks", "Tool Windows";
    ToggleFindWindow => "Find", "Tool Windows";
    ToggleRunWindow => "Run", "Tool Windows";
    ToggleDebugWindow => "Debug", "Tool Windows";
    ToggleProblemsWindow => "Problems", "Tool Windows";
    ToggleStructureWindow => "Structure", "Tool Windows";
    ToggleServicesWindow => "Services", "Tool Windows";
    ToggleGitWindow => "Git", "Tool Windows";
    ToggleCommitWindow => "Commit", "Tool Windows";
    ToggleTerminalWindow => "Terminal", "Tool Windows";
    HideAllToolWindows => "Hide All Tool Windows", "Tool Windows";
    HideActiveToolWindow => "Hide Active Tool Window", "Tool Windows";
    FocusEditor => "Focus Editor", "Tool Windows";
}
