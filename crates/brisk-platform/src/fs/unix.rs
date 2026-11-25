use std::path::Path;

/// A rename is already atomic here; the caller copies the permissions and renames.
pub fn replace_file(_: &Path, _: &Path) -> bool {
    false
}
