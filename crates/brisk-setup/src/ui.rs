//! The setup window: a gradient hero on the left with the logo breathing in its glow, the
//! steps on the right. Install: Welcome (one click with sensible defaults) → Customize
//! (folder, shortcuts, PATH, Explorer menu, file types) → Installing → Done. Uninstall:
//! confirm (keep or delete settings) → Removing → Done. Modify: Customize → Done.

use crate::ops;
use brisk_platform::install::{self, Options, FILE_TYPES};
use gpui::prelude::*;
use gpui::{
    div, img, linear_color_stop, linear_gradient, px, rgb, rgba, Animation, AnimationExt, Context, FontWeight, Hsla, Image, ImageFormat, SharedString, Window,
    WindowControlArea,
};
use std::path::PathBuf;
use std::sync::{Arc, Mutex};
use std::time::Duration;

#[derive(Clone, Copy, PartialEq, Eq)]
pub enum Mode {
    Install,
    Uninstall,
    Modify,
}

#[derive(Clone, PartialEq)]
enum Page {
    Welcome,
    Customize,
    Busy,
    Done,
    Failed(String),
}

/// Set by the worker thread: the fraction done, the current step and, at the end, the result.
type Progress = Arc<Mutex<(f32, String, Option<Result<(), String>>)>>;

pub struct Setup {
    mode: Mode,
    page: Page,
    dir: PathBuf,
    options: Options,
    /// Already installed (the version), for "Update" wording and the options it had.
    existing: Option<String>,
    /// Set by the worker thread: fraction and the current step.
    progress: Progress,
    /// The bar as drawn, easing toward the reported fraction.
    shown: f32,
    launch: bool,
    delete_settings: bool,
    /// Brisk windows running from the folder: the step waits for them.
    blocked: usize,
    logo: Arc<Image>,
}

// The palette: Brisk's blue, a violet, and near-black surfaces.
fn blue() -> Hsla {
    rgb(0x3574f0).into()
}
fn violet() -> Hsla {
    rgb(0x8b5cf6).into()
}
fn surface() -> Hsla {
    rgb(0x111217).into()
}
fn card() -> Hsla {
    rgb(0x1a1c23).into()
}
fn card_hover() -> Hsla {
    rgb(0x21242d).into()
}
fn line() -> Hsla {
    rgb(0x2a2d37).into()
}
fn text() -> Hsla {
    rgb(0xeef0f5).into()
}
fn muted() -> Hsla {
    rgb(0x8f94a3).into()
}
fn danger() -> Hsla {
    rgb(0xf0525f).into()
}

/// Segoe MDL2 Assets (Windows 10 and 11) for the few glyphs.
const ICONS: &str = "Segoe MDL2 Assets";
const UI: &str = "Segoe UI";

fn glyph(code: &str, size: f32, color: Hsla) -> gpui::Div {
    div().font_family(ICONS).text_size(px(size)).text_color(color).child(SharedString::from(code.to_string()))
}

impl Setup {
    pub fn new(mode: Mode, cx: &mut Context<Self>) -> Self {
        let existing = install::installed();
        let dir = existing.as_ref().map(|i| i.dir.clone()).or_else(install::default_dir).unwrap_or_else(|| PathBuf::from(r"C:\Brisk"));
        let options = existing.as_ref().map(|i| i.options.clone()).unwrap_or_default();
        let page = match mode {
            Mode::Modify => Page::Customize,
            _ => Page::Welcome,
        };
        let _ = cx;
        Setup {
            mode,
            page,
            dir,
            options,
            existing: existing.map(|i| i.version),
            progress: Arc::new(Mutex::new((0., String::new(), None))),
            shown: 0.,
            launch: true,
            delete_settings: false,
            blocked: 0,
            logo: Arc::new(Image::from_bytes(ImageFormat::Png, include_bytes!("../../../assets/brisk.png").to_vec())),
        }
    }

    fn start(&mut self, cx: &mut Context<Self>) {
        self.blocked = install::running_in(&self.dir).len();
        if self.blocked > 0 {
            cx.notify();
            return;
        }
        self.page = Page::Busy;
        self.shown = 0.;
        *self.progress.lock().unwrap_or_else(|e| e.into_inner()) = (0., "Starting".into(), None);
        let (mode, dir, options, settings, version) =
            (self.mode, self.dir.clone(), self.options.clone(), self.delete_settings, self.existing.clone().unwrap_or_else(|| ops::VERSION.into()));
        let progress = self.progress.clone();
        std::thread::spawn(move || {
            let report = |f: f32, s: &str| {
                let mut p = progress.lock().unwrap_or_else(|e| e.into_inner());
                p.0 = f;
                p.1 = s.to_string();
            };
            let res = match mode {
                Mode::Install => ops::install(&dir, &options, &report),
                Mode::Modify => ops::modify(&dir, &version, &options, &report),
                Mode::Uninstall => ops::uninstall(&dir, settings, &report),
            };
            progress.lock().unwrap_or_else(|e| e.into_inner()).2 = Some(res);
        });
        // Redraw while the work runs; the bar eases toward the reported fraction.
        cx.spawn(async move |this, cx| loop {
            cx.background_executor().timer(Duration::from_millis(16)).await;
            let going = this
                .update(cx, |this, cx| {
                    let (f, _, done) = this.progress.lock().unwrap_or_else(|e| e.into_inner()).clone();
                    this.shown += (f - this.shown) * 0.12;
                    if let Some(res) = done.filter(|_| this.shown > 0.985) {
                        this.page = match res {
                            Ok(()) => Page::Done,
                            Err(e) => Page::Failed(e),
                        };
                        this.shown = 1.;
                    }
                    cx.notify();
                    this.page == Page::Busy
                })
                .unwrap_or(false);
            if !going {
                break;
            }
        })
        .detach();
        cx.notify();
    }

    fn choose_folder(&mut self, cx: &mut Context<Self>) {
        let rx = cx.prompt_for_paths(gpui::PathPromptOptions { files: false, directories: true, multiple: false, prompt: Some("Install here".into()) });
        cx.spawn(async move |this, cx| {
            if let Ok(Ok(Some(paths))) = rx.await {
                if let Some(p) = paths.into_iter().next() {
                    let _ = this.update(cx, |this, cx| {
                        // A parent folder gets its own Brisk folder, as installers do.
                        this.dir = if p.file_name().is_some_and(|n| n.eq_ignore_ascii_case("Brisk")) { p } else { p.join("Brisk") };
                        cx.notify();
                    });
                }
            }
        })
        .detach();
    }

    fn finish(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        if self.mode == Mode::Install && self.launch && self.page == Page::Done {
            ops::launch(&self.dir);
        }
        window.remove_window();
        cx.quit();
    }

    // ---- Pieces ----------------------------------------------------------------------

    fn hero(&self, compact: bool) -> impl IntoElement {
        let orb = |id: &'static str, size: f32, x: f32, y: f32, color: u32, secs: u64| {
            div().absolute().left(px(x)).top(px(y)).size(px(size)).rounded_full().bg(rgba(color)).with_animation(
                id,
                Animation::new(Duration::from_secs(secs)).repeat().with_easing(gpui::pulsating_between(0.35, 1.0)),
                |d, t| d.opacity(t),
            )
        };
        let (title, line) = match self.mode {
            Mode::Install if self.existing.is_some() => ("Update Brisk", "Your settings and projects stay as they are."),
            Mode::Install => ("Brisk", "A fast, light IDE that feels like PyCharm."),
            Mode::Uninstall => ("Brisk", "Sorry to see you go."),
            Mode::Modify => ("Brisk", "Change how Brisk fits into Windows."),
        };
        div()
            .relative()
            .w(px(if compact { 250. } else { 320. }))
            .h_full()
            .flex_none()
            .overflow_hidden()
            .bg(linear_gradient(155., linear_color_stop(rgb(0x2a49c9), 0.), linear_color_stop(rgb(0x5b2bb5), 1.)))
            .child(orb("orb-a", 260., -90., -70., 0x6aa0ff55, 5))
            .child(orb("orb-b", 220., 160., 300., 0xc084fc44, 7))
            .child(orb("orb-c", 120., 210., 40., 0xffffff18, 4))
            // A dark fade at the bottom for the version line.
            .child(div().absolute().left_0().right_0().bottom_0().h(px(160.)).bg(linear_gradient(
                180.,
                linear_color_stop(rgba(0x00000000), 0.),
                linear_color_stop(rgba(0x0000004d), 1.),
            )))
            .child(div().absolute().left_0().right_0().top_0().h(px(40.)).occlude().window_control_area(WindowControlArea::Drag))
            .child(
                div()
                    .absolute()
                    .left(px(if compact { 28. } else { 36. }))
                    .right(px(24.))
                    .top(px(if compact { 70. } else { 120. }))
                    .flex()
                    .flex_col()
                    .gap(px(18.))
                    .child(
                        div()
                            .relative()
                            .size(px(84.))
                            .child(div().absolute().left(px(-14.)).top(px(-14.)).size(px(112.)).rounded_full().bg(rgba(0xffffff26)).with_animation(
                                "glow",
                                Animation::new(Duration::from_millis(2600)).repeat().with_easing(gpui::pulsating_between(0.2, 0.9)),
                                |d, t| d.opacity(t),
                            ))
                            .child(img(self.logo.clone()).size(px(84.))),
                    )
                    .child(div().text_size(px(40.)).font_weight(FontWeight::BOLD).text_color(gpui::white()).child(title))
                    .child(div().text_size(px(14.)).line_height(px(21.)).text_color(rgba(0xffffffcc)).child(line)),
            )
            .child(
                div()
                    .absolute()
                    .left(px(36.))
                    .bottom(px(28.))
                    .flex()
                    .gap(px(8.))
                    .items_center()
                    .child(
                        div().px(px(10.)).py(px(3.)).rounded_full().bg(rgba(0xffffff22)).text_size(px(12.)).text_color(gpui::white()).child(format!(
                            "v{}",
                            if self.mode == Mode::Install { ops::VERSION.to_string() } else { self.existing.clone().unwrap_or_default() }
                        )),
                    )
                    .child(div().text_size(px(12.)).text_color(rgba(0xffffff99)).child("Open source · MIT / Apache-2.0")),
            )
    }

    fn caption(&self) -> impl IntoElement {
        let button = |id: &'static str, area: WindowControlArea, code: &'static str, hover: Hsla| {
            div()
                .id(id)
                .occlude()
                .w(px(46.))
                .h(px(32.))
                .flex()
                .items_center()
                .justify_center()
                .window_control_area(area)
                .hover(move |s| s.bg(hover))
                .child(glyph(code, 10., muted()))
        };
        div()
            .absolute()
            .top_0()
            .left_0()
            .right_0()
            .h(px(32.))
            .flex()
            .child(div().flex_1().h_full().occlude().window_control_area(WindowControlArea::Drag))
            .child(button("min", WindowControlArea::Min, "\u{E921}", card_hover()))
            .child(button("close", WindowControlArea::Close, "\u{E8BB}", rgb(0xc42b1c).into()))
    }

    fn primary(
        &self,
        id: &'static str,
        label: &str,
        danger_tone: bool,
        cx: &mut Context<Self>,
        on: impl Fn(&mut Self, &mut Window, &mut Context<Self>) + 'static,
    ) -> impl IntoElement {
        let (a, b) = if danger_tone { (rgb(0xe5484d), rgb(0xc2255c)) } else { (rgb(0x3574f0), rgb(0x8b5cf6)) };
        div()
            .id(id)
            .h(px(42.))
            .px(px(26.))
            .rounded(px(10.))
            .flex()
            .items_center()
            .justify_center()
            .cursor_pointer()
            .bg(linear_gradient(100., linear_color_stop(a, 0.), linear_color_stop(b, 1.)))
            .shadow_lg()
            .hover(|s| s.opacity(0.92))
            .active(|s| s.opacity(0.8))
            .text_size(px(14.))
            .font_weight(FontWeight::SEMIBOLD)
            .text_color(gpui::white())
            .child(label.to_string())
            .on_click(cx.listener(move |this, _, window, cx| on(this, window, cx)))
    }

    fn secondary(
        &self,
        id: &'static str,
        label: &str,
        cx: &mut Context<Self>,
        on: impl Fn(&mut Self, &mut Window, &mut Context<Self>) + 'static,
    ) -> impl IntoElement {
        div()
            .id(id)
            .h(px(42.))
            .px(px(20.))
            .rounded(px(10.))
            .flex()
            .items_center()
            .cursor_pointer()
            .text_size(px(14.))
            .text_color(muted())
            .hover(|s| s.text_color(text()).bg(card()))
            .child(label.to_string())
            .on_click(cx.listener(move |this, _, window, cx| on(this, window, cx)))
    }

    fn switch(on: bool) -> impl IntoElement {
        div()
            .w(px(38.))
            .h(px(22.))
            .rounded_full()
            .flex_none()
            .p(px(3.))
            .flex()
            .when(on, |d| d.justify_end().bg(linear_gradient(90., linear_color_stop(blue(), 0.), linear_color_stop(violet(), 1.))))
            .when(!on, |d| d.bg(line()))
            .child(div().size(px(16.)).rounded_full().bg(gpui::white()))
    }

    fn toggle_card(
        id: &'static str,
        code: &'static str,
        title: &str,
        note: &str,
        on: bool,
        cx: &mut Context<Self>,
        flip: fn(&mut Options),
    ) -> impl IntoElement {
        div()
            .id(id)
            .min_w_0()
            .flex()
            .items_center()
            .gap(px(14.))
            .px(px(14.))
            .py(px(11.))
            .rounded(px(10.))
            .bg(card())
            .border_1()
            .border_color(if on { rgba(0x3574f066) } else { rgba(0x00000000) })
            .cursor_pointer()
            .hover(|s| s.bg(card_hover()))
            .child(
                div()
                    .size(px(34.))
                    .rounded(px(8.))
                    .flex()
                    .items_center()
                    .justify_center()
                    .flex_none()
                    .bg(if on { rgba(0x3574f02e) } else { rgba(0xffffff0d) })
                    .child(glyph(code, 16., if on { rgb(0x8fb3ff).into() } else { muted() })),
            )
            .child(
                div()
                    .flex_1()
                    .min_w_0()
                    .flex()
                    .flex_col()
                    .gap(px(2.))
                    .child(div().text_size(px(13.)).font_weight(FontWeight::MEDIUM).text_color(text()).child(title.to_string()))
                    .child(div().text_size(px(12.)).text_color(muted()).child(note.to_string())),
            )
            .child(Self::switch(on))
            .on_click(cx.listener(move |this, _, _, cx| {
                flip(&mut this.options);
                cx.notify();
            }))
    }

    fn checkbox(id: &'static str, on: bool, label: &str, cx: &mut Context<Self>, flip: fn(&mut Self)) -> impl IntoElement {
        div()
            .id(id)
            .flex()
            .items_center()
            .gap(px(10.))
            .cursor_pointer()
            .child(
                div()
                    .size(px(18.))
                    .rounded(px(5.))
                    .flex()
                    .items_center()
                    .justify_center()
                    .when(on, |d| d.bg(blue()))
                    .when(!on, |d| d.border_1().border_color(line()))
                    .when(on, |d| d.child(glyph("\u{E73E}", 11., gpui::white()))),
            )
            .child(div().text_size(px(13.)).text_color(text()).child(label.to_string()))
            .on_click(cx.listener(move |this, _, _, cx| {
                flip(this);
                cx.notify();
            }))
    }

    fn heading(title: &str, sub: &str) -> impl IntoElement {
        div()
            .flex()
            .flex_col()
            .gap(px(6.))
            .child(div().text_size(px(24.)).font_weight(FontWeight::SEMIBOLD).text_color(text()).child(title.to_string()))
            .child(div().text_size(px(13.)).line_height(px(19.)).text_color(muted()).child(sub.to_string()))
    }

    fn blocked_note(&self) -> Option<impl IntoElement> {
        (self.blocked > 0).then(|| {
            div()
                .flex()
                .items_center()
                .gap(px(10.))
                .px(px(14.))
                .py(px(10.))
                .rounded(px(10.))
                .bg(rgba(0xf0525f1f))
                .child(glyph("\u{E7BA}", 14., danger()))
                .child(div().text_size(px(13.)).text_color(text()).child("Brisk is running. Close its windows, then try again."))
        })
    }

    // ---- Pages -----------------------------------------------------------------------

    fn welcome(&self, cx: &mut Context<Self>) -> gpui::AnyElement {
        let update = self.existing.is_some();
        let (title, sub, go) = match self.mode {
            Mode::Uninstall => (
                "Uninstall Brisk".to_string(),
                format!("Removes Brisk from {} and from the Start menu, the Explorer menu and PATH. Your projects are not touched.", self.dir.display()),
                "Uninstall",
            ),
            _ if update => (
                format!("Update to {}", ops::VERSION),
                format!(
                    "Brisk {} is installed in {}. The update replaces its files and keeps your settings.",
                    self.existing.clone().unwrap_or_default(),
                    self.dir.display()
                ),
                "Update",
            ),
            _ => (
                "Ready when you are".to_string(),
                format!("Brisk goes to {} for your account only: no administrator rights, no restart.", self.dir.display()),
                "Install",
            ),
        };
        let uninstall = self.mode == Mode::Uninstall;
        let summary: Vec<(&str, &str)> = if uninstall {
            vec![]
        } else {
            let o = &self.options;
            [
                (o.start_menu, "\u{E71D}", "Start menu shortcut"),
                (o.desktop, "\u{E7F4}", "Desktop shortcut"),
                (o.path, "\u{E756}", "brisk command in terminals"),
                (o.context_menu, "\u{E8A5}", "\"Edit with Brisk\" in Explorer"),
                (!o.file_types.is_empty(), "\u{E8B7}", "Listed in \"Open with\" for code files"),
            ]
            .into_iter()
            .filter(|(on, _, _)| *on)
            .map(|(_, c, t)| (c, t))
            .collect()
        };
        div()
            .flex()
            .flex_col()
            .size_full()
            .gap(px(22.))
            .child(Self::heading(&title, &sub))
            .when(!summary.is_empty(), |d| {
                d.child(div().flex().flex_col().gap(px(10.)).children(summary.into_iter().map(|(c, t)| {
                    div()
                        .flex()
                        .items_center()
                        .gap(px(12.))
                        .child(div().size(px(28.)).rounded(px(7.)).flex().items_center().justify_center().bg(rgba(0x3574f022)).child(glyph(
                            c,
                            13.,
                            rgb(0x8fb3ff).into(),
                        )))
                        .child(div().text_size(px(13.)).text_color(text()).child(t))
                })))
            })
            .when(uninstall, |d| {
                d.child(Self::checkbox("del-settings", self.delete_settings, "Also delete my settings, database connections and saved passwords", cx, |s| {
                    s.delete_settings = !s.delete_settings
                }))
            })
            .children(self.blocked_note())
            .child(div().flex_1())
            .child(
                div()
                    .flex()
                    .items_center()
                    .gap(px(8.))
                    .when(!uninstall, |d| {
                        d.child(self.secondary("customize", "Customize", cx, |this, _, cx| {
                            this.page = Page::Customize;
                            cx.notify();
                        }))
                    })
                    .child(div().flex_1())
                    .child(self.secondary("cancel", "Cancel", cx, |_, window, cx| {
                        window.remove_window();
                        cx.quit();
                    }))
                    .child(self.primary("go", go, uninstall, cx, |this, _, cx| this.start(cx))),
            )
            .into_any_element()
    }

    fn customize(&self, cx: &mut Context<Self>) -> gpui::AnyElement {
        let o = &self.options;
        let modify = self.mode == Mode::Modify;
        let chips: Vec<_> = FILE_TYPES
            .iter()
            .map(|(ext, _)| {
                let on = o.file_types.iter().any(|t| t == ext);
                let ext_s = ext.to_string();
                div()
                    .id(SharedString::from(format!("ft-{ext}")))
                    .px(px(10.))
                    .py(px(4.))
                    .rounded_full()
                    .cursor_pointer()
                    .text_size(px(12.))
                    .when(on, |d| d.bg(rgba(0x3574f033)).text_color(rgb(0xbcd0ff)).border_1().border_color(rgba(0x3574f080)))
                    .when(!on, |d| d.bg(card()).text_color(muted()).border_1().border_color(line()))
                    .hover(|s| s.border_color(rgba(0x8b5cf6aa)))
                    .child(format!(".{ext}"))
                    .on_click(cx.listener(move |this, _, _, cx| {
                        let t = &mut this.options.file_types;
                        match t.iter().position(|x| *x == ext_s) {
                            Some(i) => {
                                t.remove(i);
                            }
                            None => t.push(ext_s.clone()),
                        }
                        cx.notify();
                    }))
            })
            .collect();
        // The options scroll on a short screen; the buttons stay in view below them.
        let body = div()
            .id("customize-page")
            .flex_1()
            .min_h_0()
            .overflow_y_scroll()
            .flex()
            .flex_col()
            .gap(px(16.))
            .pb(px(12.))
            .child(Self::heading(
                if modify { "Change Brisk's setup" } else { "Make it yours" },
                "Everything here can be changed later from Apps & features › Brisk › Modify.",
            ))
            .when(!modify, |d| {
                d.child(
                    div()
                        .flex()
                        .items_center()
                        .gap(px(10.))
                        .px(px(14.))
                        .py(px(10.))
                        .rounded(px(10.))
                        .bg(card())
                        .child(glyph("\u{E8B7}", 15., muted()))
                        .child(div().flex_1().overflow_hidden().text_size(px(13.)).text_color(text()).child(self.dir.display().to_string()))
                        .child(
                            div()
                                .id("change-dir")
                                .px(px(12.))
                                .py(px(5.))
                                .rounded(px(7.))
                                .cursor_pointer()
                                .text_size(px(12.))
                                .text_color(rgb(0xbcd0ff))
                                .bg(rgba(0x3574f026))
                                .hover(|s| s.bg(rgba(0x3574f044)))
                                .child("Change")
                                .on_click(cx.listener(|this, _, _, cx| this.choose_folder(cx))),
                        ),
                )
            })
            .child(
                div()
                    .grid()
                    .grid_cols(2)
                    .gap(px(10.))
                    .child(Self::toggle_card("opt-start", "\u{E71D}", "Start menu", "A shortcut in Start", o.start_menu, cx, |o| o.start_menu = !o.start_menu))
                    .child(Self::toggle_card("opt-desktop", "\u{E7F4}", "Desktop", "A shortcut there", o.desktop, cx, |o| o.desktop = !o.desktop))
                    .child(Self::toggle_card("opt-path", "\u{E756}", "Add to PATH", "brisk . in any terminal", o.path, cx, |o| o.path = !o.path))
                    .child(Self::toggle_card("opt-menu", "\u{E8A5}", "Explorer menu", "Right-click a file or folder", o.context_menu, cx, |o| {
                        o.context_menu = !o.context_menu
                    })),
            )
            .child(
                div()
                    .flex()
                    .items_center()
                    .child(div().flex_1().text_size(px(13.)).font_weight(FontWeight::MEDIUM).text_color(text()).child("Open with Brisk"))
                    .child(div().text_size(px(12.)).text_color(muted()).child(format!("{} of {} types", o.file_types.len(), FILE_TYPES.len()))),
            )
            .child(div().flex().flex_wrap().gap(px(6.)).children(chips))
            .children(self.blocked_note());
        div()
            .flex()
            .flex_col()
            .size_full()
            .gap(px(12.))
            .child(body)
            .child(
                div()
                    .flex()
                    .items_center()
                    .gap(px(8.))
                    .when(!modify, |d| {
                        d.child(self.secondary("back", "Back", cx, |this, _, cx| {
                            this.page = Page::Welcome;
                            cx.notify();
                        }))
                    })
                    .child(div().flex_1())
                    .child(self.primary(
                        "go2",
                        if modify {
                            "Apply"
                        } else if self.existing.is_some() {
                            "Update"
                        } else {
                            "Install"
                        },
                        false,
                        cx,
                        |this, _, cx| this.start(cx),
                    )),
            )
            .into_any_element()
    }

    fn busy(&self) -> gpui::AnyElement {
        let (_, step, _) = self.progress.lock().unwrap_or_else(|e| e.into_inner()).clone();
        let title = match self.mode {
            Mode::Install if self.existing.is_some() => "Updating Brisk",
            Mode::Install => "Installing Brisk",
            Mode::Uninstall => "Removing Brisk",
            Mode::Modify => "Applying",
        };
        let pct = (self.shown * 100.).round().clamp(0., 100.);
        div()
            .flex()
            .flex_col()
            .size_full()
            .justify_center()
            .gap(px(18.))
            .child(
                div()
                    .flex()
                    .items_end()
                    .child(div().flex_1().text_size(px(24.)).font_weight(FontWeight::SEMIBOLD).text_color(text()).child(title))
                    .child(div().text_size(px(34.)).font_weight(FontWeight::BOLD).text_color(text()).child(format!("{pct}%"))),
            )
            .child(
                div()
                    .relative()
                    .h(px(8.))
                    .w_full()
                    .rounded_full()
                    .bg(card())
                    .overflow_hidden()
                    .child(div().absolute().left_0().top_0().bottom_0().w(gpui::relative(self.shown.max(0.02))).rounded_full().bg(linear_gradient(
                        90.,
                        linear_color_stop(blue(), 0.),
                        linear_color_stop(violet(), 1.),
                    )))
                    // A light sweep over the bar.
                    .child(
                        div()
                            .absolute()
                            .top_0()
                            .bottom_0()
                            .w(px(90.))
                            .bg(linear_gradient(90., linear_color_stop(rgba(0xffffff00), 0.), linear_color_stop(rgba(0xffffff40), 1.)))
                            .with_animation("sweep", Animation::new(Duration::from_millis(1400)).repeat(), |d, t| d.left(px(-90. + t * 560.))),
                    ),
            )
            .child(div().text_size(px(13.)).text_color(muted()).child(step))
            .into_any_element()
    }

    fn done(&self, cx: &mut Context<Self>) -> gpui::AnyElement {
        let (title, sub) = match self.mode {
            Mode::Install if self.existing.is_some() => ("Updated", format!("Brisk {} is ready.", ops::VERSION)),
            Mode::Install => {
                ("You're all set", "Find Brisk in Start, or type brisk in a terminal. Right-click a file or folder for Edit with Brisk.".to_string())
            }
            Mode::Uninstall => (
                "Brisk is removed",
                if self.delete_settings { "Its settings are gone too.".into() } else { "Your settings stay in %APPDATA%\\Brisk for a later install.".into() },
            ),
            Mode::Modify => ("Saved", "Brisk's Windows integration is updated.".into()),
        };
        div()
            .flex()
            .flex_col()
            .size_full()
            .gap(px(20.))
            .child(div().flex_1())
            .child(
                div()
                    .size(px(64.))
                    .rounded_full()
                    .flex()
                    .items_center()
                    .justify_center()
                    .bg(linear_gradient(135., linear_color_stop(blue(), 0.), linear_color_stop(violet(), 1.)))
                    .child(glyph("\u{E73E}", 26., gpui::white()))
                    .with_animation("pop", Animation::new(Duration::from_millis(520)).with_easing(gpui::ease_out_quint()), |d, t| {
                        d.opacity(t).mt(px(18. * (1. - t)))
                    }),
            )
            .child(Self::heading(title, &sub))
            .when(self.mode == Mode::Install, |d| d.child(Self::checkbox("launch", self.launch, "Open Brisk now", cx, |s| s.launch = !s.launch)))
            .child(div().flex_1())
            .child(div().flex().child(div().flex_1()).child(self.primary("finish", "Finish", false, cx, |this, window, cx| this.finish(window, cx))))
            .into_any_element()
    }

    fn failed(&self, err: &str, cx: &mut Context<Self>) -> gpui::AnyElement {
        div()
            .flex()
            .flex_col()
            .size_full()
            .gap(px(16.))
            .child(div().flex_1())
            .child(div().size(px(56.)).rounded_full().flex().items_center().justify_center().bg(rgba(0xf0525f26)).child(glyph("\u{E783}", 24., danger())))
            .child(Self::heading("Something went wrong", err))
            .child(div().flex_1())
            .child(
                div()
                    .flex()
                    .gap(px(8.))
                    .child(div().flex_1())
                    .child(self.secondary("close-fail", "Close", cx, |_, window, cx| {
                        window.remove_window();
                        cx.quit();
                    }))
                    .child(self.primary("retry", "Try again", false, cx, |this, _, cx| this.start(cx))),
            )
            .into_any_element()
    }
}

impl Render for Setup {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        // A small screen (a 13" laptop at 150%, or a window the display shrank): a narrower
        // hero and tighter margins, so the steps keep their room.
        let view = window.viewport_size();
        let compact = view.width < px(820.) || view.height < px(540.);
        let page = match self.page.clone() {
            Page::Welcome => self.welcome(cx),
            Page::Customize => self.customize(cx),
            Page::Busy => self.busy(),
            Page::Done => self.done(cx),
            Page::Failed(e) => self.failed(&e, cx),
        };
        div().size_full().flex().bg(surface()).font_family(UI).text_color(text()).child(self.hero(compact)).child(
            div().relative().flex_1().min_w_0().h_full().child(self.caption()).child(
                div()
                    .size_full()
                    .pt(px(if compact { 44. } else { 56. }))
                    .pb(px(if compact { 20. } else { 30. }))
                    .px(px(if compact { 26. } else { 40. }))
                    .child(page),
            ),
        )
    }
}
