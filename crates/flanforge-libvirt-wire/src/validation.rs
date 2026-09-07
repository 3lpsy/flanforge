use std::path::{Component, Path};

pub(crate) fn is_safe_name(value: &str, maximum_bytes: usize) -> bool {
    (1..=maximum_bytes).contains(&value.len())
        && value != "."
        && value != ".."
        && !value.contains("..")
        && value
            .bytes()
            .all(|byte| byte.is_ascii_alphanumeric() || matches!(byte, b'-' | b'_' | b'.'))
}

pub(crate) fn is_safe_key(value: &str, maximum_bytes: usize) -> bool {
    let path = Path::new(value);
    let bytes = value.as_bytes();
    !value.is_empty()
        && value.len() <= maximum_bytes
        && !bytes.iter().any(u8::is_ascii_control)
        && (!value.starts_with('/') || !bytes.windows(2).any(|window| window == b"//"))
        && !bytes
            .split(|byte| *byte == b'/')
            .any(|segment| segment == b"." || segment == b"..")
        && !path
            .components()
            .any(|component| matches!(component, Component::CurDir | Component::ParentDir))
        && (!value.starts_with('/') || path.is_absolute())
}

pub(crate) fn is_normal_absolute(path: &Path, maximum_bytes: usize) -> bool {
    let bytes = path.as_os_str().as_encoded_bytes();
    path.is_absolute()
        && !bytes.is_empty()
        && bytes.len() <= maximum_bytes
        && !bytes.windows(2).any(|window| window == b"//")
        && !bytes
            .split(|byte| *byte == b'/')
            .any(|segment| segment == b"." || segment == b"..")
        && !path
            .components()
            .any(|component| matches!(component, Component::CurDir | Component::ParentDir))
}

/// A POSIX account name the daemon may name in an argv it hands to the guest.
/// `is_safe_name` allows a leading `-` or `.`, which would be read as an option
/// or a relative path; volume and pool names still use it, so the extra rule
/// lives here rather than tightening theirs.
pub(crate) fn is_safe_account_name(value: &str) -> bool {
    is_safe_name(value, MAX_ACCOUNT_NAME_BYTES)
        && value
            .bytes()
            .next()
            .is_some_and(|byte| byte.is_ascii_alphanumeric())
}

const MAX_ACCOUNT_NAME_BYTES: usize = 32;
