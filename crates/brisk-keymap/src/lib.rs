//! Actions and PyCharm-compatible key bindings.
//!
//! The UI layer converts platform key events into [`Keystroke`]s and feeds them to
//! [`KeyResolver`], which handles multi-stroke chords and the PyCharm "double Shift" gesture.

mod action;
mod pycharm;
mod pycharm_import;
mod user;

pub use action::{Action, ActionInfo, ALL_ACTIONS};
pub use pycharm::pycharm_keymap;
pub use pycharm_import::{active_keymap, import_xml, pycharm_config_dirs, Imported};
pub use user::{chord_id, parse_chord, UserKeymap};

use std::fmt;
use std::time::{Duration, Instant};

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Hash)]
pub struct Modifiers {
    pub ctrl: bool,
    pub alt: bool,
    pub shift: bool,
    pub meta: bool,
}

/// One key press. `key` is lowercase: "a", "f12", "enter", "tab", "left", "space", "/", ...
#[derive(Clone, Debug, PartialEq, Eq, Hash)]
pub struct Keystroke {
    pub mods: Modifiers,
    pub key: String,
}

impl Keystroke {
    /// Parse "ctrl-shift-a", "alt-f12", "shift-f6", "ctrl-/".
    pub fn parse(s: &str) -> Option<Self> {
        let s = s.trim().to_ascii_lowercase();
        if s.is_empty() {
            return None;
        }
        let mut mods = Modifiers::default();
        let mut rest = s.as_str();
        loop {
            let (prefix, flag): (&str, &mut bool) = if rest.starts_with("ctrl-") {
                ("ctrl-", &mut mods.ctrl)
            } else if rest.starts_with("alt-") {
                ("alt-", &mut mods.alt)
            } else if rest.starts_with("shift-") {
                ("shift-", &mut mods.shift)
            } else if rest.starts_with("meta-") {
                ("meta-", &mut mods.meta)
            } else {
                break;
            };
            *flag = true;
            rest = &rest[prefix.len()..];
        }
        if rest.is_empty() {
            return None;
        }
        Some(Self { mods, key: rest.to_string() })
    }

    /// The form [`Keystroke::parse`] reads: "ctrl-shift-a".
    pub fn id(&self) -> String {
        let m = &self.mods;
        let mut s = String::new();
        for (on, p) in [(m.ctrl, "ctrl-"), (m.alt, "alt-"), (m.shift, "shift-"), (m.meta, "meta-")] {
            if on {
                s.push_str(p);
            }
        }
        s + &self.key
    }

    pub fn is_modifier_only(&self) -> bool {
        matches!(self.key.as_str(), "shift" | "ctrl" | "control" | "alt" | "meta")
    }
}

impl fmt::Display for Keystroke {
    /// PyCharm-style label: "Ctrl+Shift+A".
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        let mut parts: Vec<String> = Vec::new();
        if self.mods.ctrl {
            parts.push("Ctrl".into());
        }
        if self.mods.alt {
            parts.push("Alt".into());
        }
        if self.mods.shift {
            parts.push("Shift".into());
        }
        if self.mods.meta {
            parts.push("Meta".into());
        }
        let key = match self.key.as_str() {
            "enter" => "Enter".to_string(),
            "escape" => "Esc".to_string(),
            "space" => "Space".to_string(),
            "tab" => "Tab".to_string(),
            "backspace" => "Backspace".to_string(),
            "delete" => "Delete".to_string(),
            "left" => "Left".to_string(),
            "right" => "Right".to_string(),
            "up" => "Up".to_string(),
            "down" => "Down".to_string(),
            "home" => "Home".to_string(),
            "end" => "End".to_string(),
            "pageup" => "PageUp".to_string(),
            "pagedown" => "PageDown".to_string(),
            k if k.starts_with('f') && k.len() > 1 && k[1..].chars().all(|c| c.is_ascii_digit()) => {
                k.to_ascii_uppercase()
            }
            k => k.to_ascii_uppercase(),
        };
        parts.push(key);
        write!(f, "{}", parts.join("+"))
    }
}

/// What triggers an action.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Trigger {
    /// One or more keystrokes pressed in sequence (PyCharm "second stroke").
    Keys(Vec<Keystroke>),
    /// Modifier pressed twice quickly with nothing in between (Shift Shift).
    DoubleTap(&'static str),
}

impl fmt::Display for Trigger {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Trigger::Keys(ks) => {
                let parts: Vec<String> = ks.iter().map(|k| k.to_string()).collect();
                write!(f, "{}", parts.join(", "))
            }
            Trigger::DoubleTap(m) => {
                let mut c = m.chars();
                let cap = c.next().map(|x| x.to_ascii_uppercase().to_string() + c.as_str()).unwrap_or_default();
                write!(f, "Double {cap}")
            }
        }
    }
}

#[derive(Clone, Debug)]
pub struct Binding {
    pub trigger: Trigger,
    pub action: Action,
    /// `None` = everywhere, otherwise only when this context is focused ("Editor", "ProjectView", ...).
    pub context: Option<&'static str>,
}

#[derive(Clone, Debug, Default)]
pub struct Keymap {
    pub bindings: Vec<Binding>,
}

impl Keymap {
    pub fn bind(&mut self, keys: &str, action: Action, context: Option<&'static str>) {
        let strokes: Vec<Keystroke> = keys
            .split_whitespace()
            .map(|k| Keystroke::parse(k).unwrap_or_else(|| panic!("bad keystroke {k:?} in {keys:?}")))
            .collect();
        self.bindings.push(Binding { trigger: Trigger::Keys(strokes), action, context });
    }

    pub fn bind_double_tap(&mut self, modifier: &'static str, action: Action) {
        self.bindings.push(Binding { trigger: Trigger::DoubleTap(modifier), action, context: None });
    }

    /// All shortcuts for an action, for menus and Find Action.
    pub fn triggers_for(&self, action: Action) -> Vec<&Trigger> {
        self.bindings.iter().filter(|b| b.action == action).map(|b| &b.trigger).collect()
    }

    pub fn shortcut_label(&self, action: Action) -> Option<String> {
        self.triggers_for(action).first().map(|t| t.to_string())
    }

    fn context_matches(binding: Option<&'static str>, active: &[&str]) -> bool {
        match binding {
            None => true,
            Some(c) => active.contains(&c),
        }
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Resolution {
    /// No binding; the key should go to the focused widget (e.g. typed into the editor).
    Unhandled,
    /// Prefix of a multi-stroke binding; wait for the next stroke.
    Pending,
    Action(Action),
}

/// Stateful resolver: tracks partial chords and double-tap timing.
pub struct KeyResolver {
    keymap: Keymap,
    pending: Vec<Keystroke>,
    last_modifier_tap: Option<(String, Instant)>,
    pub double_tap_window: Duration,
}

impl KeyResolver {
    pub fn new(keymap: Keymap) -> Self {
        Self {
            keymap,
            pending: Vec::new(),
            last_modifier_tap: None,
            double_tap_window: Duration::from_millis(350),
        }
    }

    pub fn keymap(&self) -> &Keymap {
        &self.keymap
    }

    pub fn has_pending(&self) -> bool {
        !self.pending.is_empty()
    }

    /// A key-down of a non-modifier key. `contexts` lists focused contexts, innermost first.
    pub fn key_down(&mut self, stroke: Keystroke, contexts: &[&str]) -> Resolution {
        // Any real key breaks a double-tap sequence.
        self.last_modifier_tap = None;
        self.pending.push(stroke);

        let mut best: Option<(usize, Action)> = None; // (context rank, action)
        let mut is_prefix = false;
        for b in &self.keymap.bindings {
            let Trigger::Keys(ks) = &b.trigger else { continue };
            if !Keymap::context_matches(b.context, contexts) {
                continue;
            }
            if ks.len() == self.pending.len() && *ks == self.pending {
                // Prefer the most specific (innermost) context; global bindings rank last.
                let rank = b
                    .context
                    .and_then(|c| contexts.iter().position(|x| *x == c))
                    .unwrap_or(usize::MAX);
                if best.map_or(true, |(r, _)| rank < r) {
                    best = Some((rank, b.action));
                }
            } else if ks.len() > self.pending.len() && ks[..self.pending.len()] == self.pending[..] {
                is_prefix = true;
            }
        }

        if let Some((_, action)) = best {
            self.pending.clear();
            return Resolution::Action(action);
        }
        if is_prefix {
            return Resolution::Pending;
        }
        self.pending.clear();
        Resolution::Unhandled
    }

    /// A modifier key was pressed and released alone (no other key in between).
    pub fn modifier_tapped(&mut self, modifier: &str, now: Instant) -> Resolution {
        let modifier = modifier.to_ascii_lowercase();
        if let Some((prev, at)) = &self.last_modifier_tap {
            if *prev == modifier && now.duration_since(*at) <= self.double_tap_window {
                self.last_modifier_tap = None;
                for b in &self.keymap.bindings {
                    if let Trigger::DoubleTap(m) = b.trigger {
                        if m == modifier {
                            return Resolution::Action(b.action);
                        }
                    }
                }
                return Resolution::Unhandled;
            }
        }
        self.last_modifier_tap = Some((modifier, now));
        Resolution::Unhandled
    }

    pub fn cancel(&mut self) {
        self.pending.clear();
        self.last_modifier_tap = None;
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn ks(s: &str) -> Keystroke {
        Keystroke::parse(s).unwrap()
    }

    #[test]
    fn parse_and_display() {
        let k = ks("ctrl-shift-a");
        assert!(k.mods.ctrl && k.mods.shift && !k.mods.alt);
        assert_eq!(k.key, "a");
        assert_eq!(k.to_string(), "Ctrl+Shift+A");
        assert_eq!(ks("alt-f12").to_string(), "Alt+F12");
        assert_eq!(ks("ctrl-/").key, "/");
    }

    #[test]
    fn resolves_pycharm_defaults() {
        let mut r = KeyResolver::new(pycharm_keymap());
        assert_eq!(r.key_down(ks("ctrl-shift-a"), &["Editor"]), Resolution::Action(Action::FindAction));
        assert_eq!(r.key_down(ks("alt-1"), &["Editor"]), Resolution::Action(Action::ToggleProjectWindow));
        assert_eq!(r.key_down(ks("ctrl-b"), &["Editor"]), Resolution::Action(Action::GotoDeclaration));
        assert_eq!(r.key_down(ks("x"), &["Editor"]), Resolution::Unhandled);
    }

    #[test]
    fn double_shift() {
        let mut r = KeyResolver::new(pycharm_keymap());
        let t0 = Instant::now();
        assert_eq!(r.modifier_tapped("shift", t0), Resolution::Unhandled);
        assert_eq!(
            r.modifier_tapped("shift", t0 + Duration::from_millis(200)),
            Resolution::Action(Action::SearchEverywhere)
        );
        // Too slow: no action.
        let t1 = t0 + Duration::from_secs(5);
        r.modifier_tapped("shift", t1);
        assert_eq!(r.modifier_tapped("shift", t1 + Duration::from_secs(1)), Resolution::Unhandled);
    }

    #[test]
    fn context_specificity() {
        let mut km = Keymap::default();
        km.bind("enter", Action::EditorNewline, Some("Editor"));
        km.bind("enter", Action::OpenSelected, Some("ProjectView"));
        let mut r = KeyResolver::new(km);
        assert_eq!(r.key_down(ks("enter"), &["ProjectView"]), Resolution::Action(Action::OpenSelected));
        assert_eq!(r.key_down(ks("enter"), &["Editor"]), Resolution::Action(Action::EditorNewline));
    }

    #[test]
    fn chords() {
        let mut km = Keymap::default();
        km.bind("ctrl-k ctrl-c", Action::CommentLine, None);
        let mut r = KeyResolver::new(km);
        assert_eq!(r.key_down(ks("ctrl-k"), &[]), Resolution::Pending);
        assert_eq!(r.key_down(ks("ctrl-c"), &[]), Resolution::Action(Action::CommentLine));
    }

    #[test]
    fn every_action_has_info() {
        for a in ALL_ACTIONS {
            assert!(!a.info().title.is_empty());
        }
    }
}
