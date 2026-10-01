//! The dispatch pipeline for one call (§7), shared by every transport:
//!
//! 1. The transport opens an `Execution` in the handler's module, seeds the inputs, and builds
//!    its `Cx` around a handle to it.
//! 2. Guards, global then controller then method: the global ones read through
//!    `Resolver::entries::<AnyGuard<T>>()`, each obtained only once the one before it admitted
//!    (the shared value, the singleton, a per-execution build, or the closure call), then
//!    `can_activate`. On a refusal, later guards are never built.
//! 3. Once every guard admits: the interceptors, then the controller, then the handler inside
//!    the interceptor chain. The handler closure resolves the controller itself; a singleton or
//!    per-execution controller is already built by then, so a failure to build it reaches the
//!    error handlers without passing through the interceptors.
//! 4. Errors go through the error handlers, method first, then controller, then global, each
//!    tier last-declared first. A refusal is one of them: `Ok(false)` becomes
//!    `GuardRejected { guard }`. An error no handler claims is returned for the transport to
//!    render (its forbidden status for `GuardRejected`).
//! 5. The transport drops its `Cx` when the reply is written; a streaming reply holds a clone.

use std::any::type_name;
use std::future::Future;
use std::sync::Arc;

use crate::binding::downcast_instance;
use crate::error::{GuardRejected, LookupError, LookupKind};
use crate::execution::ExecutionRef;
use crate::graph::{Effective, Graph, ModuleId, Visible};
use crate::key::BindingKind;
use crate::resolver::Resolver;
use crate::timer::{BoxError, BoxFuture};
use crate::transport::controller::MountedHandler;
use crate::transport::enhancer::Decl;
use crate::transport::next::Next;
use crate::transport::{AnyErrorHandler, AnyGuard, AnyInterceptor, ErasedErrorHandler, ErasedGuard, Transport};

/// Runs `handler`'s enhancer stack around `call` for one call. `exec` is the call's execution,
/// the one `cx` wraps; `call` receives a clone of `cx` and answers the handler's reply.
///
/// Every declaration resolves with the controller module's visibility, inside `exec`, so a
/// per-execution enhancer is built once per call and shared with the handler's own dependencies.
pub async fn dispatch<T, F, Fut>(handler: &MountedHandler<T>, exec: &ExecutionRef, cx: &T::Cx, call: F) -> Result<T::Reply, BoxError>
where
    T: Transport,
    F: FnOnce(T::Cx) -> Fut + Send,
    Fut: Future<Output = Result<T::Reply, BoxError>> + Send,
{
    let graph = handler.module.app.graph();
    let module = handler.module.module;
    let resolver = exec.resolver().in_module(module);
    let at = Lookup { graph: &graph, module, resolver: &resolver };
    let outcome = match admit(handler, &at, cx).await {
        Ok(()) => respond(handler, &at, cx, call).await,
        Err(err) => Err(err),
    };
    match outcome {
        Ok(reply) => Ok(reply),
        Err(err) => recover(handler, &at, cx, err).await,
    }
}

/// Where a declaration resolves: the controller's module, inside the call's execution.
struct Lookup<'s> {
    graph: &'s Graph,
    module: ModuleId,
    resolver: &'s Resolver<'s>,
}

async fn admit<T: Transport>(handler: &MountedHandler<T>, at: &Lookup<'_>, cx: &T::Cx) -> Result<(), BoxError> {
    for entry in at.resolver.entries::<AnyGuard<T>>()? {
        let guard = entry.resolve().await?;
        check(&*guard, cx).await?;
    }
    for decl in handler.controller_spec.guards.iter().chain(&handler.method_spec.guards) {
        let guard = obtain(decl, at).await?;
        check(&*guard, cx).await?;
    }
    Ok(())
}

async fn check<T: Transport>(guard: &AnyGuard<T>, cx: &T::Cx) -> Result<(), BoxError> {
    if <AnyGuard<T> as ErasedGuard<T>>::can_activate(guard, cx).await? {
        Ok(())
    } else {
        Err(BoxError::from(GuardRejected::new(guard.name())))
    }
}

async fn respond<T, F, Fut>(handler: &MountedHandler<T>, at: &Lookup<'_>, cx: &T::Cx, call: F) -> Result<T::Reply, BoxError>
where
    T: Transport,
    F: FnOnce(T::Cx) -> Fut + Send,
    Fut: Future<Output = Result<T::Reply, BoxError>> + Send,
{
    let mut interceptors: Vec<Arc<AnyInterceptor<T>>> = Vec::new();
    for entry in at.resolver.entries::<AnyInterceptor<T>>()? {
        interceptors.push(entry.resolve().await?);
    }
    for decl in handler.controller_spec.interceptors.iter().chain(&handler.method_spec.interceptors) {
        interceptors.push(obtain(decl, at).await?);
    }
    build_controller(handler, at).await?;
    chain(cx, &interceptors, call).await
}

/// Builds the controller into the execution ahead of the chain, so the handler closure's own
/// lookup reads the stored singleton or the execution's cached instance. A transient
/// controller is left to that lookup: building one here would build a second.
async fn build_controller<T: Transport>(handler: &MountedHandler<T>, at: &Lookup<'_>) -> Result<(), LookupError> {
    if let Some(&Visible::Binding(id)) = at.graph.lookup(at.module, handler.controller.key()) {
        if at.graph.binding(id).effective != Effective::Transient {
            at.resolver.instance(id).await?;
        }
    }
    Ok(())
}

fn chain<'a, T, F, Fut>(cx: &'a T::Cx, interceptors: &'a [Arc<AnyInterceptor<T>>], call: F) -> BoxFuture<'a, Result<T::Reply, BoxError>>
where
    T: Transport,
    F: FnOnce(T::Cx) -> Fut + Send + 'a,
    Fut: Future<Output = Result<T::Reply, BoxError>> + Send + 'a,
{
    let handler: Box<dyn FnOnce(&'a T::Cx) -> BoxFuture<'a, Result<T::Reply, BoxError>> + Send + 'a> =
        Box::new(move |cx: &'a T::Cx| -> BoxFuture<'a, Result<T::Reply, BoxError>> { Box::pin(call(cx.clone())) });
    Next { cx, rest: interceptors, handler }.run()
}

/// Offers `err` to each error handler in turn. `Ok` claims it; `Err` hands the next handler the
/// error returned. A handler that cannot be built hands on its own `LookupError` the same way.
async fn recover<T: Transport>(handler: &MountedHandler<T>, at: &Lookup<'_>, cx: &T::Cx, mut err: BoxError) -> Result<T::Reply, BoxError> {
    let declared = handler.method_spec.error_handlers.iter().rev().chain(handler.controller_spec.error_handlers.iter().rev());
    for decl in declared {
        err = match obtain(decl, at).await {
            Ok(eh) => match offer(&*eh, err, cx).await {
                Ok(reply) => return Ok(reply),
                Err(next) => next,
            },
            Err(failed) => BoxError::from(failed),
        };
    }
    let global: Vec<_> = match at.resolver.entries::<AnyErrorHandler<T>>() {
        Ok(entries) => entries.collect(),
        Err(failed) => return Err(BoxError::from(failed)),
    };
    for entry in global.iter().rev() {
        err = match entry.resolve().await {
            Ok(eh) => match offer(&*eh, err, cx).await {
                Ok(reply) => return Ok(reply),
                Err(next) => next,
            },
            Err(failed) => BoxError::from(failed),
        };
    }
    Err(err)
}

fn offer<'a, T: Transport>(eh: &'a AnyErrorHandler<T>, err: BoxError, cx: &'a T::Cx) -> BoxFuture<'a, Result<T::Reply, BoxError>> {
    <AnyErrorHandler<T> as ErasedErrorHandler<T>>::handle(eh, err, cx)
}

/// One controller- or method-level declaration as an instance of its role.
async fn obtain<R: ?Sized + Send + Sync + 'static>(decl: &Decl<R>, at: &Lookup<'_>) -> Result<Arc<R>, LookupError> {
    match decl {
        Decl::Value(value) => Ok(Arc::clone(value)),
        Decl::Closure(closure) => (closure.build)(at.resolver).await,
        Decl::Type { key, coerce } => {
            let name = key.name(BindingKind::Single);
            // Wiring has refused a by-type enhancer the controller's module cannot see, so a
            // miss here means the graph and the declaration disagree.
            let Some(&Visible::Binding(id)) = at.graph.lookup(at.module, *key) else {
                return Err(LookupError::NotFound { key: name, kind: LookupKind::Binding });
            };
            let instance = at.resolver.instance(id).await?;
            downcast_instance::<R>(&coerce(&instance)).ok_or(LookupError::WrongType { key: name, requested: type_name::<R>() })
        }
    }
}
