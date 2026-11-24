//! BRISK_SMOKE_KEYMAP=1 with a scratch APPDATA (it writes keymap.json there, and a fake
//! PyCharm settings folder to import from). Filters Keymap by shortcut, gives Ctrl+D to
//! Delete Line through the "press the keys" box (keys go through `on_key_down`), checks the
//! conflict, the saved file and the editor; imports a PyCharm keymap; resets all.

use super::{Target, Workspace};
use brisk_keymap::{Action, Keystroke};
use gpui::{Context, Window};

impl Workspace {
    pub(super) fn keymap_smoke(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let file = self.root.join("lines.py");
        let _ = std::fs::write(&file, "one\ntwo\n");
        self.open_file(&file, window, cx);
        let text = |this: &Workspace, cx: &gpui::App| this.active_editor().map(|e| e.read(cx).buffer.text()).unwrap_or_default();

        let found: Vec<String> = self.keymap_items("ctrl+d").iter().map(|i| format!("{} [{}]", i.label, i.hint)).collect();
        eprintln!("smoke-keymap: filter ctrl+d -> {found:?}");
        eprintln!("smoke-keymap: Delete Line menu -> {:?}", self.keymap_action_items(Action::DeleteLine).iter().map(|i| i.label.clone()).collect::<Vec<_>>());

        self.keymap_pick(Target::KeymapAdd(Action::DeleteLine), window, cx);
        self.smoke_press("ctrl-d", window, cx);
        if std::env::var("BRISK_SMOKE_KEYMAP").is_ok_and(|v| v == "hold") {
            return;
        }
        let shown = self.key_capture.as_ref().map(|c| (c.strokes.iter().map(Keystroke::to_string).collect::<Vec<_>>(), self.resolver.keymap().conflicts(c.action, &c.strokes)));
        eprintln!("smoke-keymap: capture -> {shown:?}");
        self.smoke_press("enter", window, cx);
        eprintln!("smoke-keymap: status {:?}, capture closed {}", self.status, self.key_capture.is_none());
        eprintln!("smoke-keymap: keymap.json {:?}", std::fs::read_to_string(crate::keymap_store::path()).unwrap_or_default());
        self.focus_editor(window, cx);
        self.smoke_press("ctrl-d", window, cx);
        eprintln!("smoke-keymap: ctrl+d in the editor -> {:?}", text(self, cx));

        // A PyCharm settings folder with a custom keymap.
        let jb = std::path::PathBuf::from(std::env::var_os("APPDATA").unwrap_or_default()).join("JetBrains").join("PyCharm2025.2");
        let _ = std::fs::create_dir_all(jb.join("keymaps"));
        let _ = std::fs::create_dir_all(jb.join("options"));
        let _ = std::fs::write(
            jb.join("keymaps/Mine.xml"),
            r#"<keymap version="1" name="Mine" parent="$default"><action id="ReformatCode"><keyboard-shortcut first-keystroke="ctrl alt F" /></action><action id="AnalyzeStacktrace"><keyboard-shortcut first-keystroke="ctrl alt shift S" /></action></keymap>"#,
        );
        let _ = std::fs::write(jb.join("options/keymap.xml"), r#"<application><component name="KeymapManager"><active_keymap name="Mine" /></component></application>"#);
        self.keymap_pick(Target::KeymapImport, window, cx);
        let k = Keystroke::parse("ctrl-alt-f").unwrap();
        let by = self.resolver.keymap().bindings.iter().filter(|b| matches!(&b.trigger, brisk_keymap::Trigger::Keys(ks) if ks == &vec![k.clone()])).map(|b| b.action).collect::<Vec<_>>();
        eprintln!("smoke-keymap: import -> {:?}; ctrl+alt+f -> {by:?}; delete line still {:?}", self.status, self.resolver.keymap().shortcut_label(Action::DeleteLine));

        self.keymap_pick(Target::KeymapResetAll, window, cx);
        let _ = std::fs::write(&file, "one\ntwo\n");
        self.focus_editor(window, cx);
        self.smoke_press("ctrl-d", window, cx);
        eprintln!("smoke-keymap: after Reset All -> file exists {}, ctrl+d {:?}", crate::keymap_store::path().exists(), self.resolver.keymap().shortcut_label(Action::DuplicateLine));
        cx.quit();
    }
}
