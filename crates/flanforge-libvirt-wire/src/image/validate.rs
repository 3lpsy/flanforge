use time::{OffsetDateTime, format_description::well_known::Rfc3339};
use url::Url;

use super::{
    guest::MAX_GUEST_CONTRACT_VERSION,
    limits::{MAX_IMAGE_FILE_BYTES, MAX_IMAGE_FORMAT_BYTES, MAX_VIRTUAL_IMAGE_BYTES},
    model::BaseImageManifest,
};
use crate::{WireError, validation::is_safe_account_name};

const CONTRACT: &str = "base image manifest";

/// System accounts are never the job account: the base bakes an unprivileged
/// one, and a manifest claiming otherwise is refused rather than trusted.
const MIN_JOB_ACCOUNT_UID: u32 = 1_000;

const MAX_BLOCK_RPCS_BYTES: usize = 512;

pub(super) fn ensure_valid(manifest: &BaseImageManifest) -> Result<(), WireError> {
    if manifest.schema_version != 1 {
        return invalid("schema_version");
    }
    // Every section but `image` is optional provenance. Absent means "not
    // recorded"; present means "must be well formed", so a full manifest is
    // checked exactly as strictly as before.
    if manifest.created_at.as_deref().is_some_and(|value| {
        !is_bounded_graphic(value, 20, 64) || OffsetDateTime::parse(value, &Rfc3339).is_err()
    }) {
        return invalid("created_at");
    }
    ensure_source_valid(manifest)?;
    ensure_builder_valid(manifest)?;
    ensure_image_valid(manifest)?;
    ensure_provenance_valid(manifest)?;
    ensure_guest_valid(manifest)?;
    if let (Some(provenance), Some(guest)) = (&manifest.provenance, &manifest.guest)
        && provenance.dependency_proxy.configured != guest.dependency_proxy_configured
    {
        return invalid("dependency_proxy_configured");
    }
    Ok(())
}

fn ensure_source_valid(manifest: &BaseImageManifest) -> Result<(), WireError> {
    let Some(source) = &manifest.source else {
        return Ok(());
    };
    if !is_https_url(&source.os_image_url) {
        return invalid("source.os_image_url");
    }
    if !is_sha256(&source.os_image_sha256) {
        return invalid("source.os_image_sha256");
    }
    if !is_bounded_graphic(&source.repository_revision, 1, 128) {
        return invalid("source.repository_revision");
    }
    let _ = source.repository_dirty;
    Ok(())
}

fn ensure_builder_valid(manifest: &BaseImageManifest) -> Result<(), WireError> {
    let Some(builder) = &manifest.builder else {
        return Ok(());
    };
    if !is_bounded_graphic(&builder.packer_version, 1, 64) {
        return invalid("builder.packer_version");
    }
    if !is_bounded_graphic(&builder.qemu_plugin_version, 1, 64) {
        return invalid("builder.qemu_plugin_version");
    }
    Ok(())
}

fn ensure_image_valid(manifest: &BaseImageManifest) -> Result<(), WireError> {
    let image = &manifest.image;
    if !is_safe_image_file(&image.file) {
        return invalid("image.file");
    }
    if !is_bounded_graphic(&image.format, 1, MAX_IMAGE_FORMAT_BYTES) {
        return invalid("image.format");
    }
    if !is_sha256(&image.sha256) {
        return invalid("image.sha256");
    }
    if image.bytes == 0
        || image.bytes > image.virtual_bytes
        || image.virtual_bytes > MAX_VIRTUAL_IMAGE_BYTES
    {
        return invalid("image.bytes");
    }
    Ok(())
}

fn ensure_provenance_valid(manifest: &BaseImageManifest) -> Result<(), WireError> {
    let Some(provenance) = &manifest.provenance else {
        return Ok(());
    };
    let release = &provenance.runner_release;
    if !is_https_url(&release.api_url) {
        return invalid("provenance.runner_release.api_url");
    }
    if !is_https_url(&release.download_base_url) {
        return invalid("provenance.runner_release.download_base_url");
    }
    if release.signing_primary_fingerprint.len() != 40
        || !release
            .signing_primary_fingerprint
            .bytes()
            .all(|byte| byte.is_ascii_hexdigit() && !byte.is_ascii_lowercase())
    {
        return invalid("provenance.runner_release.signing_primary_fingerprint");
    }
    if provenance
        .dependency_proxy
        .ca_sha256
        .as_deref()
        .is_some_and(|value| !is_sha256(value))
    {
        return invalid("provenance.dependency_proxy.ca_sha256");
    }
    Ok(())
}

fn ensure_guest_valid(manifest: &BaseImageManifest) -> Result<(), WireError> {
    let Some(guest) = &manifest.guest else {
        return Ok(());
    };
    if guest.schema_version != 1 {
        return invalid("guest.schema_version");
    }
    // Reported separately from the schema: an operator whose base is simply
    // older needs to be told which number to rebuild against.
    if guest.contract_version == 0 || guest.contract_version > MAX_GUEST_CONTRACT_VERSION {
        return invalid("guest.guest_contract_version");
    }
    if !is_bounded_graphic(&guest.os.id, 1, 64) {
        return invalid("guest.os.id");
    }
    if !is_bounded_graphic(&guest.os.version_id, 1, 64) {
        return invalid("guest.os.version_id");
    }
    if !is_bounded_graphic(&guest.os.architecture, 1, 64) {
        return invalid("guest.os.architecture");
    }
    if !is_bounded_graphic(&guest.forgejo_runner.version, 1, 128)
        || !is_sha256(&guest.forgejo_runner.sha256)
    {
        return invalid("guest.forgejo_runner");
    }
    if !is_bounded_graphic(&guest.podman.version, 1, 128) {
        return invalid("guest.podman");
    }
    if guest
        .agent
        .as_ref()
        .is_some_and(|agent| !is_block_list(&agent.block_rpcs))
    {
        return invalid("guest.guest_agent.block_rpcs");
    }
    if guest.job_account.as_ref().is_some_and(|account| {
        !is_safe_account_name(&account.name) || account.uid < MIN_JOB_ACCOUNT_UID
    }) {
        return invalid("guest.job_account");
    }
    if guest.privileged_account.as_ref().is_some_and(|account| {
        !is_safe_account_name(&account.name) || account.uid < MIN_JOB_ACCOUNT_UID
    }) {
        return invalid("guest.privileged_account");
    }
    Ok(())
}

/// The agent's own block-list syntax: a comma-joined list of RPC names, or
/// empty when nothing is blocked.
fn is_block_list(value: &str) -> bool {
    value.len() <= MAX_BLOCK_RPCS_BYTES
        && value.bytes().all(|byte| {
            byte.is_ascii_lowercase() || byte.is_ascii_digit() || matches!(byte, b',' | b'-')
        })
}

fn is_https_url(value: &str) -> bool {
    value.len() <= 2_048
        && Url::parse(value).is_ok_and(|url| {
            url.scheme() == "https"
                && url.has_host()
                && url.username().is_empty()
                && url.password().is_none()
                && url.fragment().is_none()
        })
}

fn is_sha256(value: &str) -> bool {
    value.len() == 64
        && value
            .bytes()
            .all(|byte| byte.is_ascii_digit() || (b'a'..=b'f').contains(&byte))
}

fn is_safe_image_file(value: &str) -> bool {
    (1..=MAX_IMAGE_FILE_BYTES).contains(&value.len())
        && value != "."
        && value != ".."
        && value
            .bytes()
            .all(|byte| byte.is_ascii_alphanumeric() || matches!(byte, b'-' | b'_' | b'.'))
}

fn is_bounded_graphic(value: &str, min: usize, max: usize) -> bool {
    (min..=max).contains(&value.len()) && value.bytes().all(|byte| byte.is_ascii_graphic())
}

fn invalid<T>(field: &'static str) -> Result<T, WireError> {
    Err(WireError::invalid(CONTRACT, field))
}
