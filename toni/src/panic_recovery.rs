//! Shared `catch_unwind` wrappers so every pipeline segment gets the same
//! [`PanicRecovered`] event shape on unwind.
//!
//! Each transport's dispatcher wraps user-supplied callbacks — guards,
//! interceptors, pipes, error handlers, response renderers — in
//! `AssertUnwindSafe(...).catch_unwind()` and converts the payload into a
//! [`PanicRecovered`] tagged with the matching [`PipelineSegment`]. The
//! event then flows through the existing observer + error chain rather
//! than escaping the dispatcher and tearing down the request.
//!
//! Two flavours: [`catch_async`] for `Future`-returning callbacks (guards,
//! interceptors, handlers, error handlers) and [`catch_sync`] for the rare
//! synchronous segment (today: pipes' `process(&mut C)`).
//!
//! `AssertUnwindSafe` is load-bearing — user code isn't required to be
//! `UnwindSafe`, and adding that bound to public traits would propagate
//! deep into application code. The dispatcher is responsible for not
//! reusing borrowed state after a caught unwind, which is the case at
//! every wrap site below since the callback owns its own borrow scope.

use std::future::Future;
use std::panic::AssertUnwindSafe;

use futures::FutureExt;

use crate::errors::{PanicRecovered, PipelineSegment};

/// Drive an async callback, returning [`PanicRecovered`] tagged with
/// `segment` on caught unwind.
pub async fn catch_async<Fut, T>(segment: PipelineSegment, fut: Fut) -> Result<T, PanicRecovered>
where
    Fut: Future<Output = T>,
{
    match AssertUnwindSafe(fut).catch_unwind().await {
        Ok(v) => Ok(v),
        Err(payload) => Err(PanicRecovered::from_panic_payload(segment, payload)),
    }
}

/// Invoke a synchronous callback, returning [`PanicRecovered`] tagged with
/// `segment` on caught unwind. Used for segments whose trait method is
/// sync (e.g. `Pipe::process`).
pub fn catch_sync<F, T>(segment: PipelineSegment, f: F) -> Result<T, PanicRecovered>
where
    F: FnOnce() -> T,
{
    match std::panic::catch_unwind(AssertUnwindSafe(f)) {
        Ok(v) => Ok(v),
        Err(payload) => Err(PanicRecovered::from_panic_payload(segment, payload)),
    }
}
