//! Per-call helpers invoked from `#[grpc_methods]`-generated code.
//!
//! Lives here so the chain logic is tonic-free, unit-testable, and shared
//! across every gRPC service the macro emits. The macro generates a thin
//! per-method shim that builds a [`GrpcContext`], calls into this module,
//! maps any [`GrpcStatus`] back to `tonic::Status`, then either returns or
//! delegates to the user's body.

use std::collections::HashMap;
use std::future::Future;
use std::sync::{Arc, Mutex};

use async_trait::async_trait;

use crate::adapter::ResolvedGrpcEnhancers;
use crate::context::GrpcContext;
use crate::errors::{GuardRejection, PipelineSegment};
use crate::grpc_status::GrpcHandlerResult;
use crate::grpc_status::GrpcStatus;
use crate::panic_recovery::catch_async;
use crate::traits_helpers::{
    GrpcGuardEntry, GrpcInterceptorEntry, Guard, Interceptor, InterceptorNext,
};

/// Run guards then wrap the user delegation in the interceptor chain.
///
/// `delegate` is the user's `<UserType as ProtoTrait>::method(&self.inner, req)`
/// call, packaged as a closure that returns `()`. The user's typed
/// `Result<Response<_>, Status>` is method-specific and can't fit a
/// generic chain-runner signature, so the macro stashes it in an
/// `Arc<Mutex<Option<_>>>` side-channel inside the closure and reads it
/// back after `run_grpc_pipeline` returns.
///
/// Returns `Err(GrpcStatus)` when a guard rejects or an interceptor answers
/// with one instead of calling `next.run(ctx)`. The macro maps `GrpcStatus` to
/// `tonic::Status` at the wire boundary; `Ok(())` means the chain completed
/// normally and the user's delegate (which fills the side-channel) was reached.
pub async fn run_grpc_pipeline<D, Fut>(
    ctx: &GrpcContext,
    enhancers: &ResolvedGrpcEnhancers,
    method: &str,
    delegate: D,
) -> Result<(), GrpcStatus>
where
    D: FnOnce() -> Fut + Send + 'static,
    Fut: Future<Output = ()> + Send + 'static,
{
    run_grpc_guards_inline(ctx, enhancers, method).await?;

    let mut all_interceptors = enhancers.interceptors.clone();
    if let Some(per_method) = enhancers.handler_interceptors.get(method) {
        all_interceptors.extend_from_slice(per_method);
    }
    let interceptors = resolve_interceptors(&all_interceptors, ctx).await;

    // An interceptor panic is caught deep in the link chain, where neither the
    // enhancers nor the method name are in scope. The slot carries the event
    // back out to here, which has both, so the chain gets first claim on it —
    // the same side-channel the generated wrapper uses for a handler panic.
    let panicked: PanicSlot = Arc::new(Mutex::new(None));
    let answer = execute_with_interceptors(ctx, &interceptors, panicked.clone(), delegate).await;

    // Bound to a local: the guard must drop before the `.await` below, or the
    // future stops being `Send`.
    let caught = panicked
        .lock()
        .expect("interceptor panic slot poisoned")
        .take();
    match caught {
        Some(event) => {
            let claimed = run_grpc_error_chain(ctx, enhancers, method, &event).await;
            Err(claimed.unwrap_or_else(|| {
                GrpcStatus::new(
                    crate::grpc_status::GrpcCode::Internal,
                    format!("interceptor panicked: {}", event.message),
                )
            }))
        }
        None => answer,
    }
}

/// Carries a caught interceptor panic out of the link chain to
/// [`run_grpc_pipeline`], which holds what the error chain needs.
type PanicSlot = Arc<Mutex<Option<crate::errors::PanicRecovered>>>;

/// Guards-only entry point — same shape as PR #1 shipped, retained for
/// services that declare no interceptors so the macro can skip the
/// closure-boxing cost.
pub async fn run_grpc_guards(
    ctx: &GrpcContext,
    enhancers: &ResolvedGrpcEnhancers,
    method: &str,
) -> Result<(), GrpcStatus> {
    run_grpc_guards_inline(ctx, enhancers, method).await
}

async fn run_grpc_guards_inline(
    ctx: &GrpcContext,
    enhancers: &ResolvedGrpcEnhancers,
    method: &str,
) -> Result<(), GrpcStatus> {
    let mut all_guards = enhancers.guards.clone();
    if let Some(per_method) = enhancers.handler_guards.get(method) {
        all_guards.extend_from_slice(per_method);
    }

    let guards = resolve_guards(&all_guards, ctx).await;
    for (index, guard) in guards.iter().enumerate() {
        // A panicking guard is a bug, not a verdict: the chain gets first
        // claim on the typed event, and an unclaimed one renders `Internal`
        // rather than telling the caller its credentials were refused.
        let activated = match catch_async(PipelineSegment::Guard, guard.can_activate(ctx)).await {
            Ok(b) => b,
            Err(event) => {
                tracing::debug!(guard_index = index, panic = %event.message, "guard panicked");
                let claimed = run_grpc_error_chain(ctx, enhancers, method, &event).await;
                return Err(claimed.unwrap_or_else(|| {
                    GrpcStatus::new(
                        crate::grpc_status::GrpcCode::Internal,
                        format!("guard {} panicked: {}", index, event.message),
                    )
                }));
            }
        };
        if !activated {
            // The chain gets first claim, as it does on HTTP; an unclaimed
            // refusal renders as the `PermissionDenied` it always did.
            let event = GuardRejection::new(index);
            let claimed = run_grpc_error_chain(ctx, enhancers, method, &event).await;
            return Err(claimed.unwrap_or_else(|| {
                GrpcStatus::permission_denied(format!("guard {} rejected request", index))
            }));
        }
    }
    Ok(())
}

/// Linked chain of interceptors wrapping a final delegate. Mirrors
/// `RpcControllerWrapper::execute_with_interceptors_impl` — each `Box<Self>`
/// move on `InterceptorNext::run` enforces the once-only invocation
/// contract.
async fn execute_with_interceptors<D, Fut>(
    ctx: &GrpcContext,
    interceptors: &[Arc<dyn Interceptor<GrpcContext, GrpcHandlerResult>>],
    panicked: PanicSlot,
    delegate: D,
) -> GrpcHandlerResult
where
    D: FnOnce() -> Fut + Send + 'static,
    Fut: Future<Output = ()> + Send + 'static,
{
    if interceptors.is_empty() {
        delegate().await;
        return Ok(());
    }

    let next = build_next(&interceptors[1..], panicked.clone(), delegate);
    match catch_async(
        PipelineSegment::Middleware,
        interceptors[0].intercept(ctx, next),
    )
    .await
    {
        Ok(answer) => answer,
        Err(event) => record_interceptor_panic(&panicked, event),
    }
}

fn build_next<D, Fut>(
    rest: &[Arc<dyn Interceptor<GrpcContext, GrpcHandlerResult>>],
    panicked: PanicSlot,
    delegate: D,
) -> Box<dyn InterceptorNext<GrpcContext, GrpcHandlerResult>>
where
    D: FnOnce() -> Fut + Send + 'static,
    Fut: Future<Output = ()> + Send + 'static,
{
    if rest.is_empty() {
        Box::new(LeafNext {
            delegate: Some(delegate),
        })
    } else {
        Box::new(LinkNext {
            head: rest[0].clone(),
            rest: rest[1..].to_vec(),
            panicked,
            delegate: Some(delegate),
        })
    }
}

/// Stash the event for [`run_grpc_pipeline`] to route, and answer with the
/// status it renders when nothing claims it.
fn record_interceptor_panic(
    panicked: &PanicSlot,
    event: crate::errors::PanicRecovered,
) -> GrpcHandlerResult {
    let status = GrpcStatus::new(
        crate::grpc_status::GrpcCode::Internal,
        format!("interceptor panicked: {}", event.message),
    );
    *panicked.lock().expect("interceptor panic slot poisoned") = Some(event);
    Err(status)
}

/// Innermost link: invokes the user delegate.
struct LeafNext<D> {
    delegate: Option<D>,
}

#[async_trait]
impl<D, Fut> InterceptorNext<GrpcContext, GrpcHandlerResult> for LeafNext<D>
where
    D: FnOnce() -> Fut + Send + 'static,
    Fut: Future<Output = ()> + Send + 'static,
{
    async fn run(mut self: Box<Self>, _ctx: &GrpcContext) -> GrpcHandlerResult {
        if let Some(delegate) = self.delegate.take() {
            delegate().await;
        }
        Ok(())
    }
}

/// Outer link: hands off to the next interceptor in line.
struct LinkNext<D> {
    head: Arc<dyn Interceptor<GrpcContext, GrpcHandlerResult>>,
    rest: Vec<Arc<dyn Interceptor<GrpcContext, GrpcHandlerResult>>>,
    panicked: PanicSlot,
    delegate: Option<D>,
}

#[async_trait]
impl<D, Fut> InterceptorNext<GrpcContext, GrpcHandlerResult> for LinkNext<D>
where
    D: FnOnce() -> Fut + Send + 'static,
    Fut: Future<Output = ()> + Send + 'static,
{
    async fn run(mut self: Box<Self>, ctx: &GrpcContext) -> GrpcHandlerResult {
        match self.delegate.take() {
            Some(delegate) => {
                let next = build_next(&self.rest, self.panicked.clone(), delegate);
                match catch_async(PipelineSegment::Middleware, self.head.intercept(ctx, next)).await
                {
                    Ok(answer) => answer,
                    Err(event) => record_interceptor_panic(&self.panicked, event),
                }
            }
            None => Ok(()),
        }
    }
}

async fn resolve_guards(
    entries: &[GrpcGuardEntry],
    ctx: &GrpcContext,
) -> Vec<Arc<dyn Guard<GrpcContext>>> {
    let mut out = Vec::with_capacity(entries.len());
    for entry in entries {
        let g = match entry {
            GrpcGuardEntry::Ready(g) => g.clone(),
            GrpcGuardEntry::Factory(f) => f.create(ctx).await,
        };
        out.push(g);
    }
    out
}

async fn resolve_interceptors(
    entries: &[GrpcInterceptorEntry],
    ctx: &GrpcContext,
) -> Vec<Arc<dyn Interceptor<GrpcContext, GrpcHandlerResult>>> {
    let mut out = Vec::with_capacity(entries.len());
    for entry in entries {
        let i = match entry {
            GrpcInterceptorEntry::Ready(i) => i.clone(),
            GrpcInterceptorEntry::Factory(f) => f.create(ctx).await,
        };
        out.push(i);
    }
    out
}

/// Run the error-handler chain for one gRPC call.
///
/// Walks service- + method-level error handlers in **reverse**
/// registration order. The first handler that returns `Some(GrpcStatus)`
/// claims the response; the macro maps that to `tonic::Status` at the wire
/// boundary. `None` from every handler means no rewrite — the caller keeps
/// the original status.
///
/// The same chain handles two distinct sources: a user-returned
/// `Err(Status)` (wrapped as `GrpcStatus` by the macro before being
/// passed here) and a caught handler panic, where `err` is a
/// `PanicRecovered` event a `#[catch]` handler can downcast.
pub async fn run_grpc_error_chain(
    ctx: &GrpcContext,
    enhancers: &ResolvedGrpcEnhancers,
    method: &str,
    err: &(dyn std::error::Error + Send + Sync + 'static),
) -> Option<crate::grpc_status::GrpcStatus> {
    let mut all = enhancers.error_handlers.clone();
    if let Some(per_method) = enhancers.handler_error_handlers.get(method) {
        all.extend_from_slice(per_method);
    }
    for (position, handler) in all.iter().rev().enumerate() {
        // Wrap the chain handler so a panicking `handle_error` doesn't
        // kill the rest of the chain (and lose the original error).
        // Policy: log the panic, treat it as a `None` claim, move on to
        // the next handler.
        let outcome = catch_async(
            PipelineSegment::ErrorHandler,
            handler.handle_error(err, ctx),
        )
        .await;
        match outcome {
            Ok(Some(claimed)) => return Some(claimed),
            Ok(None) => continue,
            Err(panic_event) => {
                tracing::error!(chain_position = position, error = %err, panic = %panic_event.message, "error handler panicked; trying the next one");
            }
        }
    }
    None
}

/// Carries a domain error through a `tonic::Status`'s source slot.
///
/// That slot is typed `dyn std::error::Error`, which drops the `Send + Sync`
/// the error chain needs. Wrapping the error in a concrete type is what lets a
/// downcast on the way out recover the bound.
#[derive(Debug)]
pub struct GrpcFailure(Arc<dyn crate::errors::Error>);

impl GrpcFailure {
    pub fn new(error: Arc<dyn crate::errors::Error>) -> Self {
        Self(error)
    }

    /// The error a status carries, read off the source slot.
    ///
    /// Called by the `#[grpc_methods]` wrapper on its way to the error chain,
    /// so a `#[catch(MyError)]` handler matches on this transport as it does on
    /// the other three.
    pub fn recover(
        source: Option<&(dyn std::error::Error + 'static)>,
    ) -> Option<Arc<dyn crate::errors::Error>> {
        source?.downcast_ref::<Self>().map(|f| f.0.clone())
    }
}

impl std::fmt::Display for GrpcFailure {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        self.0.fmt(f)
    }
}

impl std::error::Error for GrpcFailure {}

/// Wrap a future in `AssertUnwindSafe(...).catch_unwind()` and surface
/// the panic payload as a [`PanicRecovered`](crate::errors::PanicRecovered)
/// event scoped to the
/// `HandlerBody` segment. Used by the macro around the user delegation
/// inside a `#[grpc_methods]` proto method.
///
/// Thin wrapper around [`crate::panic_recovery::catch_async`] so the
/// macro can keep a stable, transport-specific entry point even as the
/// shared helper evolves.
pub async fn catch_handler_panic<Fut, T>(fut: Fut) -> Result<T, crate::errors::PanicRecovered>
where
    Fut: Future<Output = T>,
{
    crate::panic_recovery::catch_async(crate::errors::PipelineSegment::HandlerBody, fut).await
}

/// Empty bundle helper — used by the gRPC adapter when a service hasn't
/// been resolved through the framework (e.g. wired directly via
/// `add_service` on the adapter), and by tests.
#[doc(hidden)]
pub fn empty_enhancers() -> Arc<ResolvedGrpcEnhancers> {
    Arc::new(ResolvedGrpcEnhancers {
        guards: Vec::new(),
        handler_guards: HashMap::new(),
        interceptors: Vec::new(),
        handler_interceptors: HashMap::new(),
        error_handlers: Vec::new(),
        handler_error_handlers: HashMap::new(),
    })
}

/// Delegates to a streaming reply while owning the execution's context — cache,
/// extensions and cancellation token stay alive until the last item.
///
/// The `#[grpc_methods]` wrapper declares this as its associated stream type,
/// so it is the reply tonic serves. `Pin<Box<_>>` rather than a pin projection:
/// the inner stream is the user's associated type and carries no `Unpin` bound.
pub struct ScopedGrpcStream<S> {
    inner: std::pin::Pin<Box<S>>,
    context: GrpcContext,
    /// Set once the inner stream answers `None`. An item carrying a `Status`
    /// does not set it: tonic ends the call there and drops this un-drained, so
    /// the producer behind an abnormal end hears the token too.
    drained: bool,
}

impl<S> ScopedGrpcStream<S> {
    pub fn new(inner: S, context: GrpcContext) -> Self {
        Self {
            inner: Box::pin(inner),
            context,
            drained: false,
        }
    }
}

impl<S: futures::Stream> futures::Stream for ScopedGrpcStream<S> {
    type Item = S::Item;

    fn poll_next(
        self: std::pin::Pin<&mut Self>,
        cx: &mut std::task::Context<'_>,
    ) -> std::task::Poll<Option<Self::Item>> {
        let this = self.get_mut();
        let polled = this.inner.as_mut().poll_next(cx);
        if matches!(polled, std::task::Poll::Ready(None)) {
            this.drained = true;
        }
        polled
    }

    fn size_hint(&self) -> (usize, Option<usize>) {
        self.inner.size_hint()
    }
}

/// A stream dropped with items still to come is the caller having gone — a reset
/// stream, a dead connection, or the drain deadline dropping the server. The
/// handler returned when it had a stream, so whatever feeds that stream is not
/// inside a future tonic drops.
impl<S> Drop for ScopedGrpcStream<S> {
    fn drop(&mut self) {
        if !self.drained {
            use crate::context::HandlerContext as _;
            self.context.cancellation().cancel();
        }
    }
}

/// Carries a reply from the type the user's method produced to the type the
/// generated wrapper's signature declares.
///
/// The wrapper cannot know per method whether it is re-typing a stream or
/// passing a message through, and the answer is decided by the target type
/// alone: a message resolves to the identity impl, a stream whose associated
/// type the wrapper rewrote resolves to the wrapping one. The two never
/// overlap, since that would need `S == ScopedGrpcStream<S>`.
#[doc(hidden)]
pub trait IntoScoped<Out> {
    fn into_scoped(self, context: GrpcContext) -> Out;
}

impl<T> IntoScoped<T> for T {
    fn into_scoped(self, _context: GrpcContext) -> T {
        self
    }
}

impl<S: futures::Stream> IntoScoped<ScopedGrpcStream<S>> for S {
    fn into_scoped(self, context: GrpcContext) -> ScopedGrpcStream<S> {
        ScopedGrpcStream::new(self, context)
    }
}
