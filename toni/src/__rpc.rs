//! Bridge between a `#[rpc_controller]` struct and its optional `#[patterns]` impl.
//!
//! `#[rpc_controller]` emits `handle_message` on the struct and a `RpcControllerSource` companion
//! carrying the patterns and the enhancer tokens; all three delegate to `Self::__toni_rpc_*` at the
//! concrete type. `#[patterns]` emits inherent `__toni_rpc_*` fns that out-rank the defaults below.
//! RPC has no connection hooks, so all three are derived from the impl scan — `#[patterns]` is pure
//! aggregation, and a controller without it registers but routes nothing.
//!
//! What a controller *declares* — its patterns and its enhancer tokens — takes no receiver, because
//! the framework has to read it at startup to register the controller, and a request-scoped
//! controller has no instance until a call arrives. Only `handle_message` needs one.
//!
//! Both forms carry the same constraint: the call site must name the concrete type. Reached through
//! a generic, `T::__toni_rpc_patterns()` resolves to the default below and answers empty rather than
//! failing — see ADR 0001.

#![doc(hidden)]

use async_trait::async_trait;

use crate::context::Metadata;
use crate::context::RpcContext;
use crate::http_helpers::ExecutionResult;
use crate::rpc::{RpcData, RpcEnhancers, RpcError};

/// Blanket "no patterns" defaults, implemented for every type. `#[patterns]` shadows these with
/// inherent fns of the same name, which win at the concrete-type call site in the generated
/// `RpcControllerTrait` impl.
#[async_trait]
pub trait RpcHandlersBridge {
    fn __toni_rpc_patterns() -> Vec<String>
    where
        Self: Sized,
    {
        Vec::new()
    }

    async fn __toni_rpc_handle_message(
        &self,
        ctx: &RpcContext,
    ) -> ExecutionResult<Option<RpcData>, RpcError> {
        ExecutionResult::Err(RpcError::PatternNotFound(format!(
            "Unknown pattern: {}",
            ctx.pattern()
        )))
    }

    fn __toni_rpc_enhancers() -> RpcEnhancers
    where
        Self: Sized,
    {
        RpcEnhancers::default()
    }

    fn __toni_rpc_metadata() -> Metadata
    where
        Self: Sized,
    {
        Metadata::new()
    }

    fn __toni_rpc_handler_metadata() -> Vec<(String, Metadata)>
    where
        Self: Sized,
    {
        Vec::new()
    }
}

impl<T: ?Sized + Sync> RpcHandlersBridge for T {}
