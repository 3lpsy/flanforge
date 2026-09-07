use std::{process::Stdio, time::Duration};

use flanforge_libvirt_wire::{
    HelperReply, HelperRequest, LIBVIRT_HELPER_ARGUMENT, MAX_LIBVIRT_HELPER_REPLY_BYTES,
};
use tokio::{
    io::{AsyncReadExt, AsyncWriteExt},
    process::{Child, Command},
};

use crate::RuntimeError;

use super::model::LibvirtActor;

impl LibvirtActor {
    pub(super) async fn request(
        &self,
        request: HelperRequest,
        timeout: Duration,
    ) -> Result<HelperReply, RuntimeError> {
        self.request_with(timeout, |_| Ok(request)).await
    }

    /// Runs a request that changes host state. These are serialized against
    /// each other so two allocations cannot race on the same pool or domain.
    pub(super) async fn request_with(
        &self,
        timeout: Duration,
        build: impl FnOnce(Duration) -> Result<HelperRequest, RuntimeError>,
    ) -> Result<HelperReply, RuntimeError> {
        let deadline = tokio::time::Instant::now() + timeout;
        let _guard = tokio::time::timeout(timeout, self.serial.lock())
            .await
            .map_err(|_| RuntimeError::Deadline)?;
        self.dispatch(deadline, build).await
    }

    /// Runs a request that only reads host state, without waiting behind a
    /// mutation. Each request is its own helper process, so there is no shared
    /// handle to protect; the lock exists to order mutations. Serializing
    /// reads too meant one guest ignoring ACPI shutdown blocked admission and
    /// `daemon status` for the whole cleanup deadline.
    pub(super) async fn request_read_only(
        &self,
        request: HelperRequest,
        timeout: Duration,
    ) -> Result<HelperReply, RuntimeError> {
        let deadline = tokio::time::Instant::now() + timeout;
        self.dispatch(deadline, |_| Ok(request)).await
    }

    /// The read-only lane for a request whose inner timeout has to be derived
    /// from what is left of the outer budget.
    pub(super) async fn request_read_only_with(
        &self,
        timeout: Duration,
        build: impl FnOnce(Duration) -> Result<HelperRequest, RuntimeError>,
    ) -> Result<HelperReply, RuntimeError> {
        let deadline = tokio::time::Instant::now() + timeout;
        self.dispatch(deadline, build).await
    }

    /// Runs a mutation that deliberately does not take the ordering lock.
    ///
    /// The only caller is the warm capture, which can run for the better part
    /// of an hour: holding the lock for that long would block admission,
    /// cleanup, and `daemon status` behind it, the exact failure
    /// `request_read_only` exists to avoid. It is safe because the capture
    /// creates one journaled, unpublished volume under a fresh UUID that
    /// nothing references and creates no reference of its own, so it collides
    /// with no concurrent `create_guest_resources`, `cleanup`, or `delete_vm`.
    /// Every other mutation, including retirement, still serializes.
    pub(super) async fn request_unlocked_mutation(
        &self,
        timeout: Duration,
        build: impl FnOnce(Duration) -> Result<HelperRequest, RuntimeError>,
    ) -> Result<HelperReply, RuntimeError> {
        let deadline = tokio::time::Instant::now() + timeout;
        self.dispatch(deadline, build).await
    }

    async fn dispatch(
        &self,
        deadline: tokio::time::Instant,
        build: impl FnOnce(Duration) -> Result<HelperRequest, RuntimeError>,
    ) -> Result<HelperReply, RuntimeError> {
        let remaining = deadline
            .checked_duration_since(tokio::time::Instant::now())
            .filter(|duration| !duration.is_zero())
            .ok_or(RuntimeError::Deadline)?;
        let request = build(remaining)?;
        let encoded = request.encode().map_err(RuntimeError::helper)?;
        let mut child = Command::new(&self.executable)
            .arg(LIBVIRT_HELPER_ARGUMENT)
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::null())
            .kill_on_drop(true)
            .spawn()
            .map_err(RuntimeError::helper)?;
        let stdin = child
            .stdin
            .take()
            .ok_or_else(|| RuntimeError::helper("helper stdin is unavailable"))?;
        let stdout = child
            .stdout
            .take()
            .ok_or_else(|| RuntimeError::helper("helper stdout is unavailable"))?;
        let interaction = interact(&mut child, stdin, stdout, &encoded);
        let result = tokio::time::timeout(remaining, interaction).await;
        let (status, reply) = match result {
            Ok(Ok(result)) => result,
            Ok(Err(error)) => {
                terminate(&mut child).await;
                return Err(error);
            }
            Err(_) => {
                terminate(&mut child).await;
                return Err(RuntimeError::Deadline);
            }
        };
        if !status.success() {
            return Err(RuntimeError::helper("libvirt helper exited unsuccessfully"));
        }
        let reply = HelperReply::parse(&reply).map_err(RuntimeError::helper)?;
        match reply {
            HelperReply::Error(failure) => Err(RuntimeError::from_helper(&failure)),
            reply => Ok(reply),
        }
    }
}

async fn interact(
    child: &mut Child,
    stdin: tokio::process::ChildStdin,
    stdout: tokio::process::ChildStdout,
    request: &[u8],
) -> Result<(std::process::ExitStatus, Vec<u8>), RuntimeError> {
    // The handle is moved in and dropped here: closing it is what the helper
    // sees as end of request, and `shutdown` does not close a child pipe. Held
    // open, every request would run to its deadline instead.
    let write = async move {
        let mut stdin = stdin;
        stdin.write_all(request).await?;
        stdin.shutdown().await
    };
    let read = async {
        let mut reply = Vec::new();
        stdout
            .take((MAX_LIBVIRT_HELPER_REPLY_BYTES + 1) as u64)
            .read_to_end(&mut reply)
            .await?;
        Ok::<_, std::io::Error>(reply)
    };
    let ((), reply, status) =
        tokio::try_join!(write, read, child.wait()).map_err(RuntimeError::helper)?;
    if reply.len() > MAX_LIBVIRT_HELPER_REPLY_BYTES {
        return Err(RuntimeError::helper("libvirt helper reply is oversized"));
    }
    Ok((status, reply))
}

async fn terminate(child: &mut Child) {
    let _ = child.kill().await;
    let _ = child.wait().await;
}
