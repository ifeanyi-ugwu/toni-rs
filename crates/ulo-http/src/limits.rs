use std::time::Duration;

use ulo::{Bound, TransportMetadata};

use crate::transport::Http;

/// 1024 bytes, for `.body_limit(64 * KB)`.
pub const KB: u64 = 1024;

/// 1024 × 1024 bytes, for `.body_limit(2 * MB)`.
pub const MB: u64 = 1024 * 1024;

/// A route's body limit, overriding the server's `.body_limit(..)`:
/// `#[meta(BodyLimit(50 * MB))]`. A body over it fails with 413 before deserialization.
///
/// HTTP metadata: written on a handler of another transport, it fails to compile there (X24).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct BodyLimit(pub u64);

impl TransportMetadata for BodyLimit {
    type Transport = Http;
}

/// A route's timeout: `#[meta(Timeout::after(Duration::from_secs(5)))]`. When it passes, the
/// execution is cancelled with `CancelReason::Deadline`. An answer not yet started is dropped, and
/// the error handlers receive `Timeout` under the server's `timeout_grace`. Their answer is the
/// response when it is produced within the grace, its body included: a body that ends in time is
/// buffered and written whole, with its exact length. Unclaimed, not answered within the grace, or
/// answered with a body still open when the grace runs out, an `Sse` among them, or one yielding
/// more than 1 MiB before it ends, it renders 504.
///
/// The most specific declaration wins, as for all metadata, so under a `#[routes]` impl that
/// declares a timeout, a handler's own declaration decides:
///
/// | Handler declares | Effect |
/// |---|---|
/// | nothing | the impl's timeout |
/// | `Timeout::after(d)` | `d` |
/// | `Timeout::OFF` | no timeout |
/// | `Timeout(Bound::Default)` | the server's route-timeout default, which is none |
///
/// `Bound::Default` means the server's default here as everywhere else, not the impl's timeout; a
/// handler inherits the impl's timeout by declaring nothing.
///
/// `Timeout::after(Duration::ZERO)` is refused in `prepare`, once per declaration, naming every
/// route that runs with it. A zero that every handler under it replaces is not refused.
///
/// HTTP metadata: written on a handler of another transport, it fails to compile there (X24).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Timeout(pub Bound);

impl TransportMetadata for Timeout {
    type Transport = Http;
}

impl Timeout {
    /// No timeout, lifting one the `#[routes]` impl declares: `#[meta(Timeout::OFF)]`.
    pub const OFF: Timeout = Timeout(Bound::Unbounded);

    /// A timeout of `d`: `Timeout(Bound::After(d))`.
    pub const fn after(d: Duration) -> Timeout {
        Timeout(Bound::After(d))
    }

    /// How long the route may run, `None` for no timeout.
    pub(crate) fn duration(self) -> Option<Duration> {
        match self.0 {
            Bound::After(d) => Some(d),
            // The server has no route-timeout default, so `Default` is no timeout.
            Bound::Default | Bound::Unbounded => None,
        }
    }
}
