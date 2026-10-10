//! A broker link's held `cancel`, and the hold on it a test can see and shorten.
//!
//! NATS, RabbitMQ, MQTT and Kafka hold a streamed request's `cancel` sent before the server
//! acknowledged its `open`, and send it alone when the `opened` arrives. A held `cancel` is kept
//! for [`CANCEL_HOLD`] and then dropped with its entry, so a call whose `opened` never comes holds
//! nothing until the link closes.

use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Mutex, PoisonError};
use std::time::Duration;

/// How long a broker link keeps a `cancel` waiting for its call's `opened`: the RPC client's
/// default timeout, so an `opened` later than a whole default wait after the `cancel` is taken as
/// one that never comes.
pub const CANCEL_HOLD: Duration = Duration::from_secs(5);

/// One broker link's client side as the RPC conformance suite reaches it: the calls its table
/// holds, the `opened` acknowledgments it withholds on request and delivers late, and the hold
/// on a held `cancel`, which a scenario shortens. A link holds one from its construction and
/// attaches each client side it connects.
#[derive(Default)]
pub struct ClientProbe {
    withholding: AtomicBool,
    withheld: Mutex<Vec<u64>>,
    hold: Mutex<Option<Duration>>,
    attached: Mutex<Option<Attached>>,
}

struct Attached {
    opened: Box<dyn Fn(u64) + Send + Sync>,
    calls: Box<dyn Fn() -> usize + Send + Sync>,
}

impl ClientProbe {
    /// Attaches a connected client side: `opened` takes an acknowledgment for call `id` as if it
    /// had arrived now, and `calls` counts the entries in its table. Replaces the side attached
    /// before.
    pub fn attach(&self, opened: impl Fn(u64) + Send + Sync + 'static, calls: impl Fn() -> usize + Send + Sync + 'static) {
        *lock(&self.attached) = Some(Attached { opened: Box::new(opened), calls: Box::new(calls) });
    }

    /// What the link asks of each `opened` it receives: `true` when the probe withholds it, the
    /// link then dropping it, to be delivered by [`release`](Self::release).
    pub fn withholds(&self, id: u64) -> bool {
        if !self.withholding.load(Ordering::Acquire) {
            return false;
        }
        lock(&self.withheld).push(id);
        true
    }

    /// Withholds every `opened` from now until [`release`](Self::release).
    pub fn withhold(&self) {
        self.withholding.store(true, Ordering::Release);
    }

    /// How many `opened` acknowledgments the probe withholds.
    pub fn withheld(&self) -> usize {
        lock(&self.withheld).len()
    }

    /// Stops withholding and delivers each withheld `opened` to the attached client side.
    pub fn release(&self) {
        self.withholding.store(false, Ordering::Release);
        let withheld = std::mem::take(&mut *lock(&self.withheld));
        if let Some(attached) = &*lock(&self.attached) {
            for id in withheld {
                (attached.opened)(id);
            }
        }
    }

    /// The entries in the attached client side's table, `None` before one is attached.
    pub fn calls(&self) -> Option<usize> {
        lock(&self.attached).as_ref().map(|attached| (attached.calls)())
    }

    /// Keeps a held `cancel` for `hold` in place of [`CANCEL_HOLD`].
    pub fn set_hold(&self, hold: Duration) {
        *lock(&self.hold) = Some(hold);
    }

    /// How long the link keeps a held `cancel`.
    pub fn hold(&self) -> Duration {
        lock(&self.hold).unwrap_or(CANCEL_HOLD)
    }
}

fn lock<T>(mutex: &Mutex<T>) -> std::sync::MutexGuard<'_, T> {
    mutex.lock().unwrap_or_else(PoisonError::into_inner)
}
