//! Convert GPUI keystrokes into brisk-keymap keystrokes.
//!
//! On Windows GPUI reports shifted punctuation as the produced character with `shift` cleared
//! (Ctrl+Shift+/ arrives as "ctrl-?"). PyCharm bindings are written against the physical key,
//! so we map those back. Letters come from the virtual key code, so shortcuts keep working
//! with non-Latin layouts (e.g. Russian).

use brisk_keymap::{Keystroke, Modifiers};

fn unshift(c: &str) -> Option<&'static str> {
    Some(match c {
        "?" => "/",
        "+" => "=",
        "_" => "-",
        "~" => "`",
        "!" => "1",
        "@" => "2",
        "#" => "3",
        "$" => "4",
        "%" => "5",
        "^" => "6",
        "&" => "7",
        "*" => "8",
        "(" => "9",
        ")" => "0",
        "{" => "[",
        "}" => "]",
        "|" => "\\",
        ":" => ";",
        "\"" => "'",
        "<" => ",",
        ">" => ".",
        _ => return None,
    })
}

pub fn convert(k: &gpui::Keystroke) -> Keystroke {
    let mut mods = Modifiers { ctrl: k.modifiers.control, alt: k.modifiers.alt, shift: k.modifiers.shift, meta: k.modifiers.platform };
    let mut key = k.key.to_ascii_lowercase();
    if let Some(base) = unshift(&key) {
        mods.shift = true;
        key = base.to_string();
    }
    Keystroke { mods, key }
}

/// The text a key press types, for views that read keys themselves (popups, the terminal,
/// small fields) instead of through an input handler. On Windows gpui gives Space no
/// `key_char`: the space comes as a character message, which only reaches an input handler.
pub fn typed_text(k: &gpui::Keystroke) -> Option<&str> {
    let m = &k.modifiers;
    match k.key_char.as_deref() {
        Some(t) if !t.is_empty() && !t.chars().any(char::is_control) => Some(t),
        None if k.key == "space" && !(m.control || m.alt || m.platform) => Some(" "),
        _ => None,
    }
}

/// True if the key event should be treated as text for the editor (no shortcut modifiers).
pub fn is_text_input(k: &gpui::Keystroke) -> bool {
    !(k.modifiers.control || k.modifiers.platform) || (k.modifiers.control && k.modifiers.alt)
}
