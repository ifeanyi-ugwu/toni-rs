//! A value whose changes wake the tasks waiting on it, a tracker's phase, drain token and count of
//! live connections, without a runtime: a `std` mutex for the value and an `event-listener` event
//! for the wake-up, as `ulo-rpc`'s server builds its own.

use std::sync::{Mutex, MutexGuard, PoisonError};

use event_listener::{Event, EventListener};

pub(crate) struct Watch<T> {
    value: Mutex<T>,
    changed: Event,
}

impl<T> Watch<T> {
    pub(crate) fn new(value: T) -> Self {
        Watch { value: Mutex::new(value), changed: Event::new() }
    }

    /// Changes the value under its lock, then wakes every waiter.
    pub(crate) fn modify(&self, change: impl FnOnce(&mut T)) {
        change(&mut self.lock());
        self.changed.notify(usize::MAX);
    }

    pub(crate) fn read<R>(&self, read: impl FnOnce(&T) -> R) -> R {
        read(&self.lock())
    }

    /// A listener that resolves at the next change. Registered before the value is read, it is
    /// woken by any change made after the read.
    pub(crate) fn changed(&self) -> EventListener {
        self.changed.listen()
    }

    /// Resolves once `ready` holds for the value. Each listener is registered before the value is
    /// read, so a change made between the read and the wait still wakes it.
    pub(crate) async fn wait_for(&self, ready: impl Fn(&T) -> bool) {
        loop {
            let changed = self.changed.listen();
            if self.read(&ready) {
                return;
            }
            changed.await;
        }
    }

    fn lock(&self) -> MutexGuard<'_, T> {
        self.value.lock().unwrap_or_else(PoisonError::into_inner)
    }
}
