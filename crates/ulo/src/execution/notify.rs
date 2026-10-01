//! A one-shot, runtime-free notification with a waker list, and the two public futures built on
//! it: cancellation of one execution, and the app's stop-accepting notice.

use std::future::Future;
use std::pin::Pin;
use std::sync::Mutex;
use std::sync::atomic::{AtomicBool, Ordering};
use std::task::{Context, Poll, Waker};

/// Fires once and stays fired. Every listener registered before or after the fire resolves.
#[derive(Default)]
pub(crate) struct Notify {
    fired: AtomicBool,
    wakers: Mutex<Vec<Waker>>,
}

impl Notify {
    pub(crate) fn new() -> Self {
        Notify::default()
    }

    pub(crate) fn fire(&self) {
        todo!()
    }

    pub(crate) fn is_fired(&self) -> bool {
        self.fired.load(Ordering::Acquire)
    }

    pub(crate) fn listen(&self) -> Listen<'_> {
        Listen { notify: self }
    }
}

/// Resolves once its `Notify` has fired.
pub(crate) struct Listen<'a> {
    notify: &'a Notify,
}

impl Future for Listen<'_> {
    type Output = ();

    fn poll(self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<()> {
        todo!()
    }
}

/// Resolves when the execution is cancelled: at a client disconnect or the deadline for a call,
/// at the deadline for a standalone execution, and at the end of the drain for either.
pub struct Cancelled<'a> {
    inner: Listen<'a>,
}

impl<'a> Cancelled<'a> {
    pub(crate) fn new(inner: Listen<'a>) -> Self {
        Cancelled { inner }
    }
}

impl Future for Cancelled<'_> {
    type Output = ();

    fn poll(mut self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<()> {
        Pin::new(&mut self.inner).poll(cx)
    }
}

/// Resolves when the app stops accepting (§9.5), the moment transports send GOAWAY and close
/// idle keep-alives. The same future on an execution and on an `AppHandle`, resolving at the
/// same moment; it does not resolve during the before-shutdown stage.
pub struct Draining<'a> {
    inner: Listen<'a>,
}

impl<'a> Draining<'a> {
    pub(crate) fn new(inner: Listen<'a>) -> Self {
        Draining { inner }
    }
}

impl Future for Draining<'_> {
    type Output = ();

    fn poll(mut self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<()> {
        Pin::new(&mut self.inner).poll(cx)
    }
}
