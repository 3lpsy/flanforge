use super::ensure_identity;

#[test]
fn configured_pool_volume_must_match_both_durable_name_and_key() {
    assert!(ensure_identity("root-a", Some("/pool/root-a"), "root-a", "/pool/root-a").is_ok());
    assert!(ensure_identity("root-a", Some("/pool/root-a"), "root-a", "/foreign/root-a").is_err());
    assert!(ensure_identity("root-a", Some("/pool/root-a"), "root-b", "/pool/root-a").is_err());
}

/// A helper killed between creating a volume and journalling its key leaves the
/// manifest holding the name alone. The name carries this allocation's own
/// artifact id, so it still authorizes the delete; anything else does not.
#[test]
fn keyless_artifact_authorizes_deletion_of_its_own_name_only() {
    assert!(ensure_identity("root-a", None, "root-a", "/pool/root-a").is_ok());
    assert!(ensure_identity("root-a", None, "root-b", "/pool/root-b").is_err());
}
