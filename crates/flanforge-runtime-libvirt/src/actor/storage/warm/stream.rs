use virt::{connect::Connect, storage_vol::StorageVol, stream::Stream};

use crate::RuntimeError;

/// Reads a bounded prefix of a volume through libvirt, so the daemon never
/// opens a pool path. The hypervisor may be on another host, and the service
/// unit's mount namespace makes the pool directory read-only even when it is
/// not — every existing write to a pool volume goes through a stream for the
/// same reason.
pub(super) fn download_prefix(
    connection: &Connect,
    volume: &StorageVol,
    length: u64,
) -> Result<Vec<u8>, RuntimeError> {
    if length == 0 {
        return Ok(Vec::new());
    }
    let stream = Stream::new(connection, 0)
        .map_err(|error| RuntimeError::libvirt("volume prefix stream", error))?;
    volume
        .download(&stream, 0, length, 0)
        .map_err(|error| RuntimeError::libvirt("volume prefix download", error))?;
    let mut bytes = Vec::with_capacity(usize::try_from(length).unwrap_or(0));
    let mut buffer = vec![0_u8; 64 * 1_024];
    while (bytes.len() as u64) < length {
        let requested = usize::try_from(length - bytes.len() as u64)
            .map_err(RuntimeError::manifest)?
            .min(buffer.len());
        match stream.recv(&mut buffer[..requested]) {
            Ok(0) => break,
            Ok(read) => bytes.extend_from_slice(&buffer[..read]),
            Err(error) => {
                let _ = stream.abort();
                return Err(RuntimeError::libvirt("volume prefix read", error));
            }
        }
    }
    // A short read is not an error: a volume smaller than the window simply
    // ends, and the header parser decides whether that is enough. The stream
    // is always aborted rather than finished, because only a prefix was ever
    // requested.
    let _ = stream.abort();
    Ok(bytes)
}
