use std::io::{Read, Write};

use flanforge_libvirt_wire::{
    HelperFailureCode, HelperReply, HelperRequest, MAX_LIBVIRT_HELPER_REQUEST_BYTES,
};

use crate::RuntimeError;

use super::host;

pub(crate) fn run() -> Result<(), RuntimeError> {
    let mut request = Vec::new();
    std::io::stdin()
        .take((MAX_LIBVIRT_HELPER_REQUEST_BYTES + 1) as u64)
        .read_to_end(&mut request)
        .map_err(RuntimeError::helper)?;
    let reply = match HelperRequest::parse(&request) {
        Ok(request) => host::execute(request),
        Err(error) => HelperReply::error(HelperFailureCode::InvalidRequest, error),
    };
    let encoded = reply.encode().map_err(RuntimeError::helper)?;
    let mut stdout = std::io::stdout().lock();
    stdout.write_all(&encoded).map_err(RuntimeError::helper)?;
    stdout.flush().map_err(RuntimeError::helper)
}
