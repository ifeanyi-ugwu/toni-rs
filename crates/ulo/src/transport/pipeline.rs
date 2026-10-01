//! The dispatch pipeline for one call (§7), shared by every transport:
//!
//! 1. The transport opens an `Execution` in the handler's module, seeds the inputs, and builds
//!    its `Cx` around a handle to it.
//! 2. Guards, global then controller then method: the global ones read through
//!    `Resolver::entries::<AnyGuard<T>>()`, each obtained only once the one before it admitted
//!    (the shared value, the singleton, a per-execution build, or the closure call), then
//!    `can_activate`. On a refusal, later guards are never built.
//! 3. Once every guard admits: the interceptors, then the handler inside the interceptor chain.
//!    The handler closure resolves the controller, built per call if inferred per-execution.
//! 4. Errors go through the error handlers, method first, then controller, then global. A
//!    refusal is one of them: `Ok(false)` becomes `GuardRejected { guard }`. An error no handler
//!    claims is returned for the transport to render (its forbidden status for `GuardRejected`).
//! 5. The transport drops its `Cx` when the reply is written; a streaming reply holds a clone.

use std::future::Future;

use crate::execution::ExecutionRef;
use crate::timer::BoxError;
use crate::transport::Transport;
use crate::transport::controller::MountedHandler;

/// Runs `handler`'s enhancer stack around `call` for one call. `exec` is the call's execution,
/// the one `cx` wraps; `call` receives a clone of `cx` and answers the handler's reply.
pub async fn dispatch<T, F, Fut>(handler: &MountedHandler<T>, exec: &ExecutionRef, cx: &T::Cx, call: F) -> Result<T::Reply, BoxError>
where
    T: Transport,
    F: FnOnce(T::Cx) -> Fut + Send,
    Fut: Future<Output = Result<T::Reply, BoxError>> + Send,
{
    todo!()
}
