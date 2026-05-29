//! Per-call helpers invoked from `#[grpc_methods]`-generated code.
//!
//! Lives here so the chain logic is tonic-free, unit-testable, and shared
//! across every gRPC service the macro emits. The macro generates a thin
//! per-method shim that builds a [`GrpcContext`], calls into this module,
//! maps any [`GrpcStatus`] back to `tonic::Status`, then either returns or
//! delegates to the user's body.

use std::collections::HashMap;
use std::future::Future;
use std::panic::AssertUnwindSafe;
use std::sync::Arc;

use async_trait::async_trait;
use futures::FutureExt;

use crate::adapter::ResolvedGrpcEnhancers;
use crate::context::{GrpcContext, HandlerContext};
use crate::errors::GuardRejection;
use crate::grpc_status::GrpcStatus;
use crate::traits_helpers::{
    ErrorObserver, GrpcGuardEntry, GrpcInterceptorEntry, Guard, Interceptor, InterceptorNext,
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
/// Returns `Err(GrpcStatus)` when a guard rejects, an interceptor sets
/// `ctx.set_response(Err(...))`, or an interceptor short-circuits without
/// calling `next.run(ctx)`. The macro maps `GrpcStatus` to `tonic::Status`
/// at the wire boundary; `Ok(())` means the chain completed normally and
/// the user's delegate (which fills the side-channel) was reached.
pub async fn run_grpc_pipeline<D, Fut>(
    ctx: &mut GrpcContext,
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
    let interceptors = resolve_interceptors(&all_interceptors).await;

    execute_with_interceptors(ctx, &interceptors, delegate).await;

    if let Some(short_circuit) = ctx.take_response() {
        return short_circuit;
    }
    Ok(())
}

/// Guards-only entry point — same shape as PR #1 shipped, retained for
/// services that declare no interceptors so the macro can skip the
/// closure-boxing cost.
pub async fn run_grpc_guards(
    ctx: &mut GrpcContext,
    enhancers: &ResolvedGrpcEnhancers,
    method: &str,
) -> Result<(), GrpcStatus> {
    run_grpc_guards_inline(ctx, enhancers, method).await
}

async fn run_grpc_guards_inline(
    ctx: &mut GrpcContext,
    enhancers: &ResolvedGrpcEnhancers,
    method: &str,
) -> Result<(), GrpcStatus> {
    let mut all_guards = enhancers.guards.clone();
    if let Some(per_method) = enhancers.handler_guards.get(method) {
        all_guards.extend_from_slice(per_method);
    }

    let guards = resolve_guards(&all_guards).await;
    for (index, guard) in guards.iter().enumerate() {
        if !guard.can_activate(ctx).await {
            let event = GuardRejection::new(index);
            fan_out_observers(&enhancers.error_observers, &event, ctx).await;
            return Err(GrpcStatus::permission_denied(format!(
                "guard {} rejected request",
                index
            )));
        }
        if ctx.should_abort() {
            let event =
                GuardRejection::with_reason(index, "request aborted by guard");
            fan_out_observers(&enhancers.error_observers, &event, ctx).await;
            return Err(GrpcStatus::permission_denied(
                "request aborted by guard",
            ));
        }
    }
    Ok(())
}

/// Linked chain of interceptors wrapping a final delegate. Mirrors
/// `RpcControllerWrapper::execute_with_interceptors_impl` — each `Box<Self>`
/// move on `InterceptorNext::run` enforces the once-only invocation
/// contract.
async fn execute_with_interceptors<D, Fut>(
    ctx: &mut GrpcContext,
    interceptors: &[Arc<dyn Interceptor<GrpcContext>>],
    delegate: D,
) where
    D: FnOnce() -> Fut + Send + 'static,
    Fut: Future<Output = ()> + Send + 'static,
{
    if interceptors.is_empty() {
        delegate().await;
        return;
    }

    let next = build_next(&interceptors[1..], delegate);
    interceptors[0].intercept(ctx, next).await;
}

fn build_next<D, Fut>(
    rest: &[Arc<dyn Interceptor<GrpcContext>>],
    delegate: D,
) -> Box<dyn InterceptorNext<GrpcContext>>
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
            delegate: Some(delegate),
        })
    }
}

/// Innermost link: invokes the user delegate.
struct LeafNext<D> {
    delegate: Option<D>,
}

#[async_trait]
impl<D, Fut> InterceptorNext<GrpcContext> for LeafNext<D>
where
    D: FnOnce() -> Fut + Send + 'static,
    Fut: Future<Output = ()> + Send + 'static,
{
    async fn run(mut self: Box<Self>, _ctx: &mut GrpcContext) {
        if let Some(delegate) = self.delegate.take() {
            delegate().await;
        }
    }
}

/// Outer link: hands off to the next interceptor in line.
struct LinkNext<D> {
    head: Arc<dyn Interceptor<GrpcContext>>,
    rest: Vec<Arc<dyn Interceptor<GrpcContext>>>,
    delegate: Option<D>,
}

#[async_trait]
impl<D, Fut> InterceptorNext<GrpcContext> for LinkNext<D>
where
    D: FnOnce() -> Fut + Send + 'static,
    Fut: Future<Output = ()> + Send + 'static,
{
    async fn run(mut self: Box<Self>, ctx: &mut GrpcContext) {
        if let Some(delegate) = self.delegate.take() {
            let next = build_next(&self.rest, delegate);
            self.head.intercept(ctx, next).await;
        }
    }
}

/// gRPC has no HTTP request; factory entries are called with `None`.
/// Factory guards with `requires_http_parts() == true` are rejected at
/// startup by [`GrpcServiceResolver`](crate::injector::GrpcServiceResolver).
async fn resolve_guards(entries: &[GrpcGuardEntry]) -> Vec<Arc<dyn Guard<GrpcContext>>> {
    let mut out = Vec::with_capacity(entries.len());
    for entry in entries {
        let g = match entry {
            GrpcGuardEntry::Ready(g) => g.clone(),
            GrpcGuardEntry::Factory(f) => f.create(None).await,
        };
        out.push(g);
    }
    out
}

async fn resolve_interceptors(
    entries: &[GrpcInterceptorEntry],
) -> Vec<Arc<dyn Interceptor<GrpcContext>>> {
    let mut out = Vec::with_capacity(entries.len());
    for entry in entries {
        let i = match entry {
            GrpcInterceptorEntry::Ready(i) => i.clone(),
            GrpcInterceptorEntry::Factory(f) => f.create(None).await,
        };
        out.push(i);
    }
    out
}

async fn fan_out_observers(
    observers: &[Arc<dyn ErrorObserver>],
    error: &(dyn std::error::Error + Send + Sync + 'static),
    ctx: &GrpcContext,
) {
    for observer in observers {
        let observe = AssertUnwindSafe(observer.observe(error, ctx));
        if let Err(payload) = observe.catch_unwind().await {
            let msg = if let Some(s) = payload.downcast_ref::<&'static str>() {
                *s
            } else if let Some(s) = payload.downcast_ref::<String>() {
                s.as_str()
            } else {
                "<panic payload was not a string>"
            };
            tracing::error!(error = %error, panic = %msg, "error observer panicked");
        }
    }
}

/// Run the error-handler chain for one gRPC call.
///
/// Fans `err` to every observer, then walks service- + method-level
/// error handlers in **reverse** registration order. The first handler
/// that returns `Some(GrpcStatus)` claims the response; the macro maps
/// that to `tonic::Status` at the wire boundary. `None` from every
/// handler means no rewrite — the caller keeps the original status.
///
/// The same chain handles two distinct sources: a user-returned
/// `Err(Status)` (wrapped as `GrpcStatus` by the macro before being
/// passed here) and a caught handler panic (where `err` is a
/// `PanicRecovered` event so observers see the typed framework signal).
pub async fn run_grpc_error_chain(
    ctx: &mut GrpcContext,
    enhancers: &ResolvedGrpcEnhancers,
    method: &str,
    err: &(dyn std::error::Error + Send + Sync + 'static),
) -> Option<crate::grpc_status::GrpcStatus> {
    fan_out_observers(&enhancers.error_observers, err, ctx).await;

    let mut all = enhancers.error_handlers.clone();
    if let Some(per_method) = enhancers.handler_error_handlers.get(method) {
        all.extend_from_slice(per_method);
    }
    for handler in all.iter().rev() {
        if let Some(claimed) = handler.handle_error(err, ctx).await {
            return Some(claimed);
        }
    }
    None
}

/// Wrap a future in `AssertUnwindSafe(...).catch_unwind()` and surface
/// the panic payload as a [`PanicRecovered`] event scoped to the
/// `HandlerBody` segment. Used by the macro around the user delegation
/// inside a `#[grpc_methods]` proto method.
pub async fn catch_handler_panic<Fut, T>(
    fut: Fut,
) -> Result<T, crate::errors::PanicRecovered>
where
    Fut: Future<Output = T>,
{
    match AssertUnwindSafe(fut).catch_unwind().await {
        Ok(v) => Ok(v),
        Err(payload) => Err(crate::errors::PanicRecovered::from_panic_payload(
            crate::errors::PipelineSegment::HandlerBody,
            payload,
        )),
    }
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
        error_observers: Vec::new(),
    })
}
