use std::os::windows::ffi::OsStrExt;
use std::path::Path;
use windows_sys::Win32::Storage::FileSystem::{ReplaceFileW, REPLACEFILE_IGNORE_MERGE_ERRORS};

/// `ReplaceFileW`: false when it fails (e.g. another program holds the file open without
/// delete sharing), and the caller falls back to a rename.
pub fn replace_file(target: &Path, replacement: &Path) -> bool {
    let wide = |p: &Path| p.as_os_str().encode_wide().chain(Some(0)).collect::<Vec<u16>>();
    let (a, b) = (wide(target), wide(replacement));
    // SAFETY: both paths are NUL-terminated UTF-16 buffers that outlive the call.
    unsafe { ReplaceFileW(a.as_ptr(), b.as_ptr(), std::ptr::null(), REPLACEFILE_IGNORE_MERGE_ERRORS, std::ptr::null(), std::ptr::null()) != 0 }
}
