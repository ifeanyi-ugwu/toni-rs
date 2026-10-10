//! A value whose changes wake the tasks waiting on it, without a runtime: a `std` mutex for the
//! value and an `event-listener` event for the wake-up. `ulo-rpc`'s server keeps its close signal
//! and drain state in one, and `ulo-ws`'s connection tracker its phase, drain token and count of
//! live connections.

use std::sync::{Mutex, MutexGuard, PoisonError};

use event_listener::{Event, EventListener};

/// A value and the event its changes notify.
///
/// An `Event` keeps no permit for a waiter that has not registered yet, so every wait registers
/// its listener before it reads the value: a change made between the read and the wait then still
/// wakes it.
pub struct Watch<T> {
    value: Mutex<T>,
    changed: Event,
}

impl<T> Watch<T> {
    pub fn new(value: T) -> Self {
        Watch { value: Mutex::new(value), changed: Event::new() }
    }

    /// Changes the value under its lock, then wakes every waiter.
    pub fn modify(&self, change: impl FnOnce(&mut T)) {
        change(&mut self.lock());
        self.changed.notify(usize::MAX);
    }

    pub fn read<R>(&self, read: impl FnOnce(&T) -> R) -> R {
        read(&self.lock())
    }

    /// A listener that resolves at the next change. Registered before the value is read, it is
    /// woken by any change made after the read.
    pub fn changed(&self) -> EventListener {
        self.changed.listen()
    }

    /// Resolves once `ready` holds for the value.
    pub async fn wait_for(&self, ready: impl Fn(&T) -> bool) {
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

#[cfg(test)]
mod tests {
    use std::future::Future;
    use std::pin::pin;
    use std::sync::atomic::{AtomicBool, Ordering};
    use std::task::{Context, Waker};

    use super::Watch;

    /// A writer's change landing between `wait_for`'s read of the value and its wait: the read sees
    /// the old value, and the change and its notification follow at once. The writer is modelled
    /// inside `ready`, after it has read, since the value's lock is held there; the notification
    /// goes to the event directly for the same reason.
    #[test]
    fn a_change_between_the_read_and_the_wait_wakes_the_waiter() {
        let watch = Watch::new(AtomicBool::new(false));
        let first = AtomicBool::new(true);
        let waiting = watch.wait_for(|value| {
            let seen = value.load(Ordering::SeqCst);
            if first.swap(false, Ordering::SeqCst) {
                value.store(true, Ordering::SeqCst);
                watch.changed.notify(usize::MAX);
            }
            seen
        });
        let mut waiting = pin!(waiting);
        let polled = waiting.as_mut().poll(&mut Context::from_waker(Waker::noop()));
        assert!(
            polled.is_ready(),
            "a change made after the value was read and before the wait began was missed: the wait is still pending with the value \
             now {}",
            watch.read(|value| value.load(Ordering::SeqCst))
        );
    }
}
