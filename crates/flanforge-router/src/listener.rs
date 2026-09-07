//! Connection bound and request-head deadline for the HTTP ingress.

use std::{
    io,
    net::SocketAddr,
    pin::Pin,
    sync::Arc,
    task::{Context, Poll},
    time::Duration,
};

use axum::serve::{Listener, ListenerExt, TapIo};
use tokio::{
    io::{AsyncRead, AsyncWrite, ReadBuf},
    net::{TcpListener, TcpStream},
    sync::{OwnedSemaphorePermit, Semaphore},
    time::Sleep,
};

const HEAD_TERMINATOR: &[u8] = b"\r\n\r\n";

/// Caps concurrently accepted connections and drops any peer that does not
/// deliver a complete request head in time.
#[derive(Debug)]
pub struct BoundedListener {
    listener: TcpListener,
    permits: Arc<Semaphore>,
    head_timeout: Duration,
}

impl BoundedListener {
    #[must_use]
    pub fn new(listener: TcpListener, max_connections: usize, head_timeout: Duration) -> Self {
        Self {
            listener,
            permits: Arc::new(Semaphore::new(max_connections)),
            head_timeout,
        }
    }
}

impl Listener for BoundedListener {
    type Io = BoundedStream;
    type Addr = SocketAddr;

    async fn accept(&mut self) -> (Self::Io, Self::Addr) {
        loop {
            let Ok(permit) = Arc::clone(&self.permits).acquire_owned().await else {
                tokio::time::sleep(Duration::from_secs(1)).await;
                continue;
            };
            match self.listener.accept().await {
                Ok((stream, address)) => {
                    return (
                        BoundedStream::new(stream, permit, self.head_timeout),
                        address,
                    );
                }
                Err(error) => {
                    tracing::warn!(%error, "cannot accept an ingress connection");
                    drop(permit);
                    tokio::time::sleep(Duration::from_millis(50)).await;
                }
            }
        }
    }

    fn local_addr(&self) -> io::Result<Self::Addr> {
        self.listener.local_addr()
    }
}

/// One accepted connection; releases its slot when dropped.
#[derive(Debug)]
pub struct BoundedStream {
    stream: TcpStream,
    head_timeout: Duration,
    _permit: OwnedSemaphorePermit,
    head: Option<HeadDeadline>,
}

impl BoundedStream {
    fn new(stream: TcpStream, permit: OwnedSemaphorePermit, head_timeout: Duration) -> Self {
        Self {
            stream,
            _permit: permit,
            head_timeout,
            head: Some(HeadDeadline::new(head_timeout, true)),
        }
    }

    /// Writing a response ends the exchange, so the next request head is
    /// bounded again. Without this the deadline disarmed permanently at the
    /// first head, and an idle keep-alive connection held its slot for as long
    /// as the peer liked — enough of them exhaust the bound before any request
    /// is authorized, and none of them need a credential to get there.
    fn await_next_head(&mut self, context: &mut Context<'_>) {
        match self.head.as_mut() {
            // Already waiting on a head: the exchange is still in flight, so
            // only the deadline moves. Partial match progress is kept.
            Some(head) => head
                .sleep
                .as_mut()
                .reset(tokio::time::Instant::now() + self.head_timeout),
            None => self.head = Some(HeadDeadline::new(self.head_timeout, false)),
        }
        // The deadline is checked in `poll_read`, but the peer has gone quiet
        // by definition, so nothing else will wake this task. Polling the
        // timer here registers its waker against the live connection, which is
        // what lets an idle connection be reaped at all.
        if let Some(head) = self.head.as_mut() {
            let _ = head.sleep.as_mut().poll(context);
        }
    }
}

#[derive(Debug)]
struct HeadDeadline {
    sleep: Pin<Box<Sleep>>,
    matched: usize,
    /// Whether this is the connection's first head. A later one is a
    /// keep-alive idle wait, which is reaped as a clean close rather than
    /// reported as a peer that failed to deliver.
    is_first: bool,
}

impl HeadDeadline {
    fn new(timeout: Duration, is_first: bool) -> Self {
        Self {
            sleep: Box::pin(tokio::time::sleep(timeout)),
            matched: 0,
            is_first,
        }
    }

    /// Advances the end-of-head match across reads that may split it.
    fn is_complete(&mut self, bytes: &[u8]) -> bool {
        for byte in bytes {
            if HEAD_TERMINATOR.get(self.matched) == Some(byte) {
                self.matched += 1;
                if self.matched == HEAD_TERMINATOR.len() {
                    return true;
                }
            } else {
                self.matched = usize::from(*byte == HEAD_TERMINATOR[0]);
            }
        }
        false
    }
}

impl AsyncRead for BoundedStream {
    fn poll_read(
        self: Pin<&mut Self>,
        context: &mut Context<'_>,
        buffer: &mut ReadBuf<'_>,
    ) -> Poll<io::Result<()>> {
        let this = self.get_mut();
        if let Some(head) = this.head.as_mut()
            && head.sleep.as_mut().poll(context).is_ready()
        {
            return if head.is_first {
                Poll::Ready(Err(io::Error::new(
                    io::ErrorKind::TimedOut,
                    "request head was not delivered in time",
                )))
            } else {
                // An idle keep-alive peer is reaped, not faulted: it did
                // nothing wrong, it is just holding a slot someone else needs.
                // An empty read is end-of-stream, so the connection closes.
                Poll::Ready(Ok(()))
            };
        }
        let start = buffer.filled().len();
        let result = Pin::new(&mut this.stream).poll_read(context, buffer);
        if matches!(result, Poll::Ready(Ok(())))
            && let Some(head) = this.head.as_mut()
            && head.is_complete(&buffer.filled()[start..])
        {
            this.head = None;
        }
        result
    }
}

impl AsyncWrite for BoundedStream {
    fn poll_write(
        self: Pin<&mut Self>,
        context: &mut Context<'_>,
        buffer: &[u8],
    ) -> Poll<io::Result<usize>> {
        let this = self.get_mut();
        this.await_next_head(context);
        Pin::new(&mut this.stream).poll_write(context, buffer)
    }

    fn poll_flush(self: Pin<&mut Self>, context: &mut Context<'_>) -> Poll<io::Result<()>> {
        Pin::new(&mut self.get_mut().stream).poll_flush(context)
    }

    fn poll_shutdown(self: Pin<&mut Self>, context: &mut Context<'_>) -> Poll<io::Result<()>> {
        Pin::new(&mut self.get_mut().stream).poll_shutdown(context)
    }

    fn poll_write_vectored(
        self: Pin<&mut Self>,
        context: &mut Context<'_>,
        buffers: &[io::IoSlice<'_>],
    ) -> Poll<io::Result<usize>> {
        let this = self.get_mut();
        this.await_next_head(context);
        Pin::new(&mut this.stream).poll_write_vectored(context, buffers)
    }

    fn is_write_vectored(&self) -> bool {
        self.stream.is_write_vectored()
    }
}

/// Wraps the listener so axum can produce peer connect info: `Connected` is
/// implemented for axum's own listeners and for `TapIo`, not for ours.
#[must_use]
pub fn with_peer_info(
    listener: BoundedListener,
) -> TapIo<BoundedListener, impl FnMut(&mut BoundedStream) + Send + 'static> {
    listener.tap_io(|_| {})
}
