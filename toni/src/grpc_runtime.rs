//! Per-call helpers invoked from `#[grpc_methods]`-generated code.
//!
//! Lives here so the chain logic is tonic-free, unit-testable, and shared
//! across every gRPC service the macro emits. The macro generates a thin
//! per-method shim that builds a [`GrpcContext`], calls into this module,
//! maps any [`GrpcStatus`] back to `tonic::Status`, then either returns or
//! delegates to the user's body.

use std::collections::HashMap;
use std::panic::AssertUnwindSafe;
use std::sync::Arc;

use futures::FutureExt;

use crate::adapter::ResolvedGrpcEnhancers;
use crate::context::{GrpcContext, HandlerContext};
use crate::errors::GuardRejection;
use crate::grpc_status::GrpcStatus;
use crate::traits_helpers::{ErrorObserver, Guard, GrpcGuardEntry};

/// Run the resolved guard chain for one gRPC call.
///
/// Returns `Ok(())` when every guard accepts. On the first rejection (or
/// `ctx.abort()`) emits a [`GuardRejection`] event to all observers and
/// returns the corresponding [`GrpcStatus::permission_denied`] — the
/// caller (macro-generated) maps that into `tonic::Status` at the wire
/// boundary.
pub async fn run_grpc_guards(
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

/// Empty bundle helper — used by the gRPC adapter when a service hasn't
/// been resolved through the framework (e.g. wired directly via
/// `add_service` on the adapter), and by tests.
#[doc(hidden)]
pub fn empty_enhancers() -> Arc<ResolvedGrpcEnhancers> {
    Arc::new(ResolvedGrpcEnhancers {
        guards: Vec::new(),
        handler_guards: HashMap::new(),
        error_observers: Vec::new(),
    })
}
