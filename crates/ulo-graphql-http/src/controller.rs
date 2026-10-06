//! The GraphQL-over-HTTP endpoint: a controller mounted `.at(config.path)`, reading
//! `Dep<dyn Engine, Q>`, answering GET and POST by the specification's rules from the response's
//! `Outcome`, and the negotiated media type.

use std::marker::PhantomData;

use http::header::{ACCEPT, CONTENT_TYPE, HeaderMap, HeaderValue};
use http::{Method, StatusCode};
use serde::Deserialize;
use serde_json::{Map, Value};
use ulo::scope::Auto;
use ulo::{BoxError, BoxFuture, Construct, ConstructError, Controller, Dep, Dependencies, HandlerSpec, Mount, Resolver};
use ulo_graphql::{Engine, GqlError, GqlRequest, GqlResponse, Outcome};
use ulo_http::__private::HttpHandler;
use ulo_http::{Bytes, Http, HttpBody, HttpCx, MethodNotAllowed, Query, Response};
use ulo_transport::{CallError, ErrorKind, ExtractError, FromCall, IntoReplyError};

use crate::playground;

/// What `GraphqlModule` binds beside the controller, read by it alone: the endpoint's path and the
/// subscription path as the playground writes them, and whether the playground is served.
pub(crate) struct EndpointSettings<Q> {
    pub(crate) path: String,
    pub(crate) subscriptions: Option<String>,
    pub(crate) playground: bool,
    pub(crate) _engine: PhantomData<fn() -> Q>,
}

/// The endpoint over the engine bound as `dyn Engine @ Q`. Its two handlers answer at `/`, which
/// the controller's prefix puts at the configured path.
pub(crate) struct GraphqlEndpoint<Q> {
    engine: Dep<dyn Engine, Q>,
    settings: Dep<EndpointSettings<Q>>,
}

impl<Q: Send + Sync + 'static> Construct for GraphqlEndpoint<Q> {
    type Scope = Auto;

    fn dependencies(d: &mut Dependencies) {
        d.field::<Dep<dyn Engine, Q>>("engine").field::<Dep<EndpointSettings<Q>>>("settings");
    }

    async fn construct(r: &Resolver<'_>) -> Result<Self, ConstructError> {
        Ok(GraphqlEndpoint {
            engine: r.dep_qualified::<dyn Engine, Q>().await?,
            settings: r.dep::<EndpointSettings<Q>>().await?,
        })
    }
}

impl<Q: Send + Sync + 'static> Controller for GraphqlEndpoint<Q> {
    fn mount(m: &mut Mount<'_>) {
        let on_get = HttpHandler::new(Method::GET, "/", |cx: HttpCx| -> BoxFuture<'static, Result<Response, BoxError>> {
            Box::pin(get::<Q>(cx))
        });
        m.handler(HandlerSpec::<Http, _>::new("get", on_get).route("/"));
        let on_post = HttpHandler::new(Method::POST, "/", |cx: HttpCx| -> BoxFuture<'static, Result<Response, BoxError>> {
            Box::pin(post::<Q>(cx))
        });
        m.handler(HandlerSpec::<Http, _>::new("post", on_post).route("/"));
    }
}

/// The query parameters a GET names; `variables` and `extensions` are JSON text.
#[derive(Deserialize)]
struct GetParams {
    query: Option<String>,
    #[serde(rename = "operationName")]
    operation_name: Option<String>,
    variables: Option<String>,
    extensions: Option<String>,
}

/// A GET: the playground for a browser asking for HTML with no `query`, otherwise a query read from
/// the query string. A mutation is refused with 405 before anything executes.
async fn get<Q: Send + Sync + 'static>(cx: HttpCx) -> Result<Response, BoxError> {
    let endpoint = cx.exec().get::<GraphqlEndpoint<Q>>().await?;
    let Query(params) = <Query<GetParams> as FromCall<Http>>::from_call(&cx).await?;
    let media = Media::negotiate(cx.headers());
    let Some(query) = params.query else {
        if endpoint.settings.playground && playground::accepts_html(cx.headers()) {
            if let Some(page) = playground::page(&*endpoint.engine, &endpoint.settings, cx.mount_prefix()) {
                return Ok(page);
            }
        }
        return render(&request_error("the request names no `query`"), media);
    };
    let variables = match json_object(params.variables.as_deref(), "variables") {
        Ok(variables) => variables,
        Err(response) => return render(&response, media),
    };
    let extensions = match json_object(params.extensions.as_deref(), "extensions") {
        Ok(extensions) => extensions,
        Err(response) => return render(&response, media),
    };
    let request = GqlRequest { query, operation_name: params.operation_name, variables, extensions };
    if operation_kind(&request.query, request.operation_name.as_deref()) == Some(OperationKind::Mutation) {
        return Err(mutation_over_get());
    }
    let response = endpoint.engine.execute(request, cx.exec().clone()).await;
    render(&response, media)
}

/// A POST: an `application/json` body holding the request. Another media type is refused with 415
/// through the error handlers, a body over the route's limit with 413.
async fn post<Q: Send + Sync + 'static>(cx: HttpCx) -> Result<Response, BoxError> {
    let endpoint = cx.exec().get::<GraphqlEndpoint<Q>>().await?;
    if !is_json(cx.headers()) {
        return Err(ExtractError::UnsupportedMediaType { param: "body", expected: "application/json" }.into());
    }
    let body = <Bytes as FromCall<Http>>::from_call(&cx).await?;
    let media = Media::negotiate(cx.headers());
    let request = match serde_json::from_slice::<GqlRequest>(&body) {
        Ok(request) => request,
        Err(error) => return render(&request_error(&format!("the body is not a GraphQL request: {error}")), media),
    };
    let response = endpoint.engine.execute(request, cx.exec().clone()).await;
    render(&response, media)
}

/// The response media type, negotiated from `Accept`.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Media {
    GraphqlResponse,
    Json,
}

impl Media {
    /// `application/graphql-response+json` when the client accepts it at least as much as
    /// `application/json`, and when it sends no `Accept`, which the specification reads that way
    /// from its 2025-01-01 watershed on; `application/json` when it prefers that, or accepts
    /// neither.
    fn negotiate(headers: &HeaderMap) -> Media {
        let ranges = accept_ranges(headers);
        if ranges.is_empty() {
            return Media::GraphqlResponse;
        }
        let graphql = quality(&ranges, "application", "graphql-response+json");
        let json = quality(&ranges, "application", "json");
        if graphql > 0 && graphql >= json { Media::GraphqlResponse } else { Media::Json }
    }

    fn content_type(self) -> HeaderValue {
        match self {
            Media::GraphqlResponse => HeaderValue::from_static("application/graphql-response+json; charset=utf-8"),
            Media::Json => HeaderValue::from_static("application/json; charset=utf-8"),
        }
    }

    /// GraphQL-over-HTTP's status: under `application/graphql-response+json` 200 for a request
    /// that executed, errors included, and 400 for one that failed before execution; under the
    /// legacy `application/json`, 200 for every GraphQL response. A request the server failed is
    /// 500 under both, the fault being the server's and no document's.
    fn status(self, outcome: Outcome) -> StatusCode {
        match (self, outcome) {
            (_, Outcome::Failed) => StatusCode::INTERNAL_SERVER_ERROR,
            (Media::GraphqlResponse, Outcome::RequestError) => StatusCode::BAD_REQUEST,
            _ => StatusCode::OK,
        }
    }
}

/// One `Accept` media range: type, subtype, lowercased, and its quality in thousandths.
pub(crate) struct Range {
    pub(crate) kind: String,
    pub(crate) subtype: String,
    pub(crate) q: u16,
}

/// Every media range of every `Accept` header; empty when there is none, or only empty ones.
pub(crate) fn accept_ranges(headers: &HeaderMap) -> Vec<Range> {
    let mut ranges = Vec::new();
    for value in headers.get_all(ACCEPT) {
        let Ok(text) = value.to_str() else { continue };
        for item in text.split(',') {
            let mut parts = item.split(';');
            let Some((kind, subtype)) = parts.next().and_then(|range| range.trim().split_once('/')) else { continue };
            let mut q = 1000;
            for parameter in parts {
                if let Some((name, value)) = parameter.split_once('=') {
                    if name.trim().eq_ignore_ascii_case("q") {
                        q = parse_quality(value.trim());
                    }
                }
            }
            ranges.push(Range { kind: kind.trim().to_ascii_lowercase(), subtype: subtype.trim().to_ascii_lowercase(), q });
        }
    }
    ranges
}

/// RFC 9110 §12.4.2's qvalue, `0` to `1` with at most three decimals; anything else reads as 0.
fn parse_quality(text: &str) -> u16 {
    let (whole, fraction) = text.split_once('.').unwrap_or((text, ""));
    if fraction.len() > 3 || !fraction.bytes().all(|b| b.is_ascii_digit()) {
        return 0;
    }
    let thousandths = format!("{fraction:0<3}").parse::<u16>().unwrap_or(0);
    match whole {
        "0" => thousandths,
        "1" if thousandths == 0 => 1000,
        _ => 0,
    }
}

/// The quality `ranges` give `kind/subtype`: that of the most specific range matching it, exact
/// before `kind/*` before `*/*` (RFC 9110 §12.5.1); 0 when none matches.
pub(crate) fn quality(ranges: &[Range], kind: &str, subtype: &str) -> u16 {
    let mut best: Option<(u8, u16)> = None;
    for range in ranges {
        let specificity = match (range.kind.as_str(), range.subtype.as_str()) {
            (k, s) if k == kind && s == subtype => 3,
            (k, "*") if k == kind => 2,
            ("*", "*") => 1,
            _ => continue,
        };
        if best.is_none_or(|(seen, _)| specificity > seen) {
            best = Some((specificity, range.q));
        }
    }
    best.map_or(0, |(_, q)| q)
}

/// Whether the request's `Content-Type` is `application/json`, parameters aside.
fn is_json(headers: &HeaderMap) -> bool {
    headers
        .get(CONTENT_TYPE)
        .and_then(|value| value.to_str().ok())
        .and_then(|value| value.split(';').next())
        .is_some_and(|essence| essence.trim().eq_ignore_ascii_case("application/json"))
}

/// A query parameter holding JSON text, as an object; absent and `null` alike are `None`. Text
/// that is not a JSON object is a request error.
fn json_object(text: Option<&str>, name: &str) -> Result<Option<Map<String, Value>>, GqlResponse> {
    let Some(text) = text else { return Ok(None) };
    serde_json::from_str::<Option<Map<String, Value>>>(text)
        .map_err(|error| request_error(&format!("`{name}` is not a JSON object: {error}")))
}

fn request_error(message: &str) -> GqlResponse {
    GqlResponse::request_error(vec![GqlError::new(message)])
}

/// `response` in `media`, with the status GraphQL-over-HTTP gives its outcome.
fn render(response: &GqlResponse, media: Media) -> Result<Response, BoxError> {
    let body = serde_json::to_vec(response).map_err(IntoReplyError::new)?;
    let mut reply = Response::new(HttpBody::from_bytes(body));
    *reply.status_mut() = media.status(response.outcome());
    reply.headers_mut().insert(CONTENT_TYPE, media.content_type());
    Ok(reply)
}

/// The 405 for a GET whose operation is a mutation, which the specification forbids executing:
/// a `BadRequest` whose source is [`MethodNotAllowed`] with `Allow` as the router computes it for
/// this path, offered to the error handlers as the router's 405 is and rendered 405 with `Allow`
/// when none claims it.
fn mutation_over_get() -> BoxError {
    let refused = MethodNotAllowed::new(Method::GET, HeaderValue::from_static("GET, HEAD, POST, OPTIONS"));
    BoxError::from(CallError::new(ErrorKind::BadRequest, "a mutation is not executed over GET; send it as a POST").with_source(refused))
}

/// An operation's type.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum OperationKind {
    Query,
    Mutation,
    Subscription,
}

/// The type of the operation `document` selects: the one named `operation_name`, or its only
/// operation. `None` when that cannot be told, a document that does not lex or selects no single
/// operation, which the engine then reports as a request error.
///
/// A scan of the document's top level, not a parse: comments, strings and block strings are
/// skipped, and what sits inside parentheses (variable defaults, directive arguments) never opens
/// a selection set.
fn operation_kind(document: &str, operation_name: Option<&str>) -> Option<OperationKind> {
    #[derive(PartialEq)]
    enum State {
        /// Between definitions.
        Start,
        /// After `query`, `mutation` or `subscription`: the next name is the operation's.
        Keyword,
        /// In an operation's or a fragment's header, before its selection set.
        Header,
        /// Inside a selection set.
        Body,
    }

    let mut operations: Vec<(OperationKind, Option<String>)> = Vec::new();
    let mut state = State::Start;
    let mut pending: Option<(OperationKind, Option<String>)> = None;
    let mut braces = 0usize;
    let mut parens = 0usize;
    let bytes = document.as_bytes();
    let mut i = 0;
    while i < bytes.len() {
        let b = bytes[i];
        match b {
            b'#' => {
                while i < bytes.len() && bytes[i] != b'\n' && bytes[i] != b'\r' {
                    i += 1;
                }
                continue;
            }
            b'"' => {
                i = skip_string(bytes, i)?;
                continue;
            }
            b'(' => parens += 1,
            b')' => parens = parens.checked_sub(1)?,
            b'{' if parens == 0 => {
                if braces == 0 {
                    match state {
                        State::Start => operations.push((OperationKind::Query, None)),
                        State::Keyword | State::Header => operations.extend(pending.take()),
                        State::Body => return None,
                    }
                    state = State::Body;
                }
                braces += 1;
            }
            b'}' if parens == 0 => {
                braces = braces.checked_sub(1)?;
                if braces == 0 {
                    state = State::Start;
                }
            }
            b'_' | b'a'..=b'z' | b'A'..=b'Z' => {
                let start = i;
                while i < bytes.len() && (bytes[i] == b'_' || bytes[i].is_ascii_alphanumeric()) {
                    i += 1;
                }
                if braces == 0 && parens == 0 {
                    let name = &document[start..i];
                    match state {
                        State::Start => {
                            let kind = match name {
                                "query" => Some(OperationKind::Query),
                                "mutation" => Some(OperationKind::Mutation),
                                "subscription" => Some(OperationKind::Subscription),
                                "fragment" => None,
                                _ => return None,
                            };
                            pending = kind.map(|kind| (kind, None));
                            state = if pending.is_some() { State::Keyword } else { State::Header };
                        }
                        State::Keyword => {
                            if let Some((_, slot)) = pending.as_mut() {
                                *slot = Some(name.to_owned());
                            }
                            state = State::Header;
                        }
                        State::Header | State::Body => {}
                    }
                }
                continue;
            }
            b'@' | b'$' if braces == 0 && parens == 0 && state == State::Keyword => state = State::Header,
            _ => {}
        }
        if b == b'(' && braces == 0 && state == State::Keyword {
            state = State::Header;
        }
        i += 1;
    }
    if braces != 0 || parens != 0 {
        return None;
    }
    let selected = match operation_name {
        Some(wanted) => operations.into_iter().find(|(_, name)| name.as_deref() == Some(wanted)),
        None if operations.len() == 1 => operations.pop(),
        None => None,
    };
    selected.map(|(kind, _)| kind)
}

/// The index just past the string starting at `start`, a `"..."` string with escapes or a
/// `"""..."""` block string; `None` for one left open.
fn skip_string(bytes: &[u8], start: usize) -> Option<usize> {
    if bytes[start..].starts_with(b"\"\"\"") {
        let mut i = start + 3;
        while i < bytes.len() {
            if bytes[i..].starts_with(b"\\\"\"\"") {
                i += 4;
            } else if bytes[i..].starts_with(b"\"\"\"") {
                return Some(i + 3);
            } else {
                i += 1;
            }
        }
        return None;
    }
    let mut i = start + 1;
    while i < bytes.len() {
        match bytes[i] {
            b'\\' => i += 2,
            b'"' => return Some(i + 1),
            b'\n' | b'\r' => return None,
            _ => i += 1,
        }
    }
    None
}
