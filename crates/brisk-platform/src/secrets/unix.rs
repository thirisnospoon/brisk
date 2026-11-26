//! No keyring support yet (Secret Service, Keychain): a password is not written to disk in a
//! form anyone with the file could read, so saving is refused and nothing is loaded. The
//! data source dialog shows the error and keeps the password for the session.

pub fn seal(_: &str) -> std::io::Result<String> {
    Err(std::io::Error::new(
        std::io::ErrorKind::Unsupported,
        "passwords can only be saved with a system keyring, which Brisk does not support on this system yet; it is kept until Brisk closes",
    ))
}

pub fn open(_: &str) -> Option<String> {
    None
}

/// Readable by the owner only (0600).
pub fn private_file_options() -> std::fs::OpenOptions {
    use std::os::unix::fs::OpenOptionsExt;
    let mut o = std::fs::OpenOptions::new();
    o.write(true).create(true).truncate(true).mode(0o600);
    o
}
