use crate::async_trait;
use crate::context::{HandlerContext, HttpContext, RpcContext, WsContext};
use crate::errors::HttpError;
use crate::http_helpers::{Body, HttpResponse};
use crate::rpc::RpcData;
use crate::websocket::WsMessage;
use serde_json::json;
use std::error::Error;

/// Convenience alias for the borrowed error reference passed to handlers.
///
/// The chain owns the boxed error and lends it to each handler in turn —
/// handlers borrow, downcast, and either claim (`Some`) or fall through
/// (`None`). Borrowing avoids forcing user errors to be `Clone` so each
/// chain iteration can re-box them.
pub type ChainError<'a> = &'a (dyn Error + Send + Sync + 'static);

/// Customize how errors are turned into responses.
///
/// Handlers are tried in order (method > controller > global) until one
/// returns `Some`. Return `None` to pass to the next handler; if all return
/// `None`, the framework's default fallback is sent.
#[async_trait]
pub trait ErrorHandler<C: ?Sized + HandlerContext, R>: Send + Sync {
    async fn handle_error(&self, error: ChainError<'_>, ctx: &C) -> Option<R>;
}

/// Default fallback for HTTP routes that didn't match a registered handler.
pub struct DefaultHttpErrorHandler;

#[async_trait]
impl ErrorHandler<HttpContext, HttpResponse> for DefaultHttpErrorHandler {
    async fn handle_error(
        &self,
        error: ChainError<'_>,
        _ctx: &HttpContext,
    ) -> Option<HttpResponse> {
        if let Some(http_error) = error.downcast_ref::<HttpError>() {
            return Some(http_error.to_response());
        }
        Some(HttpResponse {
            status: 500,
            body: Some(Body::json(json!({
                "statusCode": 500,
                "message": "Internal Server Error",
                "error": "Internal Server Error",
            }))),
            headers: vec![],
        })
    }
}

/// Default fallback for RPC handlers that returned an error not claimed by a
/// registered handler.
pub struct DefaultRpcErrorHandler;

#[async_trait]
impl ErrorHandler<RpcContext, RpcData> for DefaultRpcErrorHandler {
    async fn handle_error(&self, error: ChainError<'_>, _ctx: &RpcContext) -> Option<RpcData> {
        let message = if let Some(http_error) = error.downcast_ref::<HttpError>() {
            http_error.message().to_string()
        } else {
            error.to_string()
        };
        Some(RpcData::json(
            json!({ "status": "error", "message": message }),
        ))
    }
}

/// Default fallback for WebSocket handlers.
pub struct DefaultWsErrorHandler;

#[async_trait]
impl ErrorHandler<WsContext, WsMessage> for DefaultWsErrorHandler {
    async fn handle_error(&self, error: ChainError<'_>, _ctx: &WsContext) -> Option<WsMessage> {
        let message = if let Some(http_error) = error.downcast_ref::<HttpError>() {
            http_error.message().to_string()
        } else {
            error.to_string()
        };
        Some(WsMessage::text(
            json!({ "status": "error", "message": message }).to_string(),
        ))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn default_http_handler_with_http_error() {
        let handler = DefaultHttpErrorHandler;
        let error = HttpError::not_found("Resource not found");
        let stub = http::Request::builder().body(()).unwrap();
        let ctx = HttpContext::from_parts(stub.into_parts().0);
        let r = handler.handle_error(&error, &ctx).await.unwrap();
        assert_eq!(r.status, 404);
    }

    #[tokio::test]
    async fn default_http_handler_with_unknown_error() {
        let handler = DefaultHttpErrorHandler;
        let error = std::io::Error::new(std::io::ErrorKind::Other, "Unknown error");
        let stub = http::Request::builder().body(()).unwrap();
        let ctx = HttpContext::from_parts(stub.into_parts().0);
        let r = handler.handle_error(&error, &ctx).await.unwrap();
        assert_eq!(r.status, 500);
    }
}
