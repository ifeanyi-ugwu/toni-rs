//! hyper's runtime edges over the app's: its executor and timer over a `ulo` `Runtime` and
//! `Timer`, and its I/O traits over `futures-io`'s.

use std::future::Future;
use std::io;
use std::mem::MaybeUninit;
use std::pin::Pin;
use std::sync::{Arc, Mutex, PoisonError};
use std::task::{Context, Poll, ready};
use std::time::{Duration, Instant};

use hyper::rt::{Executor, Read, ReadBuf, ReadBufCursor, Sleep, Timer, Write};
use ulo::{BoxFuture, Runtime};

/// hyper's executor over a `ulo` [`Runtime`]: each future hyper hands it, HTTP/2's stream and
/// connection tasks among them, is spawned on the runtime and detached. Built from the app's
/// runtime, `Mounted::runtime()` in a server's `bind`, so a connection's tasks run on the runtime
/// the app runs on and a panic in one is redacted with the app's secrets.
#[derive(Clone)]
pub struct RuntimeExecutor {
    runtime: Arc<dyn Runtime>,
}

impl RuntimeExecutor {
    pub fn new(runtime: Arc<dyn Runtime>) -> RuntimeExecutor {
        RuntimeExecutor { runtime }
    }
}

impl<F> Executor<F> for RuntimeExecutor
where
    F: Future + Send + 'static,
{
    fn execute(&self, fut: F) {
        // Dropping the handle detaches the task, which hyper's own executors do too.
        drop(self.runtime.spawn(Box::pin(async move {
            fut.await;
        })));
    }
}

/// hyper's timer over a `ulo` [`Timer`](ulo::Timer): the HTTP/1.1 header-read timeout, and any
/// other clock a connection builder reads, on the app's clock. A deadline is measured against that
/// clock's `now`, which hyper reads through [`Timer::now`] too, so a test's fake clock is the one
/// both sides agree on.
#[derive(Clone)]
pub struct RuntimeTimer {
    timer: Arc<dyn ulo::Timer>,
}

impl RuntimeTimer {
    pub fn new(timer: Arc<dyn ulo::Timer>) -> RuntimeTimer {
        RuntimeTimer { timer }
    }
}

impl Timer for RuntimeTimer {
    fn sleep(&self, duration: Duration) -> Pin<Box<dyn Sleep>> {
        Box::pin(AppSleep(Mutex::new(self.timer.sleep(duration))))
    }

    fn sleep_until(&self, deadline: Instant) -> Pin<Box<dyn Sleep>> {
        self.sleep(deadline.saturating_duration_since(self.timer.now()))
    }

    fn now(&self) -> Instant {
        self.timer.now()
    }
}

/// One of the app timer's sleeps as hyper's `Sleep`, which must be `Sync`. The mutex is what makes
/// it so; it is only ever reached through `&mut`, so it is never contended.
struct AppSleep(Mutex<BoxFuture<'static, ()>>);

impl Future for AppSleep {
    type Output = ();

    fn poll(self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<()> {
        self.get_mut().0.get_mut().unwrap_or_else(PoisonError::into_inner).as_mut().poll(cx)
    }
}

impl Sleep for AppSleep {}

/// A stream between `futures-io`'s traits and hyper's, either way round, as hyper-util's `TokioIo`
/// is between tokio's and hyper's.
///
/// Around a `futures-io` stream, an `async-net` socket or a `futures-rustls` session, it
/// implements hyper's [`Read`] and [`Write`], so hyper serves it. `futures-io` reads into
/// initialized memory and hyper hands out uninitialized memory, so each read zeroes the part of
/// hyper's buffer it offers first. Around hyper's own I/O, an upgraded connection, it implements
/// `futures-io`'s `AsyncRead` and `AsyncWrite`, which need no zeroing.
#[derive(Debug)]
pub struct FuturesIo<T> {
    inner: T,
}

impl<T> FuturesIo<T> {
    pub fn new(inner: T) -> FuturesIo<T> {
        FuturesIo { inner }
    }

    pub fn get_ref(&self) -> &T {
        &self.inner
    }

    pub fn into_inner(self) -> T {
        self.inner
    }
}

impl<T: futures_io::AsyncRead + Unpin> Read for FuturesIo<T> {
    fn poll_read(self: Pin<&mut Self>, cx: &mut Context<'_>, mut buf: ReadBufCursor<'_>) -> Poll<io::Result<()>> {
        // SAFETY: `as_mut` hands out the unfilled memory; every byte of it is written before the
        // slice is read as initialized, and nothing initialized is ever uninitialized.
        let unfilled: &mut [u8] = unsafe {
            let unfilled: &mut [MaybeUninit<u8>] = buf.as_mut();
            for byte in unfilled.iter_mut() {
                byte.write(0);
            }
            &mut *(std::ptr::from_mut::<[MaybeUninit<u8>]>(unfilled) as *mut [u8])
        };
        let read = ready!(Pin::new(&mut self.get_mut().inner).poll_read(cx, unfilled))?;
        // SAFETY: the whole unfilled region was initialized above, and `read` never exceeds it.
        unsafe { buf.advance(read) };
        Poll::Ready(Ok(()))
    }
}

impl<T: futures_io::AsyncWrite + Unpin> Write for FuturesIo<T> {
    fn poll_write(self: Pin<&mut Self>, cx: &mut Context<'_>, buf: &[u8]) -> Poll<io::Result<usize>> {
        Pin::new(&mut self.get_mut().inner).poll_write(cx, buf)
    }

    fn poll_flush(self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<io::Result<()>> {
        Pin::new(&mut self.get_mut().inner).poll_flush(cx)
    }

    fn poll_shutdown(self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<io::Result<()>> {
        Pin::new(&mut self.get_mut().inner).poll_close(cx)
    }

    fn poll_write_vectored(self: Pin<&mut Self>, cx: &mut Context<'_>, bufs: &[io::IoSlice<'_>]) -> Poll<io::Result<usize>> {
        Pin::new(&mut self.get_mut().inner).poll_write_vectored(cx, bufs)
    }
}

impl<T: Read + Unpin> futures_io::AsyncRead for FuturesIo<T> {
    fn poll_read(self: Pin<&mut Self>, cx: &mut Context<'_>, buf: &mut [u8]) -> Poll<io::Result<usize>> {
        let mut read = ReadBuf::new(buf);
        ready!(Pin::new(&mut self.get_mut().inner).poll_read(cx, read.unfilled()))?;
        Poll::Ready(Ok(read.filled().len()))
    }
}

impl<T: Write + Unpin> futures_io::AsyncWrite for FuturesIo<T> {
    fn poll_write(self: Pin<&mut Self>, cx: &mut Context<'_>, buf: &[u8]) -> Poll<io::Result<usize>> {
        Pin::new(&mut self.get_mut().inner).poll_write(cx, buf)
    }

    fn poll_write_vectored(self: Pin<&mut Self>, cx: &mut Context<'_>, bufs: &[io::IoSlice<'_>]) -> Poll<io::Result<usize>> {
        Pin::new(&mut self.get_mut().inner).poll_write_vectored(cx, bufs)
    }

    fn poll_flush(self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<io::Result<()>> {
        Pin::new(&mut self.get_mut().inner).poll_flush(cx)
    }

    fn poll_close(self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<io::Result<()>> {
        Pin::new(&mut self.get_mut().inner).poll_shutdown(cx)
    }
}
