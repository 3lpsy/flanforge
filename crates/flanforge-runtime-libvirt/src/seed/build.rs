use std::io::{Cursor, Write};

use fatfs::{FileSystem, FormatVolumeOptions, FsOptions, format_volume};
use rand_core::OsRng;
use serde::Serialize;
use ssh_key::{Algorithm, LineEnding, PrivateKey};

use crate::RuntimeError;

const SEED_BYTES: usize = 2 * 1_024 * 1_024;

#[derive(Clone, Debug, Eq, PartialEq)]
pub(crate) struct SeedBundle {
    pub(crate) image: Vec<u8>,
    pub(crate) host_key_alias: String,
    pub(crate) known_hosts: String,
}

#[derive(Serialize)]
struct CloudConfig<'a> {
    ssh_deletekeys: bool,
    ssh_genkeytypes: Vec<&'a str>,
    #[serde(skip_serializing_if = "Option::is_none")]
    ssh_keys: Option<SshKeys<'a>>,
    #[serde(skip_serializing_if = "Vec::is_empty")]
    write_files: Vec<WriteFile<'a>>,
    #[serde(skip_serializing_if = "Vec::is_empty")]
    bootcmd: Vec<Vec<&'a str>>,
}

/// Runs in `cloud-init-local`, before `sshd.service` can start. `--runtime`
/// masks live in `/run`, a tmpfs, so the channel choice is never captured into
/// a warm generation the way `systemctl disable` would be.
const DISABLE_SSHD: &str = "systemctl stop sshd.service sshd.socket >/dev/null 2>&1 || true; \
                            systemctl --runtime mask sshd.service sshd.socket";

#[derive(Serialize)]
struct SshKeys<'a> {
    ed25519_private: &'a str,
    ed25519_public: &'a str,
}

#[derive(Serialize)]
struct WriteFile<'a> {
    path: String,
    owner: String,
    permissions: &'a str,
    content: &'a str,
}

#[derive(Serialize)]
struct Metadata<'a> {
    #[serde(rename = "instance-id")]
    instance_id: &'a str,
    #[serde(rename = "local-hostname")]
    local_hostname: &'a str,
}

pub(crate) fn operator_public_key(bytes: &[u8]) -> Result<String, RuntimeError> {
    if bytes.is_empty() || bytes.len() > 64 * 1_024 {
        return Err(RuntimeError::seed("guest SSH identity has an invalid size"));
    }
    let encoded = std::str::from_utf8(bytes).map_err(RuntimeError::seed)?;
    let private = PrivateKey::from_openssh(encoded).map_err(RuntimeError::seed)?;
    if private.is_encrypted() {
        return Err(RuntimeError::seed(
            "guest SSH identity cannot require a passphrase",
        ));
    }
    private
        .public_key()
        .to_openssh()
        .map_err(RuntimeError::seed)
}

/// Builds the per-allocation cloud-init seed.
///
/// `guest_user` is the account the base image already provides and that
/// `guest.runner_user` names; the seed only drops an authorized key into its
/// home, so the operator decides what that account is called.
///
/// `authorized_key` absent means no `[guest.ssh]` was configured: the seed then
/// carries no key material at all and masks sshd for the guest's lifetime, so
/// there is no login the daemon never intends to use.
///
/// When SSH is configured, `privileged_authorized_key` is dropped into the
/// privileged account's home as well, so the same transport reaches both. On a
/// base that omitted the account the write fails as a recoverable cloud-init
/// warning, which readiness tolerates.
pub(crate) fn build_seed(
    allocation_id: &str,
    hostname: &str,
    authorized_key: Option<&str>,
    guest_user: &str,
    privileged_user: &str,
    privileged_authorized_key: Option<&str>,
) -> Result<SeedBundle, RuntimeError> {
    let host_key = authorized_key.map(|_| generate_host_key()).transpose()?;
    let cloud = CloudConfig {
        ssh_deletekeys: true,
        // Defence in depth: even an ineffective mask leaves sshd keyless.
        ssh_genkeytypes: Vec::new(),
        ssh_keys: host_key.as_ref().map(|(private, public)| SshKeys {
            ed25519_private: private.as_str(),
            ed25519_public: public.as_str(),
        }),
        write_files: authorized_key
            .map(|content| WriteFile {
                path: format!("/home/{guest_user}/.ssh/authorized_keys"),
                owner: format!("{guest_user}:{guest_user}"),
                permissions: "0600",
                content,
            })
            .into_iter()
            .chain(
                authorized_key
                    .and(privileged_authorized_key)
                    .map(|content| WriteFile {
                        path: format!("/home/{privileged_user}/.ssh/authorized_keys"),
                        owner: format!("{privileged_user}:{privileged_user}"),
                        permissions: "0600",
                        content,
                    }),
            )
            .collect(),
        bootcmd: if authorized_key.is_some() {
            Vec::new()
        } else {
            vec![vec![
                "cloud-init-per",
                "once",
                "flanforge-no-sshd",
                "sh",
                "-c",
                DISABLE_SSHD,
            ]]
        },
    };
    let user_data = format!(
        "#cloud-config\n{}",
        serde_yaml::to_string(&cloud).map_err(RuntimeError::seed)?
    );
    // A VM name is not a hostname: profile names may carry underscores, which
    // hostnames cannot, and cloud-init warns on every boot it retries them.
    let local_hostname = hostname.replace('_', "-");
    let meta_data = serde_json::to_vec(&Metadata {
        instance_id: allocation_id,
        local_hostname: &local_hostname,
    })
    .map_err(RuntimeError::seed)?;
    let image = fat_image(&meta_data, user_data.as_bytes())?;
    // The alias is written whatever the channel, so the ownership manifest and
    // everything that reaps against it keep one shape.
    let host_key_alias = format!("flanforge-{allocation_id}");
    let known_hosts = host_key
        .map(|(_, public)| format!("{host_key_alias} {public}\n"))
        .unwrap_or_default();
    Ok(SeedBundle {
        image,
        host_key_alias,
        known_hosts,
    })
}

fn generate_host_key() -> Result<(String, String), RuntimeError> {
    let private = PrivateKey::random(&mut OsRng, Algorithm::Ed25519).map_err(RuntimeError::seed)?;
    let encoded = private
        .to_openssh(LineEnding::LF)
        .map_err(RuntimeError::seed)?;
    let public = private
        .public_key()
        .to_openssh()
        .map_err(RuntimeError::seed)?;
    Ok((encoded.to_string(), public))
}

fn fat_image(meta_data: &[u8], user_data: &[u8]) -> Result<Vec<u8>, RuntimeError> {
    let mut image = vec![0_u8; SEED_BYTES];
    format_volume(
        Cursor::new(image.as_mut_slice()),
        FormatVolumeOptions::new().volume_label(*b"CIDATA     "),
    )
    .map_err(RuntimeError::seed)?;
    let file_system = FileSystem::new(Cursor::new(image.as_mut_slice()), FsOptions::new())
        .map_err(RuntimeError::seed)?;
    let root = file_system.root_dir();
    write_file(&root, "meta-data", meta_data)?;
    write_file(&root, "user-data", user_data)?;
    drop(root);
    file_system.unmount().map_err(RuntimeError::seed)?;
    Ok(image)
}

fn write_file<T: fatfs::ReadWriteSeek>(
    root: &fatfs::Dir<'_, T>,
    name: &str,
    contents: &[u8],
) -> Result<(), RuntimeError> {
    let mut file = root.create_file(name).map_err(RuntimeError::seed)?;
    file.write_all(contents).map_err(RuntimeError::seed)
}
