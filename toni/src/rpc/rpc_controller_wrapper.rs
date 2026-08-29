use std::collections::HashMap;
use std::sync::Arc;

use async_trait::async_trait;

use crate::context::Metadata;
use crate::context::RpcContext;
use crate::errors::{PanicRecovered, PipelineSegment};
use crate::http_helpers::ExecutionResult;
use crate::traits_helpers::{
    ErrorObserver, Guard, Interceptor, InterceptorNext, RpcErrorHandlerArc, RpcGuardEntry,
    RpcInterceptorEntry,
};

use super::{
    RpcCallInfo, RpcControllerSource, RpcData, RpcError, RpcHandlerOutput, RpcHandlerResult,
};
use futures::stream::BoxStream;
use futures::{FutureExt, StreamExt};
use std::panic::AssertUnwindSafe;

/// Delegates to the handler's reply stream while owning the execution's
/// context — cache, extensions, and token stay alive until the last item.
///
/// `BoxStream` is `Pin<Box<_>>` and therefore `Unpin`, so the projection
/// needs no pin machinery.
struct ScopedRpcStream {
    inner: BoxStream<'static, Result<RpcData, RpcError>>,
    context: RpcContext,
    /// Set once the inner stream answers `None`. An error item does not set
    /// it: the adapter stops the drain there and drops this un-drained, so
    /// the producer behind an abnormal end hears the token too.
    drained: bool,
}

impl futures::Stream for ScopedRpcStream {
    type Item = Result<RpcData, RpcError>;

    fn poll_next(
        self: std::pin::Pin<&mut Self>,
        cx: &mut std::task::Context<'_>,
    ) -> std::task::Poll<Option<Self::Item>> {
        let this = self.get_mut();
        let polled = std::pin::Pin::new(&mut this.inner).poll_next(cx);
        if matches!(polled, std::task::Poll::Ready(None)) {
            this.drained = true;
        }
        polled
    }

    fn size_hint(&self) -> (usize, Option<usize>) {
        self.inner.size_hint()
    }
}

/// A stream dropped with items still to come is the caller having gone —
/// disconnect, cancel notice, or shutdown. Nothing else observes that: the
/// handler returned when it had a stream, and whatever feeds it is not inside
/// the future the adapter drops.
impl Drop for ScopedRpcStream {
    fn drop(&mut self) {
        if !self.drained {
            use crate::context::HandlerContext as _;
            self.context.cancellation().cancel();
        }
    }
}

struct RpcChainNext {
    interceptors: Vec<Arc<dyn Interceptor<RpcContext, RpcHandlerResult>>>,
    source: Arc<dyn RpcControllerSource>,
    error_handlers: Vec<RpcErrorHandlerArc>,
    observers: Vec<Arc<dyn ErrorObserver>>,
}

#[async_trait]
impl InterceptorNext<RpcContext, RpcHandlerResult> for RpcChainNext {
    async fn run(self: Box<Self>, context: &RpcContext) -> RpcHandlerResult {
        RpcControllerWrapper::execute_with_interceptors(
            context,
            &self.interceptors,
            &self.source,
            &self.error_handlers,
            &self.observers,
        )
        .await
    }
}

/// Wraps an [`RpcControllerSource`] with the full guard/interceptor pipeline.
pub struct RpcControllerWrapper {
    source: Arc<dyn RpcControllerSource>,
    guards: Vec<RpcGuardEntry>,
    interceptors: Vec<RpcInterceptorEntry>,
    error_handlers: Vec<RpcErrorHandlerArc>,
    error_observers: Vec<Arc<dyn ErrorObserver>>,
    metadata: Arc<Metadata>,
    /// Per-pattern metadata, already merged over `metadata` at expansion. A pattern absent
    /// here declared nothing of its own and reads the controller's.
    handler_metadata: HashMap<String, Arc<Metadata>>,
    handler_guards: HashMap<String, Vec<RpcGuardEntry>>,
    handler_interceptors: HashMap<String, Vec<RpcInterceptorEntry>>,
    handler_error_handlers: HashMap<String, Vec<RpcErrorHandlerArc>>,
}

impl RpcControllerWrapper {
    pub fn new(
        source: Arc<dyn RpcControllerSource>,
        guards: Vec<RpcGuardEntry>,
        interceptors: Vec<RpcInterceptorEntry>,
        error_handlers: Vec<RpcErrorHandlerArc>,
        error_observers: Vec<Arc<dyn ErrorObserver>>,
        metadata: Arc<Metadata>,
        handler_metadata: HashMap<String, Arc<Metadata>>,
        handler_guards: HashMap<String, Vec<RpcGuardEntry>>,
        handler_interceptors: HashMap<String, Vec<RpcInterceptorEntry>>,
        handler_error_handlers: HashMap<String, Vec<RpcErrorHandlerArc>>,
    ) -> Self {
        Self {
            source,
            guards,
            interceptors,
            error_handlers,
            error_observers,
            metadata,
            handler_metadata,
            handler_guards,
            handler_interceptors,
            handler_error_handlers,
        }
    }

    pub fn get_patterns(&self) -> Vec<String> {
        self.source.get_patterns()
    }

    pub async fn handle_message(&self, data: RpcData, info: RpcCallInfo) -> RpcHandlerResult {
        let RpcCallInfo {
            pattern,
            headers,
            extensions,
        } = info;
        let ctx = RpcContext::with_extensions(
            pattern.clone(),
            data,
            headers,
            Some(
                self.handler_metadata
                    .get(&pattern)
                    .unwrap_or(&self.metadata)
                    .clone(),
            ),
            extensions,
        );

        let mut all_guards = self.guards.clone();
        if let Some(h) = self.handler_guards.get(&pattern) {
            all_guards.extend_from_slice(h);
        }
        let mut all_interceptors = self.interceptors.clone();
        if let Some(h) = self.handler_interceptors.get(&pattern) {
            all_interceptors.extend_from_slice(h);
        }
        let mut all_error_handlers = self.error_handlers.clone();
        if let Some(h) = self.handler_error_handlers.get(&pattern) {
            all_error_handlers.extend_from_slice(h);
        }
        let observers = self.error_observers.clone();

        let guards = Self::resolve_guards(&all_guards, &ctx).await;
        for (index, guard) in guards.iter().enumerate() {
            // Treat a panic in `can_activate` as a hard rejection: observers
            // see `PanicRecovered { during: Guard }` and the caller gets
            // `RpcError::Forbidden` instead of a torn-down dispatcher.
            let activated = match crate::panic_recovery::catch_async(
                crate::errors::PipelineSegment::Guard,
                guard.can_activate(&ctx),
            )
            .await
            {
                Ok(b) => b,
                Err(event) => {
                    Self::fan_out_observers(&observers, &event, &ctx).await;
                    return Err(RpcError::Forbidden(format!(
                        "guard {} panicked: {}",
                        index, event.message
                    )));
                }
            };
            if !activated {
                let err = RpcError::Forbidden("Guard rejected message".into());
                Self::fan_out_observers(&observers, &err, &ctx).await;
                return Err(err);
            }
        }

        let interceptors = Self::resolve_interceptors(&all_interceptors, &ctx).await;
        let answer = Self::execute_with_interceptors(
            &ctx,
            &interceptors,
            &self.source,
            &all_error_handlers,
            &observers,
        )
        .await;

        // The execution ends when the answer does. A stream has emitted nothing
        // at this point, so the context rides it rather than dying here.
        match answer {
            Ok(RpcHandlerOutput::Stream(stream)) => Ok(RpcHandlerOutput::Stream(
                ScopedRpcStream {
                    inner: stream,
                    context: ctx,
                    drained: false,
                }
                .boxed(),
            )),
            other => other,
        }
    }

    /// Run one chain handler with panic recovery: a panicking
    /// `handle_error` fans `PanicRecovered { during: ErrorHandler }` to
    /// observers and returns `None` so the caller continues to the next
    /// handler. Without this, a single bad chain handler would kill the
    /// whole error-recovery path and the original error would never
    /// reach the fallback `to_data` rendering.
    /// Drive `RpcError::to_data` with panic recovery — a panic in the
    /// renderer is the last thing the framework can do for the caller,
    /// so we substitute a hardcoded `Internal` envelope and fan
    /// `PanicRecovered { during: ResponseRendering }` to observers.
    async fn safe_render<F>(
        render: F,
        observers: &[Arc<dyn ErrorObserver>],
        ctx: &RpcContext,
    ) -> RpcData
    where
        F: FnOnce() -> RpcData,
    {
        match crate::panic_recovery::catch_sync(
            crate::errors::PipelineSegment::ResponseRendering,
            render,
        ) {
            Ok(data) => data,
            Err(panic_event) => {
                Self::fan_out_observers(observers, &panic_event, ctx).await;
                Self::fallback_internal_data()
            }
        }
    }

    /// Hardcoded fallback envelope when the regular renderer panics.
    /// Uses [`RpcData::text`] so no user-supplied serialiser runs.
    fn fallback_internal_data() -> RpcData {
        RpcData::text("Internal Server Error")
    }

    async fn try_chain_handler(
        handler: &RpcErrorHandlerArc,
        error: &(dyn std::error::Error + Send + Sync + 'static),
        ctx: &RpcContext,
        observers: &[Arc<dyn ErrorObserver>],
    ) -> Option<RpcData> {
        match crate::panic_recovery::catch_async(
            crate::errors::PipelineSegment::ErrorHandler,
            handler.handle_error(error, ctx),
        )
        .await
        {
            Ok(opt) => opt,
            Err(panic_event) => {
                Self::fan_out_observers(observers, &panic_event, ctx).await;
                None
            }
        }
    }

    async fn fan_out_observers(
        observers: &[Arc<dyn ErrorObserver>],
        error: &(dyn std::error::Error + Send + Sync + 'static),
        ctx: &RpcContext,
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

    async fn resolve_guards(
        entries: &[RpcGuardEntry],
        ctx: &RpcContext,
    ) -> Vec<Arc<dyn Guard<RpcContext>>> {
        let mut out = Vec::with_capacity(entries.len());
        for entry in entries {
            let g = match entry {
                RpcGuardEntry::Ready(g) => g.clone(),
                RpcGuardEntry::Factory(f) => f.create(ctx).await,
            };
            out.push(g);
        }
        out
    }

    async fn resolve_interceptors(
        entries: &[RpcInterceptorEntry],
        ctx: &RpcContext,
    ) -> Vec<Arc<dyn Interceptor<RpcContext, RpcHandlerResult>>> {
        let mut out = Vec::with_capacity(entries.len());
        for entry in entries {
            let i = match entry {
                RpcInterceptorEntry::Ready(i) => i.clone(),
                RpcInterceptorEntry::Factory(f) => f.create(ctx).await,
            };
            out.push(i);
        }
        out
    }

    async fn execute_with_interceptors(
        context: &RpcContext,
        interceptors: &[Arc<dyn Interceptor<RpcContext, RpcHandlerResult>>],
        source: &Arc<dyn RpcControllerSource>,
        error_handlers: &[RpcErrorHandlerArc],
        observers: &[Arc<dyn ErrorObserver>],
    ) -> RpcHandlerResult {
        if interceptors.is_empty() {
            return Self::execute_handler(context, source, error_handlers, observers).await;
        }

        let (first, rest) = interceptors.split_first().unwrap();

        let next = RpcChainNext {
            interceptors: rest.to_vec(),
            source: source.clone(),
            error_handlers: error_handlers.to_vec(),
            observers: observers.to_vec(),
        };

        match crate::panic_recovery::catch_async(
            crate::errors::PipelineSegment::Middleware,
            first.intercept(context, Box::new(next)),
        )
        .await
        {
            Ok(answer) => answer,
            Err(event) => {
                Self::record_pipeline_panic(context, error_handlers, observers, event).await
            }
        }
    }

    /// Surface a panicking pre-handler segment (an interceptor)
    /// through the existing observer + chain pipeline: fan to observers,
    /// give error handlers first claim, and fall back to a wire-`Err`
    /// Internal envelope.
    async fn record_pipeline_panic(
        context: &RpcContext,
        error_handlers: &[RpcErrorHandlerArc],
        observers: &[Arc<dyn ErrorObserver>],
        event: PanicRecovered,
    ) -> RpcHandlerResult {
        Self::fan_out_observers(observers, &event, context).await;
        for handler in error_handlers.iter().rev() {
            if let Some(claimed) =
                Self::try_chain_handler(handler, &event, context, observers).await
            {
                return Ok(RpcHandlerOutput::Single(claimed));
            }
        }
        let rpc_err = RpcError::from(event);
        let data = Self::safe_render(|| rpc_err.to_data(), observers, context).await;
        Ok(RpcHandlerOutput::Single(data))
    }

    /// Run the handler, then route the result.
    ///
    /// `Ok` is the answer. On `Err`, observers fan out on the underlying error,
    /// the chain's most-specific handler gets first claim, and
    /// `RpcError::to_data` is the fallback envelope when none claims.
    async fn execute_handler(
        context: &RpcContext,
        source: &Arc<dyn RpcControllerSource>,
        error_handlers: &[RpcErrorHandlerArc],
        observers: &[Arc<dyn ErrorObserver>],
    ) -> RpcHandlerResult {
        // The instance is asked for here and nowhere earlier: a guard that rejects, an
        // interceptor that answers never builds a controller. Construction
        // sits inside the same `catch_unwind` as the handler body, so a panicking `#[new]` renders
        // an envelope instead of tearing down the dispatcher.
        let exec_result = AssertUnwindSafe(async {
            source.instance(context).await.handle_message(context).await
        })
        .catch_unwind()
        .await;
        let exec_result = match exec_result {
            Ok(result) => result,
            Err(payload) => {
                let event =
                    PanicRecovered::from_panic_payload(PipelineSegment::HandlerBody, payload);
                ExecutionResult::Err(RpcError::from(event))
            }
        };
        match exec_result {
            ExecutionResult::Ok(output) => Ok(output),
            ExecutionResult::Err(rpc_err) => {
                let observed_err: &(dyn std::error::Error + Send + Sync + 'static) = match &rpc_err
                {
                    RpcError::AppError(e) => e.as_ref(),
                    other => other,
                };
                Self::fan_out_observers(observers, observed_err, context).await;
                for handler in error_handlers.iter().rev() {
                    if let Some(claimed) =
                        Self::try_chain_handler(handler, observed_err, context, observers).await
                    {
                        return Ok(RpcHandlerOutput::Single(claimed));
                    }
                }
                let data = Self::safe_render(|| rpc_err.to_data(), observers, context).await;
                Ok(RpcHandlerOutput::Single(data))
            }
        }
    }
}
