use super::*;

#[test]
fn environment_expansion_supports_required_and_default_values() {
    let lookup = |name: &str| match name {
        "USER" => Some("signing".to_owned()),
        "EMPTY" => Some(String::new()),
        _ => None,
    };
    assert_eq!(
        expand_string("/Users/${USER}/${MISSING:-runner}", &lookup)
            .unwrap_or_else(|error| unreachable!("expand: {error}")),
        "/Users/signing/runner"
    );
    assert_eq!(
        expand_string("${EMPTY:-fallback}", &lookup)
            .unwrap_or_else(|error| unreachable!("expand: {error}")),
        "fallback"
    );
    assert!(matches!(
        expand_string("${MISSING}", &lookup),
        Err(ConfigLoadError::MissingEnvironment(name)) if name == "MISSING"
    ));
}

#[test]
fn a_required_placeholder_fails_closed_under_an_empty_environment() {
    let value: toml::Value =
        toml::from_str("[tailscale]\npreauth_key_file = '${FLANFORGE_TAILSCALE_PREAUTH_KEY_FILE}'")
            .unwrap_or_else(|error| unreachable!("fixture: {error}"));
    assert!(matches!(
        decode_config(value, &|_| None, None),
        Err(ConfigLoadError::MissingEnvironment(name))
            if name == "FLANFORGE_TAILSCALE_PREAUTH_KEY_FILE"
    ));
}

#[test]
fn expansion_is_data_and_cannot_add_toml_keys() {
    let mut value: toml::Value = toml::from_str("name = '${VALUE}'")
        .unwrap_or_else(|error| unreachable!("fixture: {error}"));
    expand_value(&mut value, &|_| Some("safe'\ninjected = true".to_owned()))
        .unwrap_or_else(|error| unreachable!("expand: {error}"));
    assert_eq!(value.get("injected"), None);
    assert_eq!(
        value.get("name").and_then(toml::Value::as_str),
        Some("safe'\ninjected = true")
    );
}

#[test]
fn malformed_placeholders_are_rejected() {
    for value in ["${}", "${9KEY}", "${BAD-NAME}", "${UNCLOSED"] {
        assert!(
            matches!(
                expand_string(value, &|_| Some("value".to_owned())),
                Err(ConfigLoadError::InvalidPlaceholder)
            ),
            "accepted {value}"
        );
    }
}

#[test]
fn tilde_paths_resolve_only_from_the_service_home() {
    let mut nested = PathBuf::from("~/Library/Logs/flanforged.log");
    resolve_home(&mut nested, Some(Path::new("/Users/signing")))
        .unwrap_or_else(|error| unreachable!("resolve: {error}"));
    assert_eq!(
        nested,
        PathBuf::from("/Users/signing/Library/Logs/flanforged.log")
    );

    let mut other_user = PathBuf::from("~someone/file");
    resolve_home(&mut other_user, Some(Path::new("/Users/signing")))
        .unwrap_or_else(|error| unreachable!("resolve: {error}"));
    assert_eq!(other_user, PathBuf::from("~someone/file"));

    let mut missing = PathBuf::from("~/file");
    assert!(matches!(
        resolve_home(&mut missing, None),
        Err(ConfigLoadError::HomeUnavailable)
    ));
}

#[test]
fn tailscale_key_path_resolves_from_the_service_home() {
    let mut config: flanforge_core::Config =
        toml::from_str(include_str!("../../../config.example.toml"))
            .unwrap_or_else(|error| unreachable!("fixture: {error}"));
    config.tailscale.preauth_key_file = Some("~/Library/Secrets/headscale-key".into());
    let mut value =
        toml::Value::try_from(config).unwrap_or_else(|error| unreachable!("fixture: {error}"));
    expand_value(&mut value, &|_| None).unwrap_or_else(|error| unreachable!("expand: {error}"));
    let resolved = decode_config(value, &|_| None, Some(Path::new("/Users/signing")))
        .unwrap_or_else(|error| unreachable!("decode: {error}"));
    assert_eq!(
        resolved.tailscale.preauth_key_file,
        Some(PathBuf::from(
            "/Users/signing/Library/Secrets/headscale-key"
        ))
    );
}
