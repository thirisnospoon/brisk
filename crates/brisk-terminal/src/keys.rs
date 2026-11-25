//! Key → byte sequence encoding (xterm conventions), shared by Windows ConPTY and Unix PTYs.

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct KeyMods {
    pub ctrl: bool,
    pub alt: bool,
    pub shift: bool,
}

impl KeyMods {
    /// xterm modifier parameter: 1 + shift + 2*alt + 4*ctrl.
    fn param(self) -> u8 {
        1 + self.shift as u8 + 2 * self.alt as u8 + 4 * self.ctrl as u8
    }
    fn any(self) -> bool {
        self.ctrl || self.alt || self.shift
    }
}

/// Encode a key. `key` is a lowercase key name ("a", "enter", "left", "f5"); `text` is the
/// character the layout produced (e.g. "A", "ж", "?"), if any.
pub fn encode_key(key: &str, text: Option<&str>, m: KeyMods, app_cursor: bool) -> Option<Vec<u8>> {
    let csi_mod = |final_: char| -> Vec<u8> {
        if m.any() {
            format!("\x1b[1;{}{final_}", m.param()).into_bytes()
        } else if app_cursor {
            format!("\x1bO{final_}").into_bytes()
        } else {
            format!("\x1b[{final_}").into_bytes()
        }
    };
    let tilde = |n: u8| -> Vec<u8> {
        if m.any() {
            format!("\x1b[{n};{}~", m.param()).into_bytes()
        } else {
            format!("\x1b[{n}~").into_bytes()
        }
    };
    let ss3_or_mod = |final_: char| -> Vec<u8> {
        if m.any() {
            format!("\x1b[1;{}{final_}", m.param()).into_bytes()
        } else {
            format!("\x1bO{final_}").into_bytes()
        }
    };
    let alt_prefix = |mut v: Vec<u8>| -> Vec<u8> {
        if m.alt {
            v.insert(0, 0x1b);
        }
        v
    };

    let out = match key {
        "enter" => alt_prefix(vec![b'\r']),
        "tab" if m.shift => b"\x1b[Z".to_vec(),
        "tab" => vec![b'\t'],
        "escape" => vec![0x1b],
        "backspace" if m.ctrl => alt_prefix(vec![0x17]), // delete word, like most terminals
        "backspace" => alt_prefix(vec![0x7f]),
        "space" if m.ctrl => vec![0],
        // gpui on Windows gives Space no text (the space comes as a character message).
        "space" => alt_prefix(vec![b' ']),
        "up" => csi_mod('A'),
        "down" => csi_mod('B'),
        "right" => csi_mod('C'),
        "left" => csi_mod('D'),
        "home" => csi_mod('H'),
        "end" => csi_mod('F'),
        "insert" => tilde(2),
        "delete" => tilde(3),
        "pageup" => tilde(5),
        "pagedown" => tilde(6),
        "f1" => ss3_or_mod('P'),
        "f2" => ss3_or_mod('Q'),
        "f3" => ss3_or_mod('R'),
        "f4" => ss3_or_mod('S'),
        "f5" => tilde(15),
        "f6" => tilde(17),
        "f7" => tilde(18),
        "f8" => tilde(19),
        "f9" => tilde(20),
        "f10" => tilde(21),
        "f11" => tilde(23),
        "f12" => tilde(24),
        _ => {
            if m.ctrl && !m.alt {
                // Ctrl+letter / Ctrl+[ \ ] ^ _ → C0 control codes.
                let k = key.as_bytes();
                if k.len() == 1 {
                    let c = k[0].to_ascii_lowercase();
                    let code = match c {
                        b'a'..=b'z' => Some(c - b'a' + 1),
                        b'[' | b'3' => Some(0x1b),
                        b'\\' | b'4' => Some(0x1c),
                        b']' | b'5' => Some(0x1d),
                        b'6' => Some(0x1e),
                        b'/' | b'-' | b'7' => Some(0x1f),
                        b'2' | b'@' => Some(0),
                        _ => None,
                    };
                    if let Some(code) = code {
                        return Some(vec![code]);
                    }
                }
                return None;
            }
            let t = text.filter(|t| !t.is_empty() && !t.chars().any(char::is_control))?;
            alt_prefix(t.as_bytes().to_vec())
        }
    };
    Some(out)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn m(ctrl: bool, alt: bool, shift: bool) -> KeyMods {
        KeyMods { ctrl, alt, shift }
    }

    #[test]
    fn basics() {
        assert_eq!(encode_key("a", Some("a"), m(false, false, false), false), Some(b"a".to_vec()));
        assert_eq!(encode_key("c", Some("c"), m(true, false, false), false), Some(vec![3]));
        assert_eq!(encode_key("enter", None, KeyMods::default(), false), Some(b"\r".to_vec()));
        assert_eq!(encode_key("up", None, KeyMods::default(), false), Some(b"\x1b[A".to_vec()));
        assert_eq!(encode_key("up", None, KeyMods::default(), true), Some(b"\x1bOA".to_vec()));
        assert_eq!(encode_key("right", None, m(true, false, false), false), Some(b"\x1b[1;5C".to_vec()));
        assert_eq!(encode_key("delete", None, KeyMods::default(), false), Some(b"\x1b[3~".to_vec()));
        assert_eq!(encode_key("b", Some("b"), m(false, true, false), false), Some(b"\x1bb".to_vec()));
        assert_eq!(encode_key("f", Some("ж"), KeyMods::default(), false), Some("ж".as_bytes().to_vec()));
        assert_eq!(encode_key("tab", None, m(false, false, true), false), Some(b"\x1b[Z".to_vec()));
        // Space, with or without the text gpui may leave out.
        assert_eq!(encode_key("space", None, KeyMods::default(), false), Some(b" ".to_vec()));
        assert_eq!(encode_key("space", Some(" "), m(false, false, true), false), Some(b" ".to_vec()));
        assert_eq!(encode_key("space", None, m(false, true, false), false), Some(b"\x1b ".to_vec()));
        assert_eq!(encode_key("space", None, m(true, false, false), false), Some(vec![0]));
    }
}
