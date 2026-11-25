//! BRISK_SMOKE_STRIPE=1 on an empty folder (writes `stripe.py`; needs ruff): the error stripe's
//! marks (an error, a warning, unused code, TODO comments, search hits), the tooltip over a
//! mark, a click on a mark going to its line, and that idle frames do not rebuild the marks.
//! `=hold` stays with the tooltip of the error mark shown.

use crate::workspace::Workspace;
use gpui::{Context, Window};
use std::time::Duration;

fn sample() -> String {
    let mut s = String::from("import os\nimport sys  # TODO: drop sys\n\n\n");
    for i in 0..120 {
        s.push_str(&format!("def step_{i}(value):\n    total = value * {i}\n    return total\n\n\n"));
    }
    s.push_str("def broken():\n    return undefined_name + 1  # FIXME: undefined\n\n\n");
    s.push_str("print(f\"no placeholders\")\n");
    s
}

impl Workspace {
    pub(in crate::workspace) fn stripe_smoke(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let file = self.root.join("stripe.py");
        let text = sample();
        let _ = std::fs::write(&file, &text);
        self.open_file(&file, window, cx);
        let hold = std::env::var("BRISK_SMOKE_STRIPE").as_deref() == Ok("hold");
        cx.spawn_in(window, async move |this, cx| {
            let bg = cx.background_executor().clone();
            for _ in 0..200 {
                bg.timer(Duration::from_millis(50)).await;
                let ready = this.update(cx, |t, cx| t.active_editor().is_some_and(|e| e.read(cx).diag_version == e.read(cx).buffer.version())).unwrap_or(true);
                if ready {
                    break;
                }
            }
            bg.timer(Duration::from_millis(300)).await;
            let Ok(Some(ed)) = this.update(cx, |t, _| t.active_editor().cloned()) else { return };
            let marks = ed.update(cx, |e, _| e.stripe_for_smoke()).unwrap_or_default();
            let lines = |k: &str| marks.iter().filter(|(m, _)| m == k).map(|(_, l)| *l).collect::<Vec<_>>();
            eprintln!("smoke-stripe: marks error {:?}, warning {:?}, todo {:?}", lines("Error"), lines("Warning"), lines("Todo"));
            let _ = cx.update(|window, cx| ed.update(cx, |e, cx| e.find_for_smoke("total", window, cx)));
            bg.timer(Duration::from_millis(300)).await;
            let marks = ed.update(cx, |e, _| e.stripe_for_smoke()).unwrap_or_default();
            let search = marks.iter().filter(|(m, _)| m == "Search").count();
            eprintln!(
                "smoke-stripe: after find 'total' -> {search} search marks (lines with hits), errors still {}",
                marks.iter().filter(|(m, _)| m == "Error").count()
            );
            let _ = cx.update(|window, cx| ed.update(cx, |e, cx| e.close_find(window, cx)));
            bg.timer(Duration::from_millis(200)).await;
            // Idle: the caret blinks, frames are drawn, the marks stay as built.
            let before = ed.read_with(cx, |e, _| e.stripe_builds_for_smoke()).unwrap_or(0);
            bg.timer(Duration::from_millis(1500)).await;
            let after = ed.read_with(cx, |e, _| e.stripe_builds_for_smoke()).unwrap_or(0);
            eprintln!("smoke-stripe: idle 1.5 s -> rebuilds {}", after - before);
            let Some(p) = ed.read_with(cx, |e, _| e.stripe_point_for_smoke("Error")).ok().flatten() else {
                eprintln!("smoke-stripe: no error mark");
                let _ = this.update(cx, |_, cx| cx.quit());
                return;
            };
            let tips = ed.update(cx, |e, cx| e.stripe_hover_for_smoke(p, cx)).unwrap_or_default();
            eprintln!("smoke-stripe: hover the error mark -> {tips:?}");
            if hold {
                eprintln!("smoke-stripe: holding");
                return;
            }
            let todo = ed.read_with(cx, |e, _| e.stripe_point_for_smoke("Todo")).ok().flatten();
            if let Some(t) = todo {
                let tips = ed.update(cx, |e, cx| e.stripe_hover_for_smoke(t, cx)).unwrap_or_default();
                eprintln!("smoke-stripe: hover the first TODO mark -> {tips:?}");
            }
            let clicked = ed.update(cx, |e, cx| e.stripe_click_for_smoke(p, cx)).unwrap_or(false);
            bg.timer(Duration::from_millis(300)).await;
            let (caret, top) = ed.read_with(cx, |e, _| (e.caret_line_col(), e.scroll_state().1)).unwrap_or_default();
            eprintln!(
                "smoke-stripe: click the error mark -> {clicked}, caret {:?}, view top row {:.0}",
                (caret.0 + 1, caret.1 + 1),
                top / crate::settings::line_height()
            );
            let typed = ed.read_with(cx, |e, _| e.stripe_builds_for_smoke()).unwrap_or(0);
            eprintln!("smoke-stripe: rebuilds so far {typed}");
            eprintln!("smoke-stripe: done");
            let _ = this.update(cx, |_, cx| cx.quit());
        })
        .detach();
    }
}
