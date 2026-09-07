use super::*;

#[test]
fn passwords_hash_verify_and_equalize_missing_credentials() {
    let phc = hash_password("correct horse").unwrap_or_else(|error| unreachable!("{error}"));
    assert!(phc.starts_with("$argon2id$"));
    assert!(verify_password("correct horse", &phc));
    assert!(!verify_password("wrong", &phc));
    assert!(!verify_password("correct horse", "not a phc string"));

    assert!(verify_password_or_dummy("correct horse", Some(&phc)));
    // A missing credential is always false, after a real verify's work.
    assert!(!verify_password_or_dummy("anything", None));
}

#[test]
fn session_tokens_are_opaque_and_stored_hashed() {
    let minted = mint_session_token().unwrap_or_else(|error| unreachable!("{error}"));
    assert_eq!(minted.token.len(), 64);
    assert_eq!(minted.token_hash, hash_token(&minted.token));
    assert_ne!(minted.token, minted.token_hash);
    let again = mint_session_token().unwrap_or_else(|error| unreachable!("{error}"));
    assert_ne!(minted.token, again.token);
}

#[test]
fn cookies_are_hardened_and_every_presented_value_is_checked() {
    let cookie = session_cookie("abc", 3_600, false);
    assert_eq!(
        cookie,
        "flanforge_session=abc; Path=/; HttpOnly; SameSite=Strict; Max-Age=3600"
    );
    assert!(session_cookie("abc", 60, true).ends_with("; Secure"));
    assert!(clearing_cookie().contains("Max-Age=0"));

    let hashes = cookie_token_hashes("other=1; flanforge_session=aaa; flanforge_session=bbb ; x=2");
    assert_eq!(hashes, vec![hash_token("aaa"), hash_token("bbb")]);
    assert!(cookie_token_hashes("flanforge_session=").is_empty());
}
