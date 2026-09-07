use flanforge_libvirt_wire::{CheckpointVolumeRole, VolumeCheckpoint, VolumePointer};
use roxmltree::Document;
use virt::{connect::Connect, storage_pool::StoragePool, storage_vol::StorageVol};

use crate::{
    RuntimeError,
    actor::message::{ActorConfig, WarmVerifyRequest},
    checkpoint::{load as load_checkpoint, warm_path},
    file::{HEADER_WINDOW_BYTES, parse_header},
};

use super::super::handles::{free_pool, free_volume};
use super::stream::download_prefix;

const MAX_XML_BYTES: usize = 1_024 * 1_024;

/// Re-reads the captured volume from libvirt and from its own bytes.
///
/// The flatten is asserted twice, because a silently preserved chain would
/// make the warm generation depend on the cold base it was meant to replace.
/// The qcow2 header is authoritative — it is the file — and the volume XML is
/// libvirt's cached view, kept as the cross-check.
pub(in crate::actor) fn verify(
    connection: &Connect,
    config: &ActorConfig,
    request: &WarmVerifyRequest,
) -> Result<VolumePointer, RuntimeError> {
    let checkpoint = load_checkpoint(&warm_path(&config.state_dir, request.capture_id))?
        .ok_or_else(|| RuntimeError::ownership("warm capture has no durable checkpoint"))?;
    checkpoint
        .ensure_warm(
            config.service_instance,
            &config.pool,
            &request.profile,
            request.capture_id,
        )
        .map_err(RuntimeError::ownership)?;
    let recorded = checkpoint
        .volume(CheckpointVolumeRole::Warm)
        .ok_or_else(|| RuntimeError::ownership("warm checkpoint records no volume key"))?;
    let volume_name = VolumeCheckpoint::warm_volume_name(request.capture_id);
    if recorded.name() != volume_name {
        return Err(RuntimeError::ownership(
            "warm checkpoint names another volume",
        ));
    }
    let mut pool = StoragePool::lookup_by_name(connection, &config.pool)
        .map_err(|error| RuntimeError::libvirt("warm verification pool lookup", error))?;
    // libvirt's cached allocation and capacity are stale until the pool is
    // probed again, and the assertion below is about exactly those.
    let refreshed = pool
        .refresh(0)
        .map_err(|error| RuntimeError::libvirt("warm verification pool refresh", error));
    let result = refreshed.and_then(|_| {
        ensure_standalone(
            connection,
            &pool,
            &volume_name,
            recorded.key(),
            request.virtual_bytes,
        )
    });
    free_pool(&mut pool)?;
    result?;
    VolumePointer::new(
        config.pool.clone(),
        volume_name,
        recorded.key().to_owned(),
        request.virtual_bytes,
        request.generation,
        request.produced_at_unix,
    )
    .map_err(RuntimeError::manifest)
}

fn ensure_standalone(
    connection: &Connect,
    pool: &StoragePool,
    volume_name: &str,
    expected_key: &str,
    virtual_bytes: u64,
) -> Result<(), RuntimeError> {
    let mut volume = StorageVol::lookup_by_name(pool, volume_name)
        .map_err(|error| RuntimeError::libvirt("warm verification lookup", error))?;
    let result = (|| {
        let key = volume
            .get_key()
            .map_err(|error| RuntimeError::libvirt("warm verification key", error))?;
        let info = volume
            .get_info()
            .map_err(|error| RuntimeError::libvirt("warm verification capacity", error))?;
        if key != expected_key || info.capacity != virtual_bytes {
            return Err(RuntimeError::ownership(
                "captured warm volume disagrees with its checkpoint",
            ));
        }
        let xml = volume
            .get_xml_desc(0)
            .map_err(|error| RuntimeError::libvirt("warm verification XML", error))?;
        if xml.len() > MAX_XML_BYTES {
            return Err(RuntimeError::ownership("warm volume XML is oversized"));
        }
        if Document::parse(&xml)
            .map_err(RuntimeError::ownership)?
            .descendants()
            .any(|node| node.has_tag_name("backingStore"))
        {
            return Err(RuntimeError::manifest(
                "captured warm volume still declares a backing store",
            ));
        }
        let prefix = download_prefix(
            connection,
            &volume,
            info.allocation.min(HEADER_WINDOW_BYTES),
        )?;
        let header = parse_header(&prefix)?
            .ok_or_else(|| RuntimeError::manifest("captured warm volume is not a qcow2 image"))?;
        if header.backing_file.is_some() || header.virtual_bytes != virtual_bytes {
            return Err(RuntimeError::manifest(
                "captured warm image virtual size or backing-file state is wrong",
            ));
        }
        Ok(())
    })();
    free_volume(&mut volume)?;
    result
}
