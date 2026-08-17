/// Bounds a string by UTF-8 bytes without splitting a character.
#[must_use]
pub fn bounded_text(value: impl Into<String>, max_bytes: usize) -> String {
    let mut value = value.into();
    if value.len() <= max_bytes {
        return value;
    }
    let mut boundary = max_bytes;
    while !value.is_char_boundary(boundary) {
        boundary = boundary.saturating_sub(1);
    }
    value.truncate(boundary);
    value
}

/// Seconds since the Unix epoch, zero when the clock precedes it.
#[must_use]
pub fn unix_time() -> u64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map_or(0, |duration| duration.as_secs())
}

/// Returns whether a Git ref is safe for policy comparison and command data.
#[must_use]
pub fn is_safe_git_ref(value: &str) -> bool {
    if !value.starts_with("refs/")
        || value.len() > 256
        || value.ends_with(['/', '.'])
        || value.contains("//")
        || value.contains("..")
        || value.contains("@{")
        || !value
            .bytes()
            .all(|byte| byte.is_ascii_alphanumeric() || matches!(byte, b'/' | b'-' | b'_' | b'.'))
    {
        return false;
    }
    value.split('/').skip(1).all(|segment| {
        !segment.is_empty()
            && !segment.starts_with('.')
            && !segment.to_ascii_lowercase().ends_with(".lock")
    })
}

#[cfg(test)]
mod tests {
    use super::{bounded_text, is_safe_git_ref};

    #[test]
    fn truncates_only_at_utf8_boundaries() {
        assert_eq!(
            bounded_text(format!("{}é", "x".repeat(511)), 512),
            "x".repeat(511)
        );
        assert_eq!(
            bounded_text(format!("{}€", "x".repeat(510)), 512),
            "x".repeat(510)
        );
        assert_eq!(bounded_text("🦀", 3), "");
        assert_eq!(bounded_text("🦀", 4), "🦀");
    }

    #[test]
    fn git_refs_have_a_conservative_command_safe_shape() {
        for valid in [
            "refs/heads/main",
            "refs/heads/feature/apple-build_1",
            "refs/tags/v1.2.3",
        ] {
            assert!(is_safe_git_ref(valid), "rejected {valid}");
        }
        for invalid in [
            "main",
            "refs/heads/main;touch",
            "refs/heads/main branch",
            "refs/heads/../main",
            "refs//main",
            "refs/heads/.hidden",
            "refs/heads/main.lock",
            "refs/heads/main/",
        ] {
            assert!(!is_safe_git_ref(invalid), "accepted {invalid}");
        }
    }
}
