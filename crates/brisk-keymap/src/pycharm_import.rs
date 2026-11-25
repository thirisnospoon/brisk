//! Import of a PyCharm keymap: the custom keymaps PyCharm keeps as XML in
//! `%APPDATA%\JetBrains\PyCharm<version>\keymaps`, and which one is active.

use crate::{user::chord_id, Action, Keystroke, Modifiers, UserKeymap};
use std::path::{Path, PathBuf};

/// PyCharm's action id for ours, where they differ.
fn our_id(pycharm: &str) -> Option<&'static str> {
    Some(match pycharm {
        "GotoAction" => "FindAction",
        "JumpToLastChange" => "LastEditLocation",
        "Back" => "NavigateBack",
        "Forward" => "NavigateForward",
        "MethodDown" => "NextMethod",
        "MethodUp" => "PreviousMethod",
        "GotoNextError" => "NextError",
        "GotoPreviousError" => "PreviousError",
        "QuickImplementations" => "QuickDefinition",
        "QuickJavaDoc" => "QuickDocumentation",
        "ExpressionTypeInfo" => "TypeInfo",
        "FindInPath" => "FindInFiles",
        "ReplaceInPath" => "ReplaceInFiles",
        "SmartTypeCompletion" => "SmartCompletion",
        "EditorCompleteStatement" => "CompleteStatement",
        "CommentByLineComment" => "CommentLine",
        "CommentByBlockComment" => "CommentBlock",
        "ExpandAllRegions" => "ExpandAll",
        "CollapseAllRegions" => "CollapseAll",
        "Refactorings.QuickListPopupAction" => "RefactorThis",
        "RenameElement" => "Rename",
        "IntroduceVariable" => "ExtractVariable",
        "IntroduceConstant" => "ExtractConstant",
        "$Undo" => "Undo",
        "$Redo" => "Redo",
        "$Cut" => "Cut",
        "$Copy" => "Copy",
        "$Paste" => "Paste",
        "$SelectAll" => "SelectAll",
        "PasteMultiple" => "PasteFromHistory",
        "CopyPaths" => "CopyPath",
        "$Delete" => "DeleteElement",
        "EditorDuplicate" => "DuplicateLine",
        "EditorDeleteLine" => "DeleteLine",
        "EditorJoinLines" => "JoinLines",
        "EditorToggleCase" => "ToggleCase",
        "EditorSelectWord" => "ExtendSelection",
        "EditorUnSelectWord" => "ShrinkSelection",
        "SelectNextOccurrence" => "AddCaretNextOccurrence",
        "UnselectPreviousOccurrence" => "UnselectOccurrence",
        "EditorCloneCaretAbove" => "CloneCaretAbove",
        "EditorCloneCaretBelow" => "CloneCaretBelow",
        "EditorToggleColumnMode" => "ToggleColumnSelectionMode",
        "EditorStartNewLine" => "StartNewLine",
        "EditorStartNewLineBefore" => "StartNewLineBefore",
        "EditorEnter" => "EditorNewline",
        "EditorBackSpace" => "EditorBackspace",
        "EditorUnindentSelection" => "EditorUnindent",
        "EditorDeleteToWordStart" => "EditorDeleteWordBack",
        "EditorDeleteToWordEnd" => "EditorDeleteWordForward",
        "EditorLeft" => "MoveLeft",
        "EditorRight" => "MoveRight",
        "EditorUp" => "MoveUp",
        "EditorDown" => "MoveDown",
        "EditorPreviousWord" => "MoveWordLeft",
        "EditorNextWord" => "MoveWordRight",
        "EditorLineStart" => "MoveLineStart",
        "EditorLineEnd" => "MoveLineEnd",
        "EditorTextStart" => "MoveDocStart",
        "EditorTextEnd" => "MoveDocEnd",
        "EditorPageUp" => "PageUp",
        "EditorPageDown" => "PageDown",
        "EditorLeftWithSelection" => "SelectLeft",
        "EditorRightWithSelection" => "SelectRight",
        "EditorUpWithSelection" => "SelectUp",
        "EditorDownWithSelection" => "SelectDown",
        "EditorPreviousWordWithSelection" => "SelectWordLeft",
        "EditorNextWordWithSelection" => "SelectWordRight",
        "EditorLineStartWithSelection" => "SelectLineStart",
        "EditorLineEndWithSelection" => "SelectLineEnd",
        "EditorTextStartWithSelection" => "SelectDocStart",
        "EditorTextEndWithSelection" => "SelectDocEnd",
        "EditorScrollUp" => "ScrollUp",
        "EditorScrollDown" => "ScrollDown",
        "CloseContent" => "CloseTab",
        "CloseAllEditorsButActive" => "CloseOtherTabs",
        "CloseAllEditors" => "CloseAllTabs",
        "PinActiveTabToggle" => "PinTab",
        "MoveEditorToOppositeTabGroup" => "MoveToOppositeGroup",
        "ShowSettings" => "Settings",
        "EditorIncreaseFontSize" => "IncreaseFontSize",
        "EditorDecreaseFontSize" => "DecreaseFontSize",
        "EditorResetFontSize" => "ResetFontSize",
        "QuickChangeScheme" => "QuickSwitchTheme",
        "EditorToggleShowWhitespaces" => "ShowWhitespaces",
        "EditorToggleUseSoftWraps" => "ToggleSoftWrap",
        "ShowProjectStructureSettings" => "ProjectStructure",
        "Synchronize" => "ReloadFromDisk",
        "RunClass" => "RunContext",
        "DebugClass" => "DebugContext",
        "RunCoverage" => "RunWithCoverage",
        "ChooseRunConfiguration" => "SelectRunConfiguration",
        "editRunConfigurations" => "EditRunConfigurations",
        "ToggleLineBreakpoint" => "ToggleBreakpoint",
        "XDebugger.MuteBreakpoints" => "MuteBreakpoints",
        "XDebugger.AttachToProcess" => "AttachToProcess",
        "XDebugger.AddToWatch" => "AddToWatches",
        "Pause" => "PauseProgram",
        "CheckinProject" => "Commit",
        "Vcs.Push" => "Push",
        "Vcs.UpdateProject" => "UpdateProject",
        "Vcs.QuickListPopupAction" => "VcsQuickList",
        "Git.Branches" => "Branches",
        "Diff.ShowDiff" => "ShowDiff",
        "ChangesView.Revert" => "RollbackChanges",
        "Vcs.ShowTabbedFileHistory" => "ShowFileHistory",
        "LocalHistory.ShowHistory" => "ShowLocalHistory",
        "LocalHistory.PutLabel" => "PutLabel",
        "ActivateProjectToolWindow" => "ToggleProjectWindow",
        "ActivateBookmarksToolWindow" => "ToggleBookmarksWindow",
        "ActivateFindToolWindow" => "ToggleFindWindow",
        "ActivateRunToolWindow" => "ToggleRunWindow",
        "ActivateDebugToolWindow" => "ToggleDebugWindow",
        "ActivateProblemsViewToolWindow" => "ToggleProblemsWindow",
        "ActivateStructureToolWindow" => "ToggleStructureWindow",
        "ActivateServicesToolWindow" => "ToggleServicesWindow",
        "ActivateVersionControlToolWindow" => "ToggleGitWindow",
        "ActivateCommitToolWindow" => "ToggleCommitWindow",
        "ActivateTerminalToolWindow" => "ToggleTerminalWindow",
        "ActivateTODOToolWindow" => "ToggleTodoWindow",
        "ActivateDatabaseToolWindow" => "ToggleDatabaseWindow",
        "ActivatePythonConsoleToolWindow" => "TogglePythonConsole",
        "HideAllWindows" => "HideAllToolWindows",
        "HideActiveWindow" => "HideActiveToolWindow",
        _ => return None,
    })
}

fn action_of(pycharm: &str) -> Option<Action> {
    our_id(pycharm).and_then(Action::from_id).or_else(|| Action::from_id(pycharm))
}

/// "ctrl alt L", "shift F6", "ctrl SLASH" as our keystroke.
fn keystroke(s: &str) -> Option<Keystroke> {
    let mut mods = Modifiers::default();
    let mut key = None;
    for t in s.split_whitespace() {
        match t {
            "ctrl" | "control" => mods.ctrl = true,
            "alt" => mods.alt = true,
            "shift" => mods.shift = true,
            "meta" => mods.meta = true,
            k => key = Some(k),
        }
    }
    let k = key?;
    let name = match k {
        "ENTER" => "enter",
        "ESCAPE" => "escape",
        "SPACE" => "space",
        "TAB" => "tab",
        "BACK_SPACE" => "backspace",
        "DELETE" => "delete",
        "INSERT" => "insert",
        "HOME" => "home",
        "END" => "end",
        "PAGE_UP" => "pageup",
        "PAGE_DOWN" => "pagedown",
        "UP" => "up",
        "DOWN" => "down",
        "LEFT" => "left",
        "RIGHT" => "right",
        "SLASH" => "/",
        "BACK_SLASH" => "\\",
        "MINUS" => "-",
        "EQUALS" => "=",
        "COMMA" => ",",
        "PERIOD" => ".",
        "SEMICOLON" => ";",
        "QUOTE" => "'",
        "BACK_QUOTE" => "`",
        "OPEN_BRACKET" => "[",
        "CLOSE_BRACKET" => "]",
        k if k.len() == 1 && k.chars().all(|c| c.is_ascii_alphanumeric()) => return Some(Keystroke { mods, key: k.to_ascii_lowercase() }),
        k if k.starts_with('F') && k[1..].parse::<u8>().is_ok_and(|n| (1..=12).contains(&n)) => return Some(Keystroke { mods, key: k.to_ascii_lowercase() }),
        // Numpad keys and others Brisk does not see as separate keys.
        _ => return None,
    };
    Some(Keystroke { mods, key: name.into() })
}

/// What an import found.
#[derive(Debug, Default, PartialEq)]
pub struct Imported {
    pub keymap: UserKeymap,
    pub name: String,
    /// Actions taken over.
    pub actions: usize,
    /// PyCharm actions Brisk does not have, or shortcuts it cannot use.
    pub skipped: usize,
    /// The keymap is based on something other than PyCharm's default (Eclipse, VS Code...).
    pub foreign_parent: Option<String>,
}

/// One keymap file and its parents among `others` (custom keymaps in the same folder).
pub fn import_xml(text: &str, others: &[(String, String)]) -> Result<Imported, String> {
    let mut out = Imported::default();
    let mut chain = Vec::new();
    let mut cur = text.to_string();
    for _ in 0..8 {
        let doc = roxmltree::Document::parse(&cur).map_err(|e| format!("not a keymap file: {e}"))?;
        let root = doc.root_element();
        if root.tag_name().name() != "keymap" {
            return Err("not a keymap file".into());
        }
        if chain.is_empty() {
            out.name = root.attribute("name").unwrap_or("").to_string();
        }
        let parent = root.attribute("parent").unwrap_or("$default").to_string();
        chain.push(cur.clone());
        match others.iter().find(|(n, _)| *n == parent) {
            Some((_, t)) => cur = t.clone(),
            None => {
                if parent != "$default" {
                    out.foreign_parent = Some(parent);
                }
                break;
            }
        }
    }
    // The base keymap first, so the selected one wins.
    for text in chain.iter().rev() {
        let doc = roxmltree::Document::parse(text).map_err(|e| e.to_string())?;
        for a in doc.root_element().children().filter(|n| n.has_tag_name("action")) {
            let Some(id) = a.attribute("id") else { continue };
            let Some(action) = action_of(id) else {
                out.skipped += 1;
                continue;
            };
            let mut chords = Vec::new();
            for k in a.children().filter(|n| n.has_tag_name("keyboard-shortcut")) {
                let first = k.attribute("first-keystroke").and_then(keystroke);
                let second = k.attribute("second-keystroke").map(keystroke);
                match (first, second) {
                    (Some(f), None) => chords.push(chord_id(&[f])),
                    (Some(f), Some(Some(s))) => chords.push(chord_id(&[f, s])),
                    _ => out.skipped += 1,
                }
            }
            out.keymap.bindings.insert(action.info().id.to_string(), chords);
        }
    }
    out.actions = out.keymap.bindings.len();
    Ok(out)
}

/// PyCharm's settings folders under `%APPDATA%\JetBrains`, newest version first.
pub fn pycharm_config_dirs() -> Vec<PathBuf> {
    let Some(appdata) = brisk_platform::dirs::roaming_app_data() else { return Vec::new() };
    let Ok(rd) = std::fs::read_dir(appdata.join("JetBrains")) else { return Vec::new() };
    let mut v: Vec<PathBuf> = rd.flatten().map(|e| e.path()).filter(|p| p.file_name().is_some_and(|n| n.to_string_lossy().starts_with("PyCharm"))).collect();
    // "PyCharm2025.2" sorts after "PyCharm2024.3"; "PyCharmCE..." lands next to its version.
    v.sort_by_key(|p| std::cmp::Reverse(p.file_name().map(|n| n.to_string_lossy().replace("CE", "")).unwrap_or_default()));
    v
}

/// A keymap file of a PyCharm settings folder: `(name, file text)`.
type KeymapFile = (String, String);

/// The active custom keymap of a PyCharm settings folder: `(name, file text, the folder's
/// other keymaps)`. `None` when PyCharm uses a built-in keymap.
pub fn active_keymap(config: &Path) -> Option<(String, String, Vec<KeymapFile>)> {
    let dir = config.join("keymaps");
    let mut all: Vec<KeymapFile> = Vec::new();
    for e in std::fs::read_dir(&dir).ok()?.flatten() {
        if e.path().extension().is_some_and(|x| x == "xml") {
            if let Ok(text) = std::fs::read_to_string(e.path()) {
                if let Some(name) = roxmltree::Document::parse(&text).ok().and_then(|d| d.root_element().attribute("name").map(str::to_string)) {
                    all.push((name, text));
                }
            }
        }
    }
    let active = std::fs::read_to_string(config.join("options").join("keymap.xml")).ok().and_then(|t| {
        let doc = roxmltree::Document::parse(&t).ok()?;
        doc.descendants().find(|n| n.has_tag_name("active_keymap")).and_then(|n| n.attribute("name")).map(str::to_string)
    });
    let pick = match active {
        Some(name) => all.iter().find(|(n, _)| *n == name)?.clone(),
        // No record of the active one: a single custom keymap is the one in use.
        None if all.len() == 1 => all[0].clone(),
        None => return None,
    };
    Some((pick.0, pick.1, all))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{pycharm_keymap, KeyResolver, Resolution};

    #[test]
    fn imports_a_keymap_with_its_parent() {
        let base = r#"<keymap version="1" name="Team" parent="$default">
  <action id="ReformatCode"><keyboard-shortcut first-keystroke="ctrl alt F" /></action>
  <action id="EditorDuplicate"><keyboard-shortcut first-keystroke="ctrl shift D" /></action>
</keymap>"#;
        let mine = r#"<keymap version="1" name="Mine" parent="Team">
  <action id="CommentByLineComment">
    <keyboard-shortcut first-keystroke="ctrl K" second-keystroke="ctrl C" />
  </action>
  <action id="EditorDuplicate"><keyboard-shortcut first-keystroke="ctrl D" /><keyboard-shortcut first-keystroke="ctrl NUMPAD1" /></action>
  <action id="EditorDeleteLine" />
  <action id="SomeJavaOnlyThing"><keyboard-shortcut first-keystroke="ctrl J" /></action>
</keymap>"#;
        let others = vec![("Team".to_string(), base.to_string()), ("Mine".to_string(), mine.to_string())];
        let got = import_xml(mine, &others).unwrap();
        assert_eq!((got.name.as_str(), got.foreign_parent.clone(), got.skipped), ("Mine", None, 2));
        let b = &got.keymap.bindings;
        assert_eq!(b["ReformatCode"], ["ctrl-alt-f"]);
        assert_eq!(b["DuplicateLine"], ["ctrl-d"]);
        assert_eq!(b["CommentLine"], ["ctrl-k ctrl-c"]);
        assert!(b["DeleteLine"].is_empty());
        let mut r = KeyResolver::new(got.keymap.apply(&pycharm_keymap()));
        let k = |s| Keystroke::parse(s).unwrap();
        assert_eq!(r.key_down(k("ctrl-alt-f"), &["Editor"]), Resolution::Action(Action::ReformatCode));
        assert_eq!(r.key_down(k("ctrl-y"), &["Editor"]), Resolution::Unhandled);

        let eclipse = r#"<keymap version="1" name="Mine" parent="Eclipse copy"></keymap>"#;
        assert_eq!(import_xml(eclipse, &[]).unwrap().foreign_parent.as_deref(), Some("Eclipse copy"));
        assert!(import_xml("<options/>", &[]).is_err());
    }
}
