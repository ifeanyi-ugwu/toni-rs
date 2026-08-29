mod extractors;
mod rpc_call_info;
mod rpc_client;
mod rpc_client_error;
mod rpc_controller_source;
mod rpc_controller_trait;
mod rpc_controller_wrapper;
mod rpc_data;
mod rpc_error;
mod rpc_handler_output;
pub mod wire;

pub use extractors::PayloadError;
pub use rpc_call_info::RpcCallInfo;
pub use rpc_client::{RpcClient, RpcRequest};
pub use rpc_client_error::RpcClientError;
pub use rpc_controller_source::{RpcControllerSource, RpcEnhancers, RpcHandlerEnhancers};
pub use rpc_controller_trait::RpcControllerTrait;
pub(crate) use rpc_controller_wrapper::RpcControllerWrapper;
pub use rpc_data::RpcData;
pub use rpc_error::RpcError;
pub use rpc_handler_output::RpcHandlerOutput;

/// What an RPC call answers with — the value the pipeline returns and the `R`
/// of [`Interceptor`](crate::traits_helpers::Interceptor) on this transport.
pub type RpcHandlerResult = Result<RpcHandlerOutput, RpcError>;
