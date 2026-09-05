// Observing errors off the request path.
//
// toni has no error-observer hook. Anything the framework calls when an error
// happens runs where the error happened — on the request path, with the client
// waiting — so a reporter that must not cost the client latency cannot live
// there. Detaching it inside the framework would mean core owning a runtime,
// which it does not.
//
// The pattern instead: publish the error onto a transport the app already
// speaks, and do the observing in a consumer. The publish is one socket write
// on the request path; everything after it — enriching, shipping to Sentry,
// writing to a store, alerting — happens in another process that can be slow,
// restarted, or scaled without the checkout endpoint noticing.
//
// The seam that reports is an ordinary `ErrorHandler` that declines: it sees
// every error the chain sees, returns `None`, and the canonical envelope
// renders exactly as it would have.
//
// Requires a running RabbitMQ: `docker run -p 5672:5672 rabbitmq:3`
//
// This app is both reporter and consumer so the example is one process. In
// production the consumer is a separate service — that separation is the whole
// point of the pattern.
//
// HTTP endpoints:
//   GET /checkout/42   → 200
//   GET /checkout/0    → 409 rendered as usual, reported on the bus
//
// Run:
//   cargo run --example error_telemetry
//
// Test:
//   curl -i http://127.0.0.1:8080/checkout/0

use std::sync::Arc;

use serde_json::json;
use toni::extractors::Payload;
use toni::{
    async_trait,
    context::HttpContext,
    controller,
    extractors::Path,
    get, module, routes,
    traits_helpers::{ChainError, ErrorHandler},
    Body as ToniBody, Error, ErrorKind, HttpResponse, RpcClient, RpcError, ToniFactory,
};
use toni_macros::{event_pattern, new, patterns};

const ERROR_BUS_PATTERN: &str = "errors.reported";
const RABBIT_URI: &str = "amqp://guest:guest@127.0.0.1:5672/%2f";

// ============================================================================
// Domain error
// ============================================================================

#[derive(Debug)]
struct OutOfStock {
    sku: u32,
}

impl std::fmt::Display for OutOfStock {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "sku {} is out of stock", self.sku)
    }
}

impl std::error::Error for OutOfStock {}

impl Error for OutOfStock {
    fn kind(&self) -> ErrorKind {
        ErrorKind::Conflict
    }
}

// ============================================================================
// The reporting seam — a chain handler that declines
// ============================================================================

/// Publishes every error the chain is handed onto the bus, then answers `None`
/// so the rendering below it is untouched. Registered globally, which gives it
/// the reach the retired `ErrorObserver` had; `#[use_error_handlers(..)]` on a
/// controller or a method scopes it instead.
struct ErrorReporter {
    client: RpcClient,
}

#[async_trait]
impl ErrorHandler<HttpContext, HttpResponse> for ErrorReporter {
    async fn handle_error(&self, error: ChainError<'_>, ctx: &HttpContext) -> Option<HttpResponse> {
        let report = json!({
            "message": error.to_string(),
            "path": ctx.request().uri.path(),
            "method": ctx.request().method.as_str(),
        });

        // Fire-and-forget: this returns once the broker has the message, not
        // once anything has read it. A broker that is down costs one failed
        // write, which is why the result is logged rather than propagated —
        // telemetry does not get to decide whether the client gets an answer.
        if let Err(e) = self.client.emit_json(ERROR_BUS_PATTERN, &report).await {
            tracing::warn!(error = %e, "could not publish the error report");
        }

        None
    }
}

// ============================================================================
// The consumer — the observing itself, off the request path
// ============================================================================

#[controller]
pub struct ErrorConsumer {}

#[patterns]
impl ErrorConsumer {
    #[new]
    pub fn new() -> Self {
        Self {}
    }

    /// Whatever an inline observer would have done goes here instead. It can
    /// take as long as it likes: nobody is waiting on it.
    #[event_pattern("errors.reported")]
    async fn on_error_reported(
        &self,
        Payload(report): Payload<serde_json::Value>,
    ) -> Result<(), RpcError> {
        println!(
            "[consumer] {} {} → {}",
            report["method"].as_str().unwrap_or("?"),
            report["path"].as_str().unwrap_or("?"),
            report["message"].as_str().unwrap_or("?"),
        );
        Ok(())
    }
}

// ============================================================================
// The endpoint that fails
// ============================================================================

#[controller("/checkout")]
pub struct CheckoutController {}

#[routes]
impl CheckoutController {
    #[new]
    pub fn new() -> Self {
        Self {}
    }

    /// A handler error, not a framework event: the chain sees both, so the
    /// reporter above needs no second registration to catch domain failures.
    #[get("/{sku}")]
    async fn checkout(&self, Path(sku): Path<u32>) -> Result<ToniBody, OutOfStock> {
        if sku == 0 {
            return Err(OutOfStock { sku });
        }
        Ok(ToniBody::json(json!({ "sku": sku, "status": "confirmed" })))
    }
}

#[module(controllers: [CheckoutController, ErrorConsumer])]
struct CheckoutModule;

// ============================================================================
// Main
// ============================================================================

#[tokio::main]
async fn main() -> anyhow::Result<()> {
    println!("Error telemetry example");
    println!("  GET http://127.0.0.1:8080/checkout/0   → 409, reported on the bus");
    println!("  GET http://127.0.0.1:8080/checkout/42  → 200");
    println!();

    let mut factory = ToniFactory::new();

    // One client, one connection, shared by every call into the reporter.
    factory.use_global_http_error_handler(Arc::new(ErrorReporter {
        client: RpcClient::new(toni_rabbitmq::RabbitMqClientTransport::new(RABBIT_URI)),
    }));

    let mut app = factory.create_with(CheckoutModule).await?;
    app.use_http_adapter(toni_axum::AxumAdapter::new(), ("127.0.0.1", 8080))
        .unwrap();
    app.use_rpc_adapter(toni_rabbitmq::RabbitMqAdapter::new(RABBIT_URI))
        .unwrap();

    app.start().await?;
    Ok(())
}
