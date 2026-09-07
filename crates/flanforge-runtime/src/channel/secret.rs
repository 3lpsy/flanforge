use flanforge_manager::WorkerError;

const MAX_SECRET_BYTES: usize = 1_024;

/// A guest-bound secret. What this type guarantees is that the value never
/// reaches argv and never appears in `Debug`, which prints only a length. The
/// buffer is best-effort cleared on drop; that does not reach copies the caller
/// already made, and is not a substitute for them.
pub struct GuestSecret(Vec<u8>);

impl GuestSecret {
    /// Stores `value` with the terminating newline the guest's `read` needs:
    /// `IFS= read -r` returns non-zero without it and takes the failure branch.
    ///
    /// # Errors
    /// Returns an error for an empty, oversized, or non-printable secret.
    pub fn line(value: &str) -> Result<Self, WorkerError> {
        if value.is_empty()
            || value.len() > MAX_SECRET_BYTES
            || value.bytes().any(|byte| !byte.is_ascii_graphic())
        {
            return Err(WorkerError::new("guest secret is structurally invalid"));
        }
        let mut bytes = Vec::with_capacity(value.len() + 1);
        bytes.extend_from_slice(value.as_bytes());
        bytes.push(b'\n');
        Ok(Self(bytes))
    }

    #[must_use]
    pub fn as_bytes(&self) -> &[u8] {
        &self.0
    }
}

impl std::fmt::Debug for GuestSecret {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter
            .debug_struct("GuestSecret")
            .field("bytes", &self.0.len())
            .finish()
    }
}

impl Drop for GuestSecret {
    fn drop(&mut self) {
        self.0.fill(0);
    }
}
