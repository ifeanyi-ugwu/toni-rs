//! Load shedding (transports DESIGN §2.8): a runtime-free semaphore with two counters, one per
//! server and one per connection where a connection exists. Over a limit a call is refused at
//! once, never queued; each transport refuses in its protocol's own way.

use std::sync::Arc;
use std::time::Duration;

use async_lock::{Semaphore, SemaphoreGuardArc};

/// A server's in-flight bound. Clone it into each connection task; every clone counts against the
/// same limit.
#[derive(Clone)]
pub struct Admission {
    server: Option<Arc<Semaphore>>,
    retry_after: Duration,
}

/// One connection's in-flight bound, nested in its server's.
#[derive(Clone)]
pub struct ConnectionAdmission {
    server: Admission,
    connection: Option<Arc<Semaphore>>,
}

/// One admitted call. Dropping it frees its place in both counters.
pub struct Permit {
    _server: Option<SemaphoreGuardArc>,
    _connection: Option<SemaphoreGuardArc>,
}

impl Admission {
    /// The server's bound: `None` admits every call. A refusal's `Retry-After` is one second
    /// until [`retry_after`](Self::retry_after) sets it, from the server's `.shed_retry_after(..)`.
    pub fn new(limit: Option<usize>) -> Self {
        Admission { server: limit.map(|limit| Arc::new(Semaphore::new(limit))), retry_after: Duration::from_secs(1) }
    }

    pub fn retry_after(self, after: Duration) -> Self {
        Admission { retry_after: after, ..self }
    }

    /// What a refusal carries as `Retry-After`, where its protocol has one.
    pub fn shed_retry_after(&self) -> Duration {
        self.retry_after
    }

    /// A place for one call, or `None` over the server's limit.
    pub fn try_admit(&self) -> Option<Permit> {
        let server = match &self.server {
            Some(semaphore) => Some(semaphore.try_acquire_arc()?),
            None => None,
        };
        Some(Permit { _server: server, _connection: None })
    }

    /// A connection's bound inside this server's: `None` bounds only by the server.
    pub fn connection(&self, limit: Option<usize>) -> ConnectionAdmission {
        ConnectionAdmission { server: self.clone(), connection: limit.map(|limit| Arc::new(Semaphore::new(limit))) }
    }
}

impl ConnectionAdmission {
    /// A place in both counters, or `None` over either limit.
    pub fn try_admit(&self) -> Option<Permit> {
        let connection = match &self.connection {
            Some(semaphore) => Some(semaphore.try_acquire_arc()?),
            None => None,
        };
        let Permit { _server, .. } = self.server.try_admit()?;
        Some(Permit { _server, _connection: connection })
    }

    pub fn shed_retry_after(&self) -> Duration {
        self.server.retry_after
    }
}
