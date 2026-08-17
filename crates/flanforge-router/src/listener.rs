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
    _permit: OwnedSemaphorePermit,
    head: Option<HeadDeadline>,
}

impl BoundedStream {
    fn new(stream: TcpStream, permit: OwnedSemaphorePermit, head_timeout: Duration) -> Self {
        Self {
            stream,
            _permit: permit,
            head: Some(HeadDeadline {
                sleep: Box::pin(tokio::time::sleep(head_timeout)),
                matched: 0,
            }),
        }
    }
}

#[derive(Debug)]
struct HeadDeadline {
    sleep: Pin<Box<Sleep>>,
    matched: usize,
}

impl HeadDeadline {
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
            return Poll::Ready(Err(io::Error::new(
                io::ErrorKind::TimedOut,
                "request head was not delivered in time",
            )));
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
        Pin::new(&mut self.get_mut().stream).poll_write(context, buffer)
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
        Pin::new(&mut self.get_mut().stream).poll_write_vectored(context, buffers)
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
