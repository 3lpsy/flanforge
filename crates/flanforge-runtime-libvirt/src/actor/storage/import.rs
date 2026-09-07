use std::{fs::OpenOptions, io::Read, os::unix::fs::OpenOptionsExt};

use flanforge_libvirt_wire::{CheckpointVolumeRole, PublishedBase};
use sha2::{Digest, Sha256};
use virt::{connect::Connect, storage_vol::StorageVol, stream::Stream};

use crate::{
    RuntimeError,
    actor::message::{ActorConfig, ImportRequest},
    checkpoint::VolumeJournal,
};

use super::{
    handles::{free_pool, free_volume},
    provision::{active_pool, create_volume},
};

pub(in crate::actor) fn import(
    connection: &Connect,
    config: &ActorConfig,
    request: ImportRequest,
) -> Result<PublishedBase, RuntimeError> {
    let mut source = OpenOptions::new()
        .read(true)
        .custom_flags(libc::O_NOFOLLOW | libc::O_CLOEXEC)
        .open(&request.staged_image_path)
        .map_err(RuntimeError::manifest)?;
    let metadata = source.metadata().map_err(RuntimeError::manifest)?;
    if !metadata.file_type().is_file() || metadata.len() != request.manifest.image_bytes() {
        return Err(RuntimeError::manifest(
            "staged image is not the bounded regular file in its manifest",
        ));
    }
    let mut pool = active_pool(connection, config, request.manifest.image_bytes())?;
    let volume_name = request.volume_name;
    let xml = format!(
        "<volume><name>{volume_name}</name><capacity unit=\"B\">{}</capacity><target><format type=\"qcow2\"/></target></volume>",
        request.manifest.virtual_bytes()
    );
    let mut journal = VolumeJournal::image_import(config, &volume_name)?;
    let mut volume = match create_volume(&pool, &xml, "base staging-volume creation") {
        Ok(volume) => volume,
        Err(error @ RuntimeError::Collision { .. }) => {
            journal.finish()?;
            return Err(error);
        }
        Err(error) => return Err(error),
    };
    let result = (|| {
        let key = volume
            .get_key()
            .map_err(|error| RuntimeError::libvirt("base volume key", error))?;
        journal.record(CheckpointVolumeRole::Base, volume_name.clone(), key.clone())?;
        upload(
            connection,
            &volume,
            &mut source,
            request.manifest.image_bytes(),
            Some(request.manifest.image_sha256()),
        )?;
        ensure_download_digest(
            connection,
            &volume,
            request.manifest.image_bytes(),
            request.manifest.image_sha256(),
        )?;
        let info = volume
            .get_info()
            .map_err(|error| RuntimeError::libvirt("base volume verification", error))?;
        if info.capacity != request.manifest.virtual_bytes() {
            return Err(RuntimeError::manifest(
                "uploaded base volume has the wrong virtual size",
            ));
        }
        PublishedBase::new(
            request.logical_name,
            config.pool.clone(),
            volume_name.clone(),
            key,
            request.manifest,
        )
        .map_err(RuntimeError::manifest)
    })();
    if result.is_err() {
        let _ = volume.delete(0);
    }
    free_volume(&mut volume)?;
    free_pool(&mut pool)?;
    result
}

pub(super) fn upload(
    connection: &Connect,
    volume: &StorageVol,
    reader: &mut impl Read,
    length: u64,
    expected_sha256: Option<&str>,
) -> Result<(), RuntimeError> {
    let stream = Stream::new(connection, 0)
        .map_err(|error| RuntimeError::libvirt("upload stream", error))?;
    volume
        .upload(&stream, 0, length, 0)
        .map_err(|error| RuntimeError::libvirt("volume upload", error))?;
    let result = send_exact(&stream, reader, length, expected_sha256);
    match result {
        Ok(()) => stream
            .finish()
            .map_err(|error| RuntimeError::libvirt("volume upload finish", error)),
        Err(error) => {
            let _ = stream.abort();
            Err(error)
        }
    }
}

fn send_exact(
    stream: &Stream,
    reader: &mut impl Read,
    length: u64,
    expected_sha256: Option<&str>,
) -> Result<(), RuntimeError> {
    let mut buffer = vec![0_u8; 1024 * 1024];
    let mut remaining = length;
    let mut digest = Sha256::new();
    while remaining > 0 {
        let requested =
            usize::try_from(remaining.min(buffer.len() as u64)).map_err(RuntimeError::manifest)?;
        let read = reader
            .read(&mut buffer[..requested])
            .map_err(RuntimeError::manifest)?;
        if read == 0 {
            return Err(RuntimeError::manifest("uploaded source ended early"));
        }
        digest.update(&buffer[..read]);
        remaining -= read as u64;
        let mut sent = 0;
        while sent < read {
            let written = stream
                .send(&buffer[sent..read])
                .map_err(|error| RuntimeError::libvirt("volume upload send", error))?;
            if written == 0 {
                return Err(RuntimeError::libvirt(
                    "volume upload send",
                    "libvirt accepted zero bytes",
                ));
            }
            sent += written;
        }
    }
    if reader
        .read(&mut buffer[..1])
        .map_err(RuntimeError::manifest)?
        != 0
    {
        return Err(RuntimeError::manifest(
            "uploaded source exceeds its declared length",
        ));
    }
    if expected_sha256.is_some_and(|expected| format!("{:x}", digest.finalize()) != expected) {
        return Err(RuntimeError::manifest(
            "uploaded source digest disagrees with its manifest",
        ));
    }
    Ok(())
}

fn ensure_download_digest(
    connection: &Connect,
    volume: &StorageVol,
    length: u64,
    expected_sha256: &str,
) -> Result<(), RuntimeError> {
    let stream = Stream::new(connection, 0)
        .map_err(|error| RuntimeError::libvirt("verification stream", error))?;
    volume
        .download(&stream, 0, length, 0)
        .map_err(|error| RuntimeError::libvirt("base volume download", error))?;
    let mut remaining = length;
    let mut digest = Sha256::new();
    let mut buffer = vec![0_u8; 1024 * 1024];
    while remaining > 0 {
        let requested =
            usize::try_from(remaining.min(buffer.len() as u64)).map_err(RuntimeError::manifest)?;
        let read = stream
            .recv(&mut buffer[..requested])
            .map_err(|error| RuntimeError::libvirt("base volume verification", error))?;
        if read == 0 {
            let _ = stream.abort();
            return Err(RuntimeError::manifest(
                "uploaded base volume ended before its declared length",
            ));
        }
        digest.update(&buffer[..read]);
        remaining -= read as u64;
    }
    stream
        .finish()
        .map_err(|error| RuntimeError::libvirt("base volume verification finish", error))?;
    if format!("{:x}", digest.finalize()) != expected_sha256 {
        return Err(RuntimeError::manifest(
            "uploaded base volume digest disagrees with its manifest",
        ));
    }
    Ok(())
}
