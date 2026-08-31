use std::collections::HashMap;
use std::net::SocketAddr;
use std::sync::Arc;
use std::time::{Duration, Instant};

use crate::context::Metadata;

use super::{CancellationToken, Extensions, HandlerContext, shared::SharedState};

/// Per-request context for gRPC handlers.
///
/// gRPC payloads are method-typed protobuf messages and can't sit in a
/// non-generic struct, so the context deliberately holds only what every
/// enhancer can name without a type parameter: the method path, the
/// inbound metadata (ASCII headers), and the optional peer address.
///
/// The same constraint decides how a handler sees this context at all. A gRPC handler's signature
/// is the tonic trait's and never includes one, so guards, interceptors and error handlers receive
/// it as a parameter and a handler takes it off the request instead — [`GrpcContext::of`], or
/// `Extensions::adopt(request.extensions())` for the bag alone.
#[derive(Clone)]
pub struct GrpcContext {
    inner: Arc<GrpcInner>,
}

struct GrpcInner {
    shared: SharedState,
    method: String,
    headers: HashMap<String, String>,
    peer: Option<SocketAddr>,
    /// Read from `grpc-timeout` at construction, so every reader sees one
    /// deadline rather than each recomputing from a clock that has moved.
    deadline: Option<Instant>,
}

impl GrpcContext {
    pub fn new(
        method: impl Into<String>,
        headers: HashMap<String, String>,
        peer: Option<SocketAddr>,
        metadata: Option<Arc<Metadata>>,
    ) -> Self {
        let deadline = headers
            .get("grpc-timeout")
            .and_then(|value| parse_grpc_timeout(value))
            .map(|budget| Instant::now() + budget);
        Self {
            inner: Arc::new(GrpcInner {
                shared: SharedState::new(metadata),
                method: method.into(),
                headers,
                peer,
                deadline,
            }),
        }
    }

    /// The method path the call arrived on, as the caller dialled it:
    /// `package.Service/Method`.
    ///
    /// A guard or interceptor matching on it matches what a proto file, a log
    /// line and a client stub all spell the same way. On a pipeline driven
    /// without an adapter — a test calling the generated method directly — it
    /// falls back to the trait and method names as written, which carry no
    /// package.
    pub fn method(&self) -> &str {
        &self.inner.method
    }

    /// The wire fields that arrived with this call.
    ///
    /// gRPC's specification calls these metadata; `headers` is the one name this framework uses for all of them, leaving `metadata`
    /// to mean what `#[set_metadata]` declared.
    #[doc(alias = "metadata")]
    pub fn headers(&self) -> &HashMap<String, String> {
        &self.inner.headers
    }

    /// One wire field by key.
    #[doc(alias = "get_metadata")]
    #[doc(alias = "metadata")]
    pub fn header(&self, key: &str) -> Option<&str> {
        self.inner.headers.get(key).map(|s| s.as_str())
    }

    pub fn peer(&self) -> Option<SocketAddr> {
        self.inner.peer
    }

    /// The context riding a gRPC request, which is how a handler reaches one.
    ///
    /// `#[grpc_methods]` puts it there before the handler runs, so this answers
    /// `Some` for any service the framework dispatches. A service handed to
    /// tonic directly through `GrpcAdapter::add_service` has no execution
    /// behind it and answers `None`.
    ///
    /// ```ignore
    /// async fn watch(&self, request: Request<WatchRequest>)
    ///     -> Result<Response<Self::WatchStream>, Status>
    /// {
    ///     let ctx = GrpcContext::of(request.extensions()).expect("dispatched by toni");
    ///     let cancelled = ctx.cancellation().clone();
    ///     // …stop feeding the reply once `cancelled.cancelled()` resolves
    /// }
    /// ```
    pub fn of(carrier: &http::Extensions) -> Option<Self> {
        carrier.get::<Self>().cloned()
    }
}

impl HandlerContext for GrpcContext {
    fn metadata(&self) -> Option<&Metadata> {
        self.inner.shared.metadata.as_deref()
    }

    fn extensions(&self) -> &Extensions {
        &self.inner.shared.extensions
    }

    fn cache(&self) -> &crate::traits_helpers::ExecutionCache {
        &self.inner.shared.cache
    }

    fn cancellation(&self) -> &CancellationToken {
        &self.inner.shared.cancellation
    }

    fn deadline(&self) -> Option<Instant> {
        self.inner.deadline
    }
}

/// Parse the `grpc-timeout` header: up to eight digits followed by a unit.
///
/// Defined by the [gRPC HTTP/2 spec][spec]. A value this cannot read is treated
/// as absent — the call is answered rather than refused over a header the caller
/// may not know it sent, and tonic refuses the malformed ones it enforces
/// itself.
///
/// [spec]: https://github.com/grpc/grpc/blob/master/doc/PROTOCOL-HTTP2.md
fn parse_grpc_timeout(value: &str) -> Option<Duration> {
    let (digits, unit) = value.split_at(value.len().checked_sub(1)?);
    if digits.is_empty() || digits.len() > 8 {
        return None;
    }
    let amount: u64 = digits.parse().ok()?;
    match unit {
        "H" => Some(Duration::from_secs(amount * 60 * 60)),
        "M" => Some(Duration::from_secs(amount * 60)),
        "S" => Some(Duration::from_secs(amount)),
        "m" => Some(Duration::from_millis(amount)),
        "u" => Some(Duration::from_micros(amount)),
        "n" => Some(Duration::from_nanos(amount)),
        _ => None,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn every_unit_the_spec_defines() {
        assert_eq!(parse_grpc_timeout("5S"), Some(Duration::from_secs(5)));
        assert_eq!(parse_grpc_timeout("2H"), Some(Duration::from_secs(7200)));
        assert_eq!(parse_grpc_timeout("3M"), Some(Duration::from_secs(180)));
        assert_eq!(parse_grpc_timeout("250m"), Some(Duration::from_millis(250)));
        assert_eq!(parse_grpc_timeout("40u"), Some(Duration::from_micros(40)));
        assert_eq!(parse_grpc_timeout("7n"), Some(Duration::from_nanos(7)));
    }

    #[test]
    fn a_value_the_spec_does_not_define_reads_as_absent() {
        assert_eq!(parse_grpc_timeout(""), None);
        assert_eq!(parse_grpc_timeout("S"), None, "no digits");
        assert_eq!(parse_grpc_timeout("5"), None, "no unit");
        assert_eq!(parse_grpc_timeout("5X"), None, "unknown unit");
        assert_eq!(parse_grpc_timeout("-1S"), None, "not a count");
        assert_eq!(parse_grpc_timeout("123456789S"), None, "over eight digits");
    }

    #[test]
    fn a_context_carries_the_deadline_its_headers_named() {
        let mut headers = HashMap::new();
        headers.insert("grpc-timeout".to_string(), "5S".to_string());
        let ctx = GrpcContext::new("pkg.Svc/Method", headers, None, None);

        let remaining = ctx
            .deadline()
            .expect("a deadline")
            .saturating_duration_since(Instant::now());
        assert!(
            remaining > Duration::from_secs(4) && remaining <= Duration::from_secs(5),
            "remaining: {remaining:?}"
        );
    }

    #[test]
    fn a_context_without_the_header_carries_none() {
        let ctx = GrpcContext::new("pkg.Svc/Method", HashMap::new(), None, None);
        assert!(ctx.deadline().is_none());
    }
}
