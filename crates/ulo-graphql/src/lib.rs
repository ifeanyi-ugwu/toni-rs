//! Transport-neutral GraphQL (transports DESIGN §7): the [`Engine`] SPI, [`GqlRequest`] and
//! [`GqlResponse`]. The specification defines the language, validation and execution, and
//! GraphQL-over-HTTP (`ulo-graphql-http`) and graphql-transport-ws (`ulo-graphql-ws`) are two
//! bindings; an RPC application serves the same engine from an ordinary handler:
//!
//! ```ignore
//! #[ulo_rpc::message("gql")]
//! async fn gql(&self, req: Payload<GqlRequest>, engine: Dep<dyn Engine>) -> GqlResponse { /* .. */ }
//! ```
//!
//! An engine adapter, `ulo-graphql-async-graphql` or `ulo-graphql-juniper`, is bound under the SPI
//! key, `AsyncGraphql<ApiSchema, GqlContext> as dyn Engine`, so a handler on any transport takes
//! `Dep<dyn Engine>`; a second schema is bound with the DI's ordinary qualifier.

use serde::{Deserialize, Serialize};
use serde_json::{Map, Value};
use ulo::{BoxFuture, ExecutionRef};

pub use futures_core::stream::BoxStream;

/// A GraphQL engine, written without macros. Each call runs inside `exec`, the execution the
/// transport opened, from which the adapter resolves the schema's context type per call.
pub trait Engine: Send + Sync + 'static {
    /// Parses, validates and executes a query or mutation.
    fn execute(&self, req: GqlRequest, exec: ExecutionRef) -> BoxFuture<'static, GqlResponse>;

    /// A subscription's responses, one per event; a request that fails before execution yields one
    /// `RequestError` response and ends.
    fn subscribe(&self, req: GqlRequest, exec: ExecutionRef) -> BoxStream<'static, GqlResponse>;

    /// The schema in SDL.
    fn sdl(&self) -> String;

    /// The engine's own playground page for `endpoint` and, where subscriptions are served,
    /// `subscriptions`; `None` serves no page.
    fn playground_html(&self, endpoint: &str, subscriptions: Option<&str>) -> Option<String>;
}

/// A GraphQL request, as GraphQL-over-HTTP's JSON body and graphql-transport-ws's `subscribe`
/// payload carry it.
#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
pub struct GqlRequest {
    pub query: String,
    #[serde(rename = "operationName", default, skip_serializing_if = "Option::is_none")]
    pub operation_name: Option<String>,
    /// `null` and absent alike read as `None`.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub variables: Option<Map<String, Value>>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub extensions: Option<Map<String, Value>>,
}

impl GqlRequest {
    pub fn new(query: impl Into<String>) -> Self {
        GqlRequest { query: query.into(), ..GqlRequest::default() }
    }
}

/// A GraphQL response, serialized as `{data, errors, extensions}` without its outcome, which the
/// HTTP binding reads to decide the status.
#[derive(Clone, Debug, PartialEq, Serialize)]
pub struct GqlResponse {
    #[serde(skip)]
    outcome: Outcome,
    #[serde(skip_serializing_if = "Option::is_none")]
    data: Option<Value>,
    #[serde(skip_serializing_if = "Vec::is_empty")]
    errors: Vec<GqlError>,
    #[serde(skip_serializing_if = "Option::is_none")]
    extensions: Option<Value>,
}

impl GqlResponse {
    /// A request that executed: its data, `null` where execution failed, and its field errors.
    pub fn executed(data: Value, errors: Vec<GqlError>) -> Self {
        GqlResponse { outcome: Outcome::Executed, data: Some(data), errors, extensions: None }
    }

    /// A request that failed before execution began: a document that does not parse or validate,
    /// a missing `query`. It carries no `data`.
    pub fn request_error(errors: Vec<GqlError>) -> Self {
        GqlResponse { outcome: Outcome::RequestError, data: None, errors, extensions: None }
    }

    pub fn with_extensions(mut self, extensions: Value) -> Self {
        self.extensions = Some(extensions);
        self
    }

    pub fn outcome(&self) -> Outcome {
        self.outcome
    }

    pub fn data(&self) -> Option<&Value> {
        self.data.as_ref()
    }

    pub fn errors(&self) -> &[GqlError] {
        &self.errors
    }

    pub fn extensions(&self) -> Option<&Value> {
        self.extensions.as_ref()
    }
}

/// Whether execution began, which decides the HTTP status under
/// `application/graphql-response+json`: 200 for `Executed`, errors included, 400 for
/// `RequestError`.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum Outcome {
    Executed,
    RequestError,
}

/// One GraphQL error, as the specification's response format writes it.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct GqlError {
    pub message: String,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub locations: Vec<Location>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub path: Vec<PathSegment>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub extensions: Option<Value>,
}

impl GqlError {
    pub fn new(message: impl Into<String>) -> Self {
        GqlError { message: message.into(), locations: Vec::new(), path: Vec::new(), extensions: None }
    }
}

/// A position in the request document.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct Location {
    pub line: u32,
    pub column: u32,
}

/// One step of an error's `path`: a field name or a list index.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(untagged)]
pub enum PathSegment {
    Field(String),
    Index(usize),
}
