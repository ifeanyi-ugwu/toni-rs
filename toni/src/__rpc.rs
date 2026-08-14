//! Bridge between a `#[rpc_controller]` struct and its optional `#[patterns]` impl.
//!
//! `#[rpc_controller]` emits `impl RpcControllerTrait for Struct` with `get_token` baked from the
//! struct name and `get_patterns` / `handle_message` / `enhancers` delegating to `Self::__toni_rpc_*`
//! at the concrete type. `#[patterns]` emits inherent `__toni_rpc_*` fns that out-rank the defaults
//! below. RPC has no connection hooks, so all three are derived from the impl scan — `#[patterns]` is
//! pure aggregation, and a controller without it registers but routes nothing.

#![doc(hidden)]

use async_trait::async_trait;

use crate::context::RpcContext;
use crate::http_helpers::ExecutionResult;
use crate::rpc::{RpcData, RpcEnhancers, RpcError};

/// Blanket "no patterns" defaults, implemented for every type. `#[patterns]` shadows these with
/// inherent fns of the same name, which win at the concrete-type call site in the generated
/// `RpcControllerTrait` impl.
#[async_trait]
pub trait RpcHandlersBridge {
    fn __toni_rpc_get_patterns(&self) -> Vec<String> {
        Vec::new()
    }

    async fn __toni_rpc_handle_message(
        &self,
        ctx: &mut RpcContext,
    ) -> ExecutionResult<Option<RpcData>, RpcError> {
        ExecutionResult::Err(RpcError::PatternNotFound(format!(
            "Unknown pattern: {}",
            ctx.pattern()
        )))
    }

    fn __toni_rpc_enhancers(&self) -> RpcEnhancers {
        RpcEnhancers::default()
    }
}

impl<T: ?Sized + Sync> RpcHandlersBridge for T {}
