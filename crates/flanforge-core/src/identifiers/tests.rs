use super::*;

#[test]
fn repository_requires_exactly_two_safe_segments() {
    for invalid in [
        "owner",
        "/repo",
        "owner/",
        "owner/repo/extra",
        "../repo",
        "owner/repo name",
    ] {
        assert!(RepositoryName::new(invalid).is_err(), "accepted {invalid}");
    }
    assert_eq!(
        RepositoryName::new("owner/repo.name")
            .map(|name| (name.owner().to_owned(), name.repository().to_owned())),
        Ok(("owner".to_owned(), "repo.name".to_owned()))
    );
}

#[test]
fn names_reject_shell_and_path_characters() {
    for invalid in [
        "ci/name", "ci name", "ci$name", "ci;name", "ci'name", "--help", ".hidden",
    ] {
        assert!(VmName::new(invalid).is_err(), "accepted {invalid}");
        assert!(RunnerLabel::new(invalid).is_err(), "accepted {invalid}");
    }
}

#[test]
fn repositories_reject_option_like_segments() {
    for invalid in ["-owner/repo", "owner/-repo", ".owner/repo", "owner/.repo"] {
        assert!(RepositoryName::new(invalid).is_err(), "accepted {invalid}");
    }
}

#[test]
fn vm_prefix_must_end_in_hyphen() {
    let valid = VmPrefix::new("ci-");
    assert!(valid.is_ok());
    assert!(valid.is_ok_and(|prefix| prefix.ensure_valid().is_ok()));

    let invalid = VmPrefix::new("bad");
    assert!(invalid.is_ok());
    assert!(invalid.is_ok_and(|prefix| prefix.ensure_valid().is_err()));
}

#[test]
fn profile_names_are_addressable_by_toml_and_cli_overlays() {
    // Hyphenated names are deliberately admitted even though the environment
    // overlay cannot spell them; TOML and --set still address them.
    for valid in ["ci", "ci_2", "2_ci", "ci-linux"] {
        assert!(ProfileName::new(valid).is_ok(), "rejected {valid}");
    }
    for invalid in ["CI", "ci.linux", "_ci", "-ci"] {
        assert!(ProfileName::new(invalid).is_err(), "accepted {invalid}");
    }
}
