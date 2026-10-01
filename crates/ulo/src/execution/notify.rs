//! A one-shot, runtime-free notification with a waker list, and the two public futures built on
//! it: cancellation of one execution, and the app's stop-accepting notice.

use std::future::Future;
use std::pin::Pin;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Mutex, MutexGuard, PoisonError};
use std::task::{Context, Poll, Waker};

/// Fires once and stays fired. Every listener registered before or after the fire resolves.
#[derive(Default)]
pub(crate) struct Notify {
    fired: AtomicBool,
    waiters: Mutex<Waiters>,
}

/// One slot per pending listener, so a listener polled many times before the fire, such as a
/// `Cancelled` raced in a loop, replaces its own waker instead of growing the list.
#[derive(Default)]
struct Waiters {
    slots: Vec<Option<Waker>>,
    free: Vec<usize>,
}

impl Notify {
    pub(crate) fn new() -> Self {
        Notify::default()
    }

    pub(crate) fn fire(&self) {
        if self.fired.swap(true, Ordering::AcqRel) {
            return;
        }
        // `fired` is set before the lock is taken, and a listener re-checks it under the lock
        // before registering, so no waker can be pushed after this take.
        let slots = {
            let mut waiters = self.waiters();
            waiters.free.clear();
            std::mem::take(&mut waiters.slots)
        };
        // Woken outside the lock: an executor may poll the listener inside `wake`.
        for waker in slots.into_iter().flatten() {
            waker.wake();
        }
    }

    pub(crate) fn is_fired(&self) -> bool {
        self.fired.load(Ordering::Acquire)
    }

    pub(crate) fn listen(&self) -> Listen<'_> {
        Listen { notify: self, slot: None }
    }

    fn waiters(&self) -> MutexGuard<'_, Waiters> {
        self.waiters.lock().unwrap_or_else(PoisonError::into_inner)
    }
}

/// Resolves once its `Notify` has fired.
pub(crate) struct Listen<'a> {
    notify: &'a Notify,
    slot: Option<usize>,
}

impl Future for Listen<'_> {
    type Output = ();

    fn poll(mut self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<()> {
        let this = &mut *self;
        if this.notify.is_fired() {
            return Poll::Ready(());
        }
        let mut waiters = this.notify.waiters();
        if this.notify.is_fired() {
            return Poll::Ready(());
        }
        match this.slot.filter(|&i| i < waiters.slots.len()) {
            Some(i) => {
                let slot = &mut waiters.slots[i];
                if !slot.as_ref().is_some_and(|waker| waker.will_wake(cx.waker())) {
                    *slot = Some(cx.waker().clone());
                }
            }
            None => {
                let waker = Some(cx.waker().clone());
                let index = match waiters.free.pop() {
                    Some(i) => {
                        waiters.slots[i] = waker;
                        i
                    }
                    None => {
                        waiters.slots.push(waker);
                        waiters.slots.len() - 1
                    }
                };
                this.slot = Some(index);
            }
        }
        Poll::Pending
    }
}

impl Drop for Listen<'_> {
    fn drop(&mut self) {
        let Some(index) = self.slot.take() else { return };
        let mut waiters = self.notify.waiters();
        // After the fire the slots were taken and the index means nothing.
        if self.notify.is_fired() {
            return;
        }
        if let Some(slot) = waiters.slots.get_mut(index) {
            *slot = None;
            waiters.free.push(index);
        }
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
