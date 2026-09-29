use crate::dispatch::Items;
use std::collections::HashMap;
use std::sync::Arc;

use async_trait::async_trait;

use super::{RpcCallInfo, RpcControllerSource, RpcData, RpcError, RpcHandlerResult};
use crate::context::Metadata;
use crate::dispatch::ExecutionResult;
use crate::dispatch::resolve::Resolved;
use crate::dispatch::transport::Rpc;
use crate::enhancer::Interceptor;
use crate::enhancer::pipeline::{GuardFailure, Leaf, through_interceptors};
use crate::rpc::RpcContext;
use futures::StreamExt;

/// The innermost step of the chain: the controller, resolved for this call, asked to handle it.
///
/// Resolution sits inside the panic recovery with the handler, so a per-call controller whose
/// constructor or startup hook panics answers the call as the handler's panic would.
struct ControllerLeaf(Arc<dyn RpcControllerSource>);

#[async_trait]
impl Leaf<Rpc> for ControllerLeaf {
    async fn call(&self, context: &RpcContext) -> RpcHandlerResult {
        match crate::panic_recovery::catch_async(
            crate::errors::PipelineSegment::HandlerBody,
            async {
                match self.0.resolve(context).await {
                    Ok(controller) => controller.handle_message(context).await,
                    Err(failed) => ExecutionResult::Err(RpcError::from(failed)),
                }
            },
        )
        .await
        {
            Ok(ExecutionResult::Ok(output)) => Ok(output),
            Ok(ExecutionResult::Err(rpc_err)) => Err(rpc_err),
            Err(event) => Err(RpcError::from(event)),
        }
    }
}

/// Wraps an [`RpcControllerSource`] with the full guard/interceptor pipeline.
pub(crate) struct RpcControllerWrapper {
    source: Arc<dyn RpcControllerSource>,
    /// The controller's enhancers, each pattern's merged over them at create.
    enhancers: Resolved<Rpc>,
    metadata: Arc<Metadata>,
    /// Per-pattern metadata, already merged over `metadata` at expansion. A pattern absent
    /// here declared nothing of its own and reads the controller's.
    handler_metadata: HashMap<String, Arc<Metadata>>,
}

impl RpcControllerWrapper {
    pub(crate) fn new(
        source: Arc<dyn RpcControllerSource>,
        enhancers: Resolved<Rpc>,
        metadata: Arc<Metadata>,
        handler_metadata: HashMap<String, Arc<Metadata>>,
    ) -> Self {
        Self {
            source,
            enhancers,
            metadata,
            handler_metadata,
        }
    }

    pub(crate) fn patterns(&self) -> Vec<String> {
        self.source.patterns()
    }

    pub(crate) async fn handle_message(
        &self,
        data: RpcData,
        info: RpcCallInfo,
    ) -> RpcHandlerResult {
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

        let enhancers = self.enhancers.for_key(&pattern);
        // Guards first, one at a time, and no interceptor built until every one has passed.
        let answer =
            match crate::enhancer::pipeline::run_guards::<Rpc>(&enhancers.guards, &ctx).await {
                Ok(()) => {
                    let interceptors = crate::enhancer::pipeline::interceptors_for::<Rpc>(
                        &enhancers.interceptors,
                        &ctx,
                    )
                    .await;
                    Self::run_chain(&ctx, &self.source, &interceptors).await
                }
                Err(GuardFailure::Rejected(rejection)) => {
                    tracing::debug!(
                        guard_index = rejection.guard_index,
                        "guard rejected message"
                    );
                    Err(RpcError::from(rejection))
                }
                Err(GuardFailure::Panicked { index, event }) => {
                    tracing::debug!(guard_index = index, panic = %event.message, "guard panicked");
                    Err(RpcError::from(event))
                }
            };

        // The one place the chain runs. A guard's refusal, a panic from any segment and the
        // handler's own error all arrive as `Err`, so a `#[catch]` handler is offered every one
        // of them and an unclaimed one renders the same way whichever produced it.
        let answer = match answer {
            Ok(output) => Ok(output),
            Err(rpc_err) => {
                let observed: &(dyn std::error::Error + Send + Sync + 'static) = match &rpc_err {
                    RpcError::AppError(e) => e.as_ref(),
                    other => other,
                };
                match crate::enhancer::pipeline::claim::<Rpc>(
                    &enhancers.error_handlers,
                    observed,
                    &ctx,
                )
                .await
                {
                    // Everything this controller produces is an answer, so it renders as one. A
                    // chain handler that recovered the call answers with its reply; one that
                    // reshaped the failure, and an unclaimed failure, both answer with the
                    // canonical envelope. A wire-`err` frame is reserved for a call that reached
                    // no controller at all.
                    Some(Ok(output)) => Ok(output),
                    Some(Err(reshaped)) => Ok(Items::One(Self::safe_render(|| reshaped.to_data()))),
                    None => Ok(Items::One(Self::safe_render(|| rpc_err.to_data()))),
                }
            }
        };

        // The execution ends when the answer does. A stream has emitted nothing
        // at this point, so the context rides it rather than dying here.
        match answer {
            Ok(Items::Many(stream)) => Ok(Items::Many(
                crate::dispatch::ScopedStream::new(stream, ctx).boxed(),
            )),
            other => other,
        }
    }

    /// The interceptor chain around the handler. Every way this can fail leaves as `Err`.
    async fn run_chain(
        ctx: &RpcContext,
        source: &Arc<dyn RpcControllerSource>,
        interceptors: &[Arc<dyn Interceptor<RpcContext, RpcHandlerResult>>],
    ) -> RpcHandlerResult {
        through_interceptors::<Rpc>(ctx, interceptors, Arc::new(ControllerLeaf(source.clone())))
            .await
    }

    /// Drive a renderer with the shared recovery, falling back to the canonical envelope.
    fn safe_render<F>(render: F) -> RpcData
    where
        F: FnOnce() -> RpcData,
    {
        crate::enhancer::pipeline::safe_render(render, Self::fallback_internal_data)
    }

    /// Hardcoded fallback envelope when the regular renderer panics.
    ///
    /// Built from static values, so none of the user code the renderer was
    /// calling runs again. It keeps the canonical envelope: an RPC frame
    /// carries no content type, so a bare string would reach the caller's
    /// decoder as a JSON string where every other reply is an object.
    fn fallback_internal_data() -> RpcData {
        RpcData::json(serde_json::json!({
            "status": "error",
            "kind": "Internal",
            "message": "Internal Server Error",
        }))
    }
}
