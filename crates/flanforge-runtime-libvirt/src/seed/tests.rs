use std::io::{Cursor, Read};

use fatfs::{FileSystem, FsOptions};
use ssh_key::{Algorithm, LineEnding, PrivateKey};

use super::{build_seed, operator_public_key};

#[test]
fn seed_has_unique_pinning_material_and_no_forgejo_secret() {
    let seed = build_seed(
        "019d0000-0000-7000-8000-000000000001",
        "ci-flanforge_linux-1-1",
        Some("ssh-ed25519 AAAAC3NzaC1lZDI1NTE5AAAAIFixture"),
        "runner",
        "prunner",
        Some("ssh-ed25519 AAAAC3NzaC1lZDI1NTE5AAAAIFixture"),
    )
    .unwrap_or_else(|error| unreachable!("seed: {error}"));
    assert_eq!(seed.image.len(), 2 * 1_024 * 1_024);
    assert!(seed.known_hosts.starts_with(&seed.host_key_alias));
    assert!(!seed.image.windows(7).any(|bytes| bytes == b"forgejo"));

    let fs = FileSystem::new(Cursor::new(seed.image), FsOptions::new())
        .unwrap_or_else(|error| unreachable!("fat: {error}"));
    assert_eq!(fs.volume_label(), "CIDATA");
    let mut user_data = String::new();
    fs.root_dir()
        .open_file("user-data")
        .and_then(|mut file| file.read_to_string(&mut user_data))
        .unwrap_or_else(|error| unreachable!("user-data: {error}"));
    assert!(user_data.contains("ssh_deletekeys: true"));
    assert!(user_data.contains("/home/runner/.ssh/authorized_keys"));
    assert!(user_data.contains("/home/prunner/.ssh/authorized_keys"));
    // A VM name is not a hostname: its underscores become hyphens.
    let mut meta_data = String::new();
    fs.root_dir()
        .open_file("meta-data")
        .and_then(|mut file| file.read_to_string(&mut meta_data))
        .unwrap_or_else(|error| unreachable!("meta-data: {error}"));
    assert!(meta_data.contains("\"local-hostname\":\"ci-flanforge-linux-1-1\""));
    assert!(!meta_data.contains('_'));
}

#[test]
fn operator_key_is_derived_without_retaining_private_material() {
    let key = PrivateKey::random(&mut rand_core::OsRng, Algorithm::Ed25519)
        .unwrap_or_else(|error| unreachable!("key: {error}"));
    let encoded = key
        .to_openssh(LineEnding::LF)
        .unwrap_or_else(|error| unreachable!("encode: {error}"));
    let public = operator_public_key(encoded.as_bytes())
        .unwrap_or_else(|error| unreachable!("public: {error}"));
    assert!(public.starts_with("ssh-ed25519 "));
    assert!(!public.contains("PRIVATE"));

    let encrypted = key
        .encrypt(&mut rand_core::OsRng, "passphrase")
        .and_then(|key| key.to_openssh(LineEnding::LF))
        .unwrap_or_else(|error| unreachable!("encrypt: {error}"));
    assert!(operator_public_key(encrypted.as_bytes()).is_err());
}

/// With no `[guest.ssh]` there is no login the daemon intends to use, so the
/// seed carries no key material at all and masks sshd for the guest's life.
#[test]
fn a_seed_without_an_operator_key_carries_no_key_material_and_masks_sshd() {
    let seed = build_seed(
        "019d0000-0000-7000-8000-000000000002",
        "ci-019d0001",
        None,
        "runner",
        "prunner",
        None,
    )
    .unwrap_or_else(|error| unreachable!("seed: {error}"));
    assert_eq!(
        seed.host_key_alias,
        "flanforge-019d0000-0000-7000-8000-000000000002"
    );
    assert!(seed.known_hosts.is_empty());

    let fs = FileSystem::new(Cursor::new(seed.image), FsOptions::new())
        .unwrap_or_else(|error| unreachable!("fat: {error}"));
    let mut user_data = String::new();
    fs.root_dir()
        .open_file("user-data")
        .and_then(|mut file| file.read_to_string(&mut user_data))
        .unwrap_or_else(|error| unreachable!("user-data: {error}"));
    for absent in ["ssh_keys", "authorized_keys", "PRIVATE"] {
        assert!(!user_data.contains(absent), "{absent} reached the seed");
    }
    assert!(user_data.contains("ssh_deletekeys: true"));
    assert!(user_data.contains("ssh_genkeytypes: []"));
    // `--runtime` lives in /run, so the channel choice is never captured into
    // a warm generation the way `systemctl disable` would be.
    assert!(user_data.contains("systemctl --runtime mask sshd.service sshd.socket"));
    assert!(user_data.contains("cloud-init-per"));
}
