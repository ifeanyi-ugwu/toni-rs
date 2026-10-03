use std::time::Duration;

/// 1024 bytes, for `.body_limit(64 * KB)`.
pub const KB: u64 = 1024;

/// 1024 × 1024 bytes, for `.body_limit(2 * MB)`.
pub const MB: u64 = 1024 * 1024;

/// A route's body limit, overriding the server's `.body_limit(..)`:
/// `#[meta(BodyLimit(50 * MB))]`. A body over it fails with 413 before deserialization.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct BodyLimit(pub u64);

/// A route's timeout: `#[meta(Timeout(Duration::from_secs(5)))]`. When it passes, the execution is
/// cancelled with `CancelReason::Deadline` and an answer not yet started renders 504.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Timeout(pub Duration);
