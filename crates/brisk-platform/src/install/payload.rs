//! The release zip rides at the end of the setup exe: `<exe><zip><zip length: u64 LE>BRISKZIP`.
//! `cargo xtask package-setup` appends it; the uninstaller is the same exe without it.
//!
//! Signing puts the Authenticode certificate table after everything else (padded to 8
//! bytes), so the trailer is looked for before that table when the PE header names one.

use std::io::{Read, Seek, SeekFrom, Write};
use std::path::Path;

const MAGIC: &[u8; 8] = b"BRISKZIP";
const TRAILER: u64 = 16;

/// Where the signature table starts, when `f` is a signed PE file; else its length.
fn content_end(f: &mut std::fs::File, len: u64) -> u64 {
    let mut read = |at: u64, buf: &mut [u8]| f.seek(SeekFrom::Start(at)).and_then(|_| f.read_exact(buf)).is_ok();
    let mut b4 = [0u8; 4];
    let mut b2 = [0u8; 2];
    if !read(0x3c, &mut b4) {
        return len;
    }
    let pe = u32::from_le_bytes(b4) as u64;
    if !read(pe, &mut b4) || &b4 != b"PE\0\0" || !read(pe + 24, &mut b2) {
        return len;
    }
    // The data directories follow the optional header's fixed part: 96 bytes in PE32, 112 in
    // PE32+. The security directory is the fifth; its "address" is a file offset.
    let dirs = pe + 24 + if u16::from_le_bytes(b2) == 0x20b { 112 } else { 96 };
    let mut d = [0u8; 8];
    if !read(dirs + 4 * 8, &mut d) {
        return len;
    }
    let (at, size) = (u32::from_le_bytes(d[..4].try_into().unwrap_or_default()) as u64, u32::from_le_bytes(d[4..].try_into().unwrap_or_default()) as u64);
    if at > 0 && size > 0 && at + size <= len {
        at
    } else {
        len
    }
}

/// Where the zip starts in `exe` and how long it is, when it carries one.
pub fn find(exe: &Path) -> Option<(u64, u64)> {
    let mut f = std::fs::File::open(exe).ok()?;
    let len = f.metadata().ok()?.len();
    let end = content_end(&mut f, len);
    // Up to 7 bytes of padding may sit between the trailer and the signature.
    for pad in 0..8u64 {
        let Some(at) = end.checked_sub(TRAILER + pad) else { break };
        f.seek(SeekFrom::Start(at)).ok()?;
        let mut t = [0u8; 16];
        f.read_exact(&mut t).ok()?;
        if &t[8..] == MAGIC {
            let zip_len = u64::from_le_bytes(t[..8].try_into().ok()?);
            return Some((at.checked_sub(zip_len)?, zip_len));
        }
        if end == len {
            break;
        }
    }
    None
}

/// Copies the first `len` bytes of `src` to `dest`: the exe without its payload.
pub fn copy_head(src: &Path, len: u64, dest: &Path) -> std::io::Result<()> {
    let f = std::fs::File::open(src)?;
    let mut out = std::fs::File::create(dest)?;
    std::io::copy(&mut f.take(len), &mut out)?;
    out.flush()
}

/// Writes the payload zip of `exe` to `dest`.
pub fn extract(exe: &Path, (start, len): (u64, u64), dest: &Path) -> std::io::Result<()> {
    let mut f = std::fs::File::open(exe)?;
    f.seek(SeekFrom::Start(start))?;
    let mut out = std::fs::File::create(dest)?;
    std::io::copy(&mut f.take(len), &mut out)?;
    out.flush()
}

/// Appends `zip` to `exe` as its payload (the packaging step, before signing).
pub fn append(exe: &Path, zip: &Path) -> std::io::Result<()> {
    let data = std::fs::read(zip)?;
    let mut f = std::fs::OpenOptions::new().append(true).open(exe)?;
    f.write_all(&data)?;
    f.write_all(&(data.len() as u64).to_le_bytes())?;
    f.write_all(MAGIC)?;
    f.flush()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn round_trip() {
        let d = std::env::temp_dir().join(format!("brisk-payload-{}", std::process::id()));
        std::fs::create_dir_all(&d).unwrap();
        let (exe, zip) = (d.join("s.exe"), d.join("p.zip"));
        std::fs::write(&exe, b"MZ program").unwrap();
        assert_eq!(find(&exe), None);
        std::fs::write(&zip, b"PK zip bytes").unwrap();
        append(&exe, &zip).unwrap();
        let at = find(&exe).unwrap();
        assert_eq!(at, (10, 12));
        extract(&exe, at, &d.join("out.zip")).unwrap();
        assert_eq!(std::fs::read(d.join("out.zip")).unwrap(), b"PK zip bytes");
        copy_head(&exe, at.0, &d.join("u.exe")).unwrap();
        assert_eq!(std::fs::read(d.join("u.exe")).unwrap(), b"MZ program");
        let _ = std::fs::remove_dir_all(&d);
    }

    #[test]
    fn found_before_a_signature() {
        // A minimal PE32+ header whose security directory points past the payload.
        let d = std::env::temp_dir().join(format!("brisk-payload-sig-{}", std::process::id()));
        std::fs::create_dir_all(&d).unwrap();
        let (exe, zip) = (d.join("s.exe"), d.join("p.zip"));
        let mut head = vec![0u8; 0x200];
        head[..2].copy_from_slice(b"MZ");
        head[0x3c..0x40].copy_from_slice(&0x80u32.to_le_bytes());
        head[0x80..0x84].copy_from_slice(b"PE\0\0");
        head[0x98..0x9a].copy_from_slice(&0x20bu16.to_le_bytes());
        std::fs::write(&exe, &head).unwrap();
        std::fs::write(&zip, b"PK zip").unwrap();
        append(&exe, &zip).unwrap();
        let mut bytes = std::fs::read(&exe).unwrap();
        bytes.extend_from_slice(&[0, 0]); // padding to 8
        let sig_at = bytes.len() as u32;
        bytes.extend_from_slice(&[7u8; 24]); // the "certificate table"
        let dir = 0x98 + 112 + 32;
        bytes[dir..dir + 4].copy_from_slice(&sig_at.to_le_bytes());
        bytes[dir + 4..dir + 8].copy_from_slice(&24u32.to_le_bytes());
        std::fs::write(&exe, &bytes).unwrap();
        assert_eq!(find(&exe), Some((0x200, 6)));
        let _ = std::fs::remove_dir_all(&d);
    }
}
