//! Reading and writing files in their own encoding, as PyCharm does: UTF-8 (with or without a
//! BOM), UTF-16 with a BOM, and the system's legacy code page (e.g. windows-1251) for files that
//! are not valid UTF-8. Binary files are detected and not opened as text.

pub use encoding_rs;
use encoding_rs::{Encoding, UTF_16BE, UTF_16LE, UTF_8};

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct TextEncoding {
    pub encoding: &'static Encoding,
    /// Write a byte order mark (the file had one).
    pub bom: bool,
}

impl Default for TextEncoding {
    fn default() -> Self {
        Self { encoding: UTF_8, bom: false }
    }
}

impl TextEncoding {
    /// Status bar label: "UTF-8", "UTF-8 BOM", "windows-1251", "UTF-16LE".
    pub fn label(&self) -> String {
        let name = self.encoding.name();
        if self.bom && self.encoding == UTF_8 {
            "UTF-8 BOM".into()
        } else {
            name.into()
        }
    }
}

#[derive(Debug)]
pub enum Decoded {
    Text { text: String, encoding: TextEncoding },
    /// NUL bytes or other signs of a binary file.
    Binary,
}

/// The legacy (ANSI) code page of this machine: windows-1251 on Russian Windows.
pub fn system_legacy() -> &'static Encoding {
    #[cfg(windows)]
    {
        #[link(name = "kernel32")]
        extern "system" {
            fn GetACP() -> u32;
        }
        let cp = unsafe { GetACP() };
        let label = match cp {
            874 => "windows-874".to_string(),
            932 => "shift_jis".into(),
            936 => "gbk".into(),
            949 => "euc-kr".into(),
            950 => "big5".into(),
            1250..=1258 => format!("windows-{cp}"),
            _ => "windows-1252".into(),
        };
        Encoding::for_label(label.as_bytes()).unwrap_or(encoding_rs::WINDOWS_1252)
    }
    #[cfg(not(windows))]
    {
        encoding_rs::WINDOWS_1252
    }
}

fn looks_binary(bytes: &[u8]) -> bool {
    let head = &bytes[..bytes.len().min(8192)];
    head.contains(&0)
}

/// Decode file bytes. `legacy` is the fallback for non-UTF-8 files (see [`system_legacy`]).
pub fn decode(bytes: &[u8], legacy: &'static Encoding) -> Decoded {
    if let Some((enc, bom_len)) = Encoding::for_bom(bytes) {
        let (text, _) = enc.decode_without_bom_handling(&bytes[bom_len..]);
        return Decoded::Text { text: text.into_owned(), encoding: TextEncoding { encoding: enc, bom: true } };
    }
    if looks_binary(bytes) {
        return Decoded::Binary;
    }
    match std::str::from_utf8(bytes) {
        Ok(t) => Decoded::Text { text: t.to_string(), encoding: TextEncoding::default() },
        Err(_) => {
            let (text, _) = legacy.decode_without_bom_handling(bytes);
            Decoded::Text { text: text.into_owned(), encoding: TextEncoding { encoding: legacy, bom: false } }
        }
    }
}

/// Encode text for saving. Err: some characters do not exist in that encoding (the caller can
/// switch the file to UTF-8).
pub fn encode(text: &str, e: TextEncoding) -> Result<Vec<u8>, String> {
    let mut out = Vec::with_capacity(text.len() + 3);
    if e.encoding == UTF_16LE || e.encoding == UTF_16BE {
        // encoding_rs encodes to UTF-8 for UTF-16 targets; do it by hand.
        out.extend_from_slice(if e.encoding == UTF_16LE { &[0xFF, 0xFE] } else { &[0xFE, 0xFF] });
        for u in text.encode_utf16() {
            out.extend_from_slice(&if e.encoding == UTF_16LE { u.to_le_bytes() } else { u.to_be_bytes() });
        }
        return Ok(out);
    }
    if e.encoding == UTF_8 {
        if e.bom {
            out.extend_from_slice(&[0xEF, 0xBB, 0xBF]);
        }
        out.extend_from_slice(text.as_bytes());
        return Ok(out);
    }
    let (bytes, _, had_errors) = e.encoding.encode(text);
    if had_errors {
        let bad = text.chars().find(|c| {
            let mut b = [0u8; 4];
            e.encoding.encode(c.encode_utf8(&mut b)).2
        });
        return Err(format!("'{}' cannot be saved in {}", bad.unwrap_or('?'), e.encoding.name()));
    }
    out.extend_from_slice(&bytes);
    Ok(out)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn text(d: Decoded) -> (String, TextEncoding) {
        match d {
            Decoded::Text { text, encoding } => (text, encoding),
            Decoded::Binary => panic!("binary"),
        }
    }

    #[test]
    fn utf8_plain_and_bom_round_trip() {
        let (t, e) = text(decode("привет".as_bytes(), encoding_rs::WINDOWS_1251));
        assert_eq!((t.as_str(), e.label().as_str()), ("привет", "UTF-8"));
        let mut bom = vec![0xEF, 0xBB, 0xBF];
        bom.extend_from_slice("x = 1\n".as_bytes());
        let (t, e) = text(decode(&bom, encoding_rs::WINDOWS_1251));
        assert_eq!(t, "x = 1\n");
        assert_eq!(e.label(), "UTF-8 BOM");
        assert_eq!(encode(&t, e).unwrap(), bom);
    }

    #[test]
    fn legacy_cp1251_round_trip() {
        let (bytes, _, _) = encoding_rs::WINDOWS_1251.encode("# комментарий\n");
        let (t, e) = text(decode(&bytes, encoding_rs::WINDOWS_1251));
        assert_eq!(t, "# комментарий\n");
        assert_eq!(e.label(), "windows-1251");
        assert_eq!(encode(&t, e).unwrap(), bytes.into_owned());
        assert!(encode("emoji 🙂", e).unwrap_err().contains("🙂"));
    }

    #[test]
    fn utf16_with_bom_and_binary() {
        let mut b = vec![0xFF, 0xFE];
        for u in "a=ё".encode_utf16() {
            b.extend_from_slice(&u.to_le_bytes());
        }
        let (t, e) = text(decode(&b, encoding_rs::WINDOWS_1251));
        assert_eq!(t, "a=ё");
        assert_eq!(e.label(), "UTF-16LE");
        assert_eq!(encode(&t, e).unwrap(), b);
        assert!(matches!(decode(&[0x50, 0x4b, 0x03, 0x04, 0, 0, 1], encoding_rs::WINDOWS_1251), Decoded::Binary));
    }
}
