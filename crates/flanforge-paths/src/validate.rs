use std::path::{Component, Path};

use crate::PathError;

#[must_use]
pub fn is_absolute_normalized(path: &Path) -> bool {
    let bytes = path.as_os_str().as_encoded_bytes();
    path.is_absolute()
        && !bytes.windows(2).any(|window| window == b"//")
        && !path
            .components()
            .any(|component| matches!(component, Component::CurDir | Component::ParentDir))
        && !path
            .as_os_str()
            .as_encoded_bytes()
            .split(|byte| *byte == b'/')
            .any(|segment| segment == b"." || segment == b"..")
}

/// Ensures a path is absolute and has no current or parent segments.
///
/// # Errors
/// Returns an error for relative or non-normalized paths.
pub fn ensure_absolute_normalized(path: &Path) -> Result<(), PathError> {
    if !path.is_absolute() {
        return Err(PathError::PathNotAbsolute);
    }
    if !is_absolute_normalized(path) {
        return Err(PathError::PathNotNormalized);
    }
    Ok(())
}
