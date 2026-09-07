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

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct TimeError;

impl std::fmt::Display for TimeError {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter.write_str("system clock is not after the Unix epoch")
    }
}

impl std::error::Error for TimeError {}

/// Returns nonzero seconds since the Unix epoch.
///
/// # Errors
/// Returns an error when the system clock is at or before the epoch.
pub fn try_unix_time() -> Result<u64, TimeError> {
    let seconds = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map_err(|_| TimeError)?
        .as_secs();
    (seconds != 0).then_some(seconds).ok_or(TimeError)
}

/// Returns whether an allowlist pattern matches a value in full.
///
/// `*` matches any run of characters including `/`, `?` matches exactly one,
/// and every other byte is literal. There are no character classes, no
/// alternation and no partial matches: a pattern authorises what it spells out
/// and nothing adjacent to it. Iterative with a single backtrack point, so a
/// pattern of repeated wildcards cannot cost exponential time.
#[must_use]
pub fn is_glob_match(pattern: &str, value: &str) -> bool {
    let (pattern, value) = (pattern.as_bytes(), value.as_bytes());
    let (mut p, mut v) = (0, 0);
    // Where to resume if the current `*` turns out to have consumed too little.
    let (mut star, mut resume) = (None, 0);
    while v < value.len() {
        match pattern.get(p) {
            Some(b'*') => {
                star = Some(p);
                resume = v;
                p += 1;
            }
            Some(&byte) if byte == b'?' || byte == value[v] => {
                p += 1;
                v += 1;
            }
            _ => match star {
                Some(index) => {
                    p = index + 1;
                    resume += 1;
                    v = resume;
                }
                None => return false,
            },
        }
    }
    pattern[p..].iter().all(|&byte| byte == b'*')
}

/// Returns whether a Git ref is safe for policy comparison and command data.
#[must_use]
pub fn is_safe_git_ref(value: &str) -> bool {
    is_safe_ref_shape(value, false)
}

/// Returns whether a ref allowlist pattern is safe.
///
/// Identical to `is_safe_git_ref` but for `*` and `?`. The `refs/` prefix is
/// still required, so a pattern names a namespace rather than authorising every
/// ref by accident; write `refs/*` when that is genuinely the intent.
#[must_use]
pub fn is_safe_git_ref_pattern(value: &str) -> bool {
    is_safe_ref_shape(value, true)
}

fn is_safe_ref_shape(value: &str, wildcards: bool) -> bool {
    if !value.starts_with("refs/")
        || value.len() > 256
        || value.ends_with(['/', '.'])
        || value.contains("//")
        || value.contains("..")
        || value.contains("@{")
        || !value.bytes().all(|byte| {
            byte.is_ascii_alphanumeric()
                || matches!(byte, b'/' | b'-' | b'_' | b'.')
                || (wildcards && matches!(byte, b'*' | b'?'))
        })
    {
        return false;
    }
    value.split('/').skip(1).all(|segment| {
        !segment.is_empty()
            && !segment.starts_with('.')
            && !segment.to_ascii_lowercase().ends_with(".lock")
    })
}

/// Returns whether a libvirt connection URI is one flanforge will drive.
///
/// Accepts the local socket and the authenticated remote transports. `tcp`
/// carries libvirt's root-equivalent API in clear text, so it is admitted only
/// when the operator opts in. A query string is always refused: it is where
/// `no_verify` disables TLS peer checking.
#[must_use]
pub fn is_supported_libvirt_uri(uri: &str, is_insecure_allowed: bool) -> bool {
    if uri.is_empty()
        || uri.len() > 255
        || uri.contains(['?', '#', ' '])
        || !uri.bytes().all(|byte| byte.is_ascii_graphic())
    {
        return false;
    }
    let Some((scheme, rest)) = uri.split_once("://") else {
        return false;
    };
    let transport = match scheme {
        "qemu" => "unix",
        _ => match scheme.strip_prefix("qemu+") {
            Some(transport) => transport,
            None => return false,
        },
    };
    let Some((authority, path)) = rest.split_once('/') else {
        return false;
    };
    if !matches!(path, "system" | "session") {
        return false;
    }
    match transport {
        "unix" => authority.is_empty(),
        "tcp" if !is_insecure_allowed => false,
        "ssh" | "tls" | "tcp" => is_libvirt_authority(authority),
        _ => false,
    }
}

/// `[user@]host[:port]`, where the host is a name or literal address.
fn is_libvirt_authority(authority: &str) -> bool {
    let host = match authority.split_once('@') {
        Some((user, host)) => {
            if user.is_empty()
                || !user
                    .bytes()
                    .all(|byte| byte.is_ascii_alphanumeric() || matches!(byte, b'-' | b'_' | b'.'))
            {
                return false;
            }
            host
        }
        None => authority,
    };
    let host = match host.rsplit_once(':') {
        Some((host, port)) => {
            if port.is_empty()
                || !port.bytes().all(|byte| byte.is_ascii_digit())
                || port.parse::<u16>().is_err()
            {
                return false;
            }
            host
        }
        None => host,
    };
    !host.is_empty()
        && host.len() <= 253
        && !host.starts_with('.')
        && !host.ends_with('.')
        && !host.contains("..")
        && host
            .bytes()
            .all(|byte| byte.is_ascii_alphanumeric() || matches!(byte, b'-' | b'.'))
}

#[cfg(test)]
mod tests {
    use super::{
        bounded_text, is_glob_match, is_safe_git_ref, is_safe_git_ref_pattern,
        is_supported_libvirt_uri,
    };

    #[test]
    fn authenticated_libvirt_transports_are_supported() {
        for uri in [
            "qemu:///system",
            "qemu:///session",
            "qemu+unix:///system",
            "qemu+ssh://host/system",
            "qemu+ssh://operator@host.example/system",
            "qemu+ssh://host.example:2222/system",
            "qemu+tls://host.example/system",
        ] {
            assert!(is_supported_libvirt_uri(uri, false), "refused {uri}");
        }
    }

    #[test]
    fn clear_text_libvirt_transport_needs_the_operator_opt_in() {
        assert!(!is_supported_libvirt_uri("qemu+tcp://host/system", false));
        assert!(is_supported_libvirt_uri("qemu+tcp://host/system", true));
    }

    #[test]
    fn unsupported_libvirt_uris_are_refused() {
        for uri in [
            "",
            "qemu:///",
            "qemu:///systemd",
            "qemu://host/system",
            "xen:///system",
            "qemu+rdp://host/system",
            "qemu+ssh:///system",
            "qemu+ssh://@host/system",
            "qemu+ssh://host:/system",
            "qemu+ssh://host:70000/system",
            "qemu+ssh://ho st/system",
            "qemu+tls://host/system?no_verify=1",
            "qemu+ssh://host/system#fragment",
            "qemu+ssh://.host/system",
            "qemu+ssh://host../system",
            "/var/run/libvirt/libvirt-sock",
        ] {
            assert!(!is_supported_libvirt_uri(uri, true), "accepted {uri}");
        }
    }

    #[test]
    fn a_pattern_without_wildcards_matches_only_itself() {
        assert!(is_glob_match("refs/heads/main", "refs/heads/main"));
        for other in [
            "refs/heads/mainx",
            "refs/heads/main/x",
            "xrefs/heads/main",
            "refs/heads/mai",
            "",
        ] {
            assert!(!is_glob_match("refs/heads/main", other), "matched {other}");
        }
    }

    #[test]
    fn wildcards_are_anchored_at_both_ends() {
        assert!(is_glob_match("refs/tags/v*", "refs/tags/v1.2.3"));
        assert!(is_glob_match("refs/tags/v*", "refs/tags/v"));
        assert!(!is_glob_match("refs/tags/v*", "refs/tags/other"));
        assert!(!is_glob_match("refs/tags/v*", "xrefs/tags/v1"));
        assert!(!is_glob_match("*.yml", "apple.yml.bak"));
        assert!(is_glob_match("*.yml", "apple.yml"));
    }

    #[test]
    fn a_star_crosses_path_separators_and_a_question_mark_takes_exactly_one() {
        assert!(is_glob_match("refs/heads/*", "refs/heads/feature/apple"));
        assert!(is_glob_match("*", "anything/at/all"));
        assert!(is_glob_match("", ""));
        assert!(!is_glob_match("", "x"));

        assert!(is_glob_match("ios-v?", "ios-v1"));
        assert!(!is_glob_match("ios-v?", "ios-v"));
        assert!(!is_glob_match("ios-v?", "ios-v12"));
    }

    #[test]
    fn repeated_wildcards_terminate_without_backtracking_blowup() {
        let pattern = "refs/heads/*a*a*a*a*a*a*a*a*a*b";
        let value = format!("refs/heads/{}", "a".repeat(2_048));
        assert!(!is_glob_match(pattern, &value));
        assert!(is_glob_match(pattern, &format!("{value}b")));
        assert!(is_glob_match("****", "abc"));
    }

    #[test]
    fn ref_patterns_allow_wildcards_but_keep_every_other_rule() {
        for valid in ["refs/tags/v*", "refs/heads/*", "refs/heads/release-?"] {
            assert!(is_safe_git_ref_pattern(valid), "rejected {valid}");
            assert!(!is_safe_git_ref(valid), "plain rule accepted {valid}");
        }
        for invalid in ["*", "heads/*", "refs/heads/../*", "refs/heads/*;touch"] {
            assert!(!is_safe_git_ref_pattern(invalid), "accepted {invalid}");
        }
    }

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
