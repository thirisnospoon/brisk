//! BRISK_SMOKE_WHITESPACE=1 on an empty folder (writes `ws.py` and `tabs.mk`): View › Active
//! Editor › Show Whitespaces through the action (as Find Action runs it), the per-line cache,
//! that it applies to this editor only, and the Settings default. `=hold` stays with them shown.

use crate::workspace::Workspace;
use brisk_keymap::Action;
use gpui::{Context, Window};
use std::time::Duration;

impl Workspace {
    pub(in crate::workspace) fn whitespace_smoke(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let mk = self.root.join("tabs.mk");
        let _ = std::fs::write(&mk, "all:\n\techo one\t# tab inside\n");
        let file = self.root.join("ws.py");
        let _ = std::fs::write(&file, "def f(a,  b):\n    x = a + b   \n    return x\n");
        self.open_file(&mk, window, cx);
        self.open_file(&file, window, cx);
        let hold = std::env::var("BRISK_SMOKE_WHITESPACE").as_deref() == Ok("hold");
        cx.spawn_in(window, async move |this, cx| {
            let bg = cx.background_executor().clone();
            bg.timer(Duration::from_millis(600)).await;
            let Ok(Some(ed)) = this.update(cx, |t, _| t.active_editor().cloned()) else { return };
            let st = |cx: &mut gpui::AsyncWindowContext, line| ed.read_with(cx, |e, _| e.whitespace_for_smoke(line)).unwrap_or_default();
            eprintln!("smoke-whitespace: default -> {:?} (shown, cached lines, spaces, tabs on line 2)", st(cx, 1));
            let _ = this.update_in(cx, |t, window, cx| t.dispatch(Action::ShowWhitespaces, window, cx));
            bg.timer(Duration::from_millis(200)).await;
            eprintln!("smoke-whitespace: Show Whitespaces -> line 2 {:?}, line 1 {:?}", st(cx, 1), st(cx, 0));
            let _ = ed.update(cx, |e, cx| e.type_for_smoke("y", cx));
            bg.timer(Duration::from_millis(200)).await;
            eprintln!("smoke-whitespace: after typing (cache rebuilt for the new text) -> {:?}", st(cx, 0));
            let mk_ed = this.update_in(cx, |t, window, cx| {
                t.open_file(&t.root.join("tabs.mk"), window, cx);
                t.active_editor().cloned()
            });
            bg.timer(Duration::from_millis(300)).await;
            if let Ok(Some(m)) = mk_ed {
                let s = m.read_with(cx, |e, _| e.whitespace_for_smoke(1)).unwrap_or_default();
                eprintln!("smoke-whitespace: other editor (tabs.mk) -> {s:?} (not shown: per editor)");
                let _ = this.update_in(cx, |t, window, cx| t.dispatch(Action::ShowWhitespaces, window, cx));
                bg.timer(Duration::from_millis(200)).await;
                let s = m.read_with(cx, |e, _| e.whitespace_for_smoke(1)).unwrap_or_default();
                eprintln!("smoke-whitespace: tabs.mk after the action -> {s:?} (tabs)");
            }
            if hold {
                eprintln!("smoke-whitespace: holding");
                return;
            }
            let _ = this.update_in(cx, |t, window, cx| t.open_file(&t.root.join("ws.py"), window, cx));
            let _ = this.update_in(cx, |t, window, cx| t.dispatch(Action::ShowWhitespaces, window, cx));
            bg.timer(Duration::from_millis(200)).await;
            eprintln!("smoke-whitespace: toggled off -> {:?}", st(cx, 1));
            let mut s = brisk_settings::current();
            s.show_whitespaces = true;
            brisk_settings::apply(&s);
            let fresh = this.update_in(cx, |t, window, cx| {
                let p = t.root.join("fresh.py");
                let _ = std::fs::write(&p, "a = 1\n");
                t.open_file(&p, window, cx);
                t.active_editor().cloned()
            });
            bg.timer(Duration::from_millis(300)).await;
            if let Ok(Some(f)) = fresh {
                eprintln!("smoke-whitespace: Settings default on, a new editor -> {:?}", f.read_with(cx, |e, _| e.whitespace_for_smoke(0)).unwrap_or_default());
            }
            brisk_settings::apply(&brisk_settings::Settings::default());
            eprintln!("smoke-whitespace: done");
            let _ = this.update(cx, |_, cx| cx.quit());
        })
        .detach();
    }
}
