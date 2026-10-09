//! An upgraded connection carries bytes both ways through `futures-io`'s traits, and with the
//! `tokio-io` feature through tokio's as well, whichever of the two traits the I/O it was built
//! from implements. The tokio cases run where the feature is on, as it is under
//! `cargo test --workspace`, whose tokio-based crates enable it.

use std::collections::VecDeque;
use std::future::Future;
use std::io;
use std::pin::Pin;
use std::task::{Context, Poll};
use std::time::Duration;

use futures::io::{AsyncReadExt, AsyncWriteExt};
use ulo_http::Upgraded;

/// Fails the test when `steps` has not finished in five seconds, so bytes that never arrive fail
/// it rather than hang it.
async fn within(steps: impl Future<Output = ()>) {
    tokio::time::timeout(Duration::from_secs(5), steps).await.expect("the steps finish within five seconds");
}

/// I/O whose reads answer what was written before them, in order.
#[derive(Default)]
struct Loopback {
    queued: VecDeque<u8>,
}

impl futures::io::AsyncRead for Loopback {
    fn poll_read(mut self: Pin<&mut Self>, _cx: &mut Context<'_>, buf: &mut [u8]) -> Poll<io::Result<usize>> {
        let n = buf.len().min(self.queued.len());
        for (slot, byte) in buf.iter_mut().zip(self.queued.drain(..n)) {
            *slot = byte;
        }
        Poll::Ready(Ok(n))
    }
}

impl futures::io::AsyncWrite for Loopback {
    fn poll_write(mut self: Pin<&mut Self>, _cx: &mut Context<'_>, buf: &[u8]) -> Poll<io::Result<usize>> {
        self.queued.extend(buf);
        Poll::Ready(Ok(buf.len()))
    }

    fn poll_flush(self: Pin<&mut Self>, _cx: &mut Context<'_>) -> Poll<io::Result<()>> {
        Poll::Ready(Ok(()))
    }

    fn poll_close(self: Pin<&mut Self>, _cx: &mut Context<'_>) -> Poll<io::Result<()>> {
        Poll::Ready(Ok(()))
    }
}

#[tokio::test]
async fn a_futures_io_connection_reads_back_what_was_written() {
    within(async {
        let mut upgraded = Upgraded::new(Loopback::default());
        upgraded.write_all(b"ping").await.expect("the write goes through");
        upgraded.flush().await.expect("the flush goes through");
        let mut read = [0u8; 4];
        upgraded.read_exact(&mut read).await.expect("the bytes written come back");
        assert_eq!(&read, b"ping");
        upgraded.close().await.expect("the close goes through");
    })
    .await;
}

#[cfg(feature = "tokio-io")]
mod tokio_io {
    use tokio::io::{AsyncReadExt, AsyncWriteExt};
    use ulo_http::Upgraded;

    use super::{Loopback, within};

    #[tokio::test]
    async fn tokio_traits_reach_a_futures_io_connection() {
        within(async {
            let mut upgraded = Upgraded::new(Loopback::default());
            upgraded.write_all(b"ping").await.expect("the write goes through");
            upgraded.flush().await.expect("the flush goes through");
            let mut read = [0u8; 4];
            upgraded.read_exact(&mut read).await.expect("the bytes written come back");
            assert_eq!(&read, b"ping");
            upgraded.shutdown().await.expect("the shutdown goes through");
        })
        .await;
    }

    #[tokio::test]
    async fn a_tokio_connection_carries_bytes_through_either_trait() {
        within(async {
            let (ours, mut peer) = tokio::io::duplex(64);
            let mut upgraded = Upgraded::from_tokio(ours);

            futures::io::AsyncWriteExt::write_all(&mut upgraded, b"ping").await.expect("a write through futures-io");
            futures::io::AsyncWriteExt::flush(&mut upgraded).await.expect("a flush through futures-io");
            let mut read = [0u8; 4];
            peer.read_exact(&mut read).await.expect("the peer reads the futures-io write");
            assert_eq!(&read, b"ping");

            peer.write_all(b"pong").await.expect("the peer writes");
            futures::io::AsyncReadExt::read_exact(&mut upgraded, &mut read).await.expect("a read through futures-io");
            assert_eq!(&read, b"pong");

            upgraded.write_all(b"tick").await.expect("a write through tokio's traits");
            upgraded.flush().await.expect("a flush through tokio's traits");
            peer.read_exact(&mut read).await.expect("the peer reads the tokio write");
            assert_eq!(&read, b"tick");

            peer.write_all(b"tock").await.expect("the peer writes");
            upgraded.read_exact(&mut read).await.expect("a read through tokio's traits");
            assert_eq!(&read, b"tock");

            upgraded.shutdown().await.expect("the shutdown goes through");
            let mut rest = Vec::new();
            peer.read_to_end(&mut rest).await.expect("the peer reads to the end");
            assert!(rest.is_empty(), "the shutdown reached the peer as the end of the stream, with nothing after it");
        })
        .await;
    }
}
