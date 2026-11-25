//! `BRISK_SMOKE_BARS=1` on a folder with a couple hundred files: the scrollbars of the Project
//! tree and the Run console with real Windows mouse messages. For each: hover over the thumb,
//! a click on the track (a page), a thumb drag, the release. The tree's selection must not
//! change; in Run, paging up stops following the output and dragging to the bottom follows
//! it again. `BRISK_SMOKE_BARS_HOLD=tree|run` stops with the thumb dragged, for a screenshot.

use crate::scrollbar::{Metrics, Source};
use crate::workspace::smoke::win_input::{post, Mouse};
use crate::workspace::Workspace;
use gpui::{AsyncWindowContext, Bounds, Context, Pixels, WeakEntity, Window};
use std::time::Duration;

type Probe = (Option<Metrics>, bool, bool, Option<Bounds<Pixels>>, Option<(f32, f32)>);

fn probe(this: &WeakEntity<Workspace>, run: bool, cx: &mut AsyncWindowContext) -> Option<Probe> {
    this.update(cx, |t, cx| {
        if run {
            return t.runs.panel.as_ref().map(|p| p.read(cx).bar_state_for_smoke());
        }
        let b = t.tree_bar.borrow();
        Some((Source::list(&t.tree_scroll).metrics(cx), b.hover, b.drag.is_some(), b.bounds, b.thumb))
    })
    .ok()
    .flatten()
}

impl Workspace {
    pub(in crate::workspace) fn bars_smoke(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let _ = std::fs::write(self.root.join("out.py"), "for i in range(500):\n    print(f'line {i}')\n");
        let scale = window.scale_factor();
        let hold = std::env::var("BRISK_SMOKE_BARS_HOLD").unwrap_or_default();
        cx.spawn_in(window, async move |this, cx| {
            let bg = cx.background_executor().clone();
            let pause = |ms| bg.timer(Duration::from_millis(ms));
            pause(800).await;
            let s = |v: f32| v * scale;
            let follow = |this: &WeakEntity<Workspace>, cx: &mut AsyncWindowContext| {
                this.update(cx, |t, cx| t.runs.panel.as_ref().map(|p| p.read(cx).scroll_to_end)).ok().flatten()
            };
            for run in [false, true] {
                let name = if run { "run" } else { "tree" };
                if run {
                    let _ = this.update_in(cx, |this, window, cx| {
                        this.run_config(brisk_run::RunConfig { name: "out".into(), target: "out.py".into(), ..Default::default() }, window, cx);
                    });
                    for _ in 0..100 {
                        pause(100).await;
                        if this.update(cx, |t, cx| t.runs.panel.as_ref().is_some_and(|p| p.read(cx).run_tabs().iter().all(|(_, r)| !r))).unwrap_or(false) {
                            break;
                        }
                    }
                    pause(500).await;
                }
                let show = |label: &str, this: &WeakEntity<Workspace>, cx: &mut AsyncWindowContext| {
                    if let Some((m, hover, drag, _, thumb)) = probe(this, run, cx) {
                        let m = m.map(|m| format!("{:.0}/{:.0} view {:.0}", m.top, m.max, m.view));
                        let extra = if run {
                            format!(", follow {:?}", follow(this, cx))
                        } else {
                            format!(", selected {:?}", this.update(cx, |t, _| t.tree_selected).ok().flatten())
                        };
                        eprintln!("smoke-bars: {name} {label}: top {m:?}, hover {hover}, drag {drag}, thumb {thumb:?}{extra}");
                    }
                };
                let Some((Some(_), _, _, Some(b), Some((top, th)))) = probe(&this, run, cx) else {
                    eprintln!("smoke-bars: {name}: no scrollbar shown");
                    continue;
                };
                let (x, btop, h) = (s(f32::from(b.left()) + 6.), f32::from(b.top()), f32::from(b.size.height));
                show("start", &this, cx);
                post(Mouse::Move, x - s(80.), s(btop + 40.), false);
                pause(80).await;
                post(Mouse::Move, x, s(btop + top + th / 2.), false);
                pause(150).await;
                show("hover thumb", &this, cx);
                post(Mouse::Move, x - s(80.), s(btop + 40.), false);
                pause(150).await;
                show("hover off", &this, cx);
                // Track click: below the thumb in the tree (it starts at the top), above in Run.
                let ty = if run { s(btop + top / 2.) } else { s(btop + (top + th + h) / 2.) };
                post(Mouse::Move, x, ty, false);
                post(Mouse::Down, x, ty, true);
                post(Mouse::Up, x, ty, false);
                pause(150).await;
                show(if run { "click track above thumb" } else { "click track below thumb" }, &this, cx);
                // Drag the thumb: down 60 px in the tree, to the very bottom in Run.
                let Some((_, _, _, _, Some((top, th)))) = probe(&this, run, cx) else { continue };
                let gy = btop + top + th / 2.;
                // Run: to the track bottom (posted moves past the window edge carry no capture).
                let dy = if run { btop + h - 1. - gy } else { 60. };
                post(Mouse::Move, x, s(gy), false);
                post(Mouse::Down, x, s(gy), true);
                for i in 1..=10 {
                    post(Mouse::Move, x, s(gy + dy * i as f32 / 10.), true);
                    pause(20).await;
                }
                pause(100).await;
                show(&format!("dragging thumb +{dy:.0} px"), &this, cx);
                if hold == name {
                    eprintln!("smoke-bars: holding");
                    return;
                }
                post(Mouse::Up, x, s(gy + dy), false);
                pause(150).await;
                show("released", &this, cx);
            }
            eprintln!("smoke-bars: done");
            let _ = this.update(cx, |_, cx| cx.quit());
        })
        .detach();
    }
}
