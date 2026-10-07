//! What a `#[ulo_grpc::method]` handler may answer past the one form per call shape `grpc.rs`
//! covers: well-known types on both sides, a proto with no `package`, `Response<S>` around a
//! stream, the stream arms under a `Result`, a `tonic::Status` returned as the error by a handler
//! or by an error handler, an error handler answering a domain error with a message or a stream of
//! its own, and `GrpcMetadata` as a parameter.

mod support;

use std::error::Error;
use std::fmt;

use futures_util::{Stream, StreamExt, stream};
use tonic::{Code, Status};
use ulo::{BoxError, Dep, ErrorHandler, Module, ModuleDef, ModuleIdentity, injectable, routes};
use ulo_codegen_tests::bare::{self, Word, bare_client::BareClient};
use ulo_codegen_tests::probe::known_client::KnownClient;
use ulo_codegen_tests::probe::shapes_client::ShapesClient;
use ulo_codegen_tests::probe::{self, Tick, Ticks};
use ulo_grpc::{Grpc, GrpcCx, GrpcMetadata, Message, Method, Reply, Response};
use ulo_transport::{CallError, Classify};

use support::{Record, Running};

/// The instant `Since` measures from, in seconds since the Unix epoch.
const EPOCH: i64 = 1_000_000_000;

/// A handler's own failure, classified `bad_request`.
#[derive(Debug, Classify)]
#[classify(bad_request)]
struct Refusal;

impl fmt::Display for Refusal {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str("refused")
    }
}

impl Error for Refusal {}

#[injectable]
struct KnownTypes;

#[routes]
impl KnownTypes {
    /// `google.protobuf.Empty` both ways: no message read, `()` answered.
    #[ulo_grpc::method(probe::known::Ping)]
    async fn ping(&self) {}

    /// `google.protobuf.StringValue` both ways, as the `String` prost maps it to.
    #[ulo_grpc::method(probe::known::Upper)]
    fn upper(&self, text: Message<String>) -> String {
        text.0.to_uppercase()
    }

    /// `prost_types` messages both ways.
    #[ulo_grpc::method(probe::known::Since)]
    fn since(&self, at: Message<prost_types::Timestamp>) -> prost_types::Duration {
        prost_types::Duration { seconds: at.0.seconds - EPOCH, nanos: at.0.nanos }
    }
}

#[injectable]
struct BareService;

#[routes]
impl BareService {
    #[ulo_grpc::method(bare::bare::Echo)]
    fn echo(&self, word: Message<Word>) -> Word {
        Word { text: format!("echo: {}", word.0.text) }
    }
}

/// `1` to `n`.
fn ticks(n: u32) -> impl Stream<Item = Result<Tick, Refusal>> + Send + 'static {
    stream::iter((1..=n).map(|n| Ok(Tick { n })))
}

/// The code of each `tonic::Status` the `Lookup` error handler was offered.
#[derive(Clone)]
struct Offered(Record<Option<Code>>);

/// Records whether the error it is offered is a `tonic::Status`, and hands it on unchanged.
#[injectable]
struct Observe {
    offered: Dep<Offered>,
}

impl ErrorHandler<Grpc> for Observe {
    async fn handle(&self, err: BoxError, _cx: &GrpcCx) -> Result<Reply, BoxError> {
        self.offered.0.push(err.downcast_ref::<Status>().map(Status::code));
        Err(err)
    }
}

/// Answers any error with a `tonic::Status` of its own.
struct AsStatus;

impl ErrorHandler<Grpc> for AsStatus {
    async fn handle(&self, _err: BoxError, _cx: &GrpcCx) -> Result<Reply, BoxError> {
        Err(BoxError::from(Status::already_exists("rescued")))
    }
}

/// The message [`Substitute`] answers a [`Refusal`] with.
const SUBSTITUTE: Tick = Tick { n: 42 };

/// Answers a [`Refusal`] with [`SUBSTITUTE`] and the reply metadata `x-substituted: yes`; anything
/// else passes on.
struct Substitute;

impl ErrorHandler<Grpc> for Substitute {
    async fn handle(&self, err: BoxError, cx: &GrpcCx) -> Result<Reply, BoxError> {
        if !refused(&err) {
            return Err(err);
        }
        Ok(cx.reply(Response::new(SUBSTITUTE).metadata("x-substituted", "yes")))
    }
}

/// Answers a [`Refusal`] with the ticks `1` to `3`; anything else passes on.
struct SubstituteStream;

impl ErrorHandler<Grpc> for SubstituteStream {
    async fn handle(&self, err: BoxError, cx: &GrpcCx) -> Result<Reply, BoxError> {
        if !refused(&err) {
            return Err(err);
        }
        Ok(cx.reply_stream(ticks(3)))
    }
}

fn refused(err: &BoxError) -> bool {
    err.downcast_ref::<CallError>().and_then(|call| call.source_as::<Refusal>()).is_some()
}

#[injectable]
struct ShapesService;

#[routes]
impl ShapesService {
    #[ulo_grpc::method(probe::shapes::Watch)]
    fn watch(&self, req: Message<Ticks>) -> Response<impl Stream<Item = Result<Tick, Refusal>>> {
        Response::new(ticks(req.0.count)).metadata("x-shape", "response")
    }

    #[ulo_grpc::method(probe::shapes::WatchOrFail)]
    async fn watch_or_fail(&self, req: Message<Ticks>) -> Result<impl Stream<Item = Result<Tick, Refusal>>, Refusal> {
        if req.0.fail { Err(Refusal) } else { Ok(ticks(req.0.count)) }
    }

    #[ulo_grpc::method(probe::shapes::WatchOrStatus)]
    async fn watch_or_status(&self, req: Message<Ticks>) -> Result<Response<impl Stream<Item = Result<Tick, Refusal>>>, Status> {
        if req.0.fail {
            return Err(Status::resource_exhausted("no ticks left"));
        }
        Ok(Response::new(ticks(req.0.count)).metadata("x-shape", "result of a response"))
    }

    #[ulo_grpc::method(probe::shapes::Lookup)]
    #[error_handlers(Observe)]
    fn lookup(&self, req: Message<Ticks>) -> Result<Tick, Status> {
        if req.0.fail { Err(Status::already_exists("tick taken")) } else { Ok(Tick { n: req.0.count }) }
    }

    #[ulo_grpc::method(probe::shapes::Rescue)]
    #[error_handlers(value = AsStatus)]
    fn rescue(&self, req: Message<Ticks>) -> Result<Tick, Refusal> {
        if req.0.fail { Err(Refusal) } else { Ok(Tick { n: req.0.count }) }
    }

    #[ulo_grpc::method(probe::shapes::Substituted)]
    #[error_handlers(value = Substitute)]
    fn substituted(&self, req: Message<Ticks>) -> Result<Tick, Refusal> {
        if req.0.fail { Err(Refusal) } else { Ok(Tick { n: req.0.count }) }
    }

    #[ulo_grpc::method(probe::shapes::SubstitutedStream)]
    #[error_handlers(value = SubstituteStream)]
    async fn substituted_stream(&self, req: Message<Ticks>) -> Result<impl Stream<Item = Result<Tick, Refusal>>, Refusal> {
        if req.0.fail { Err(Refusal) } else { Ok(ticks(req.0.count)) }
    }

    /// The `x-user` metadata the caller sent.
    #[ulo_grpc::method(probe::shapes::Whoami)]
    fn whoami(&self, metadata: GrpcMetadata) -> String {
        metadata.get("x-user").unwrap_or("nobody").to_owned()
    }
}

struct Root {
    offered: Offered,
}

impl Module for Root {
    fn identity(&self) -> ModuleIdentity {
        ModuleIdentity::of_type::<Self>()
    }

    fn register(&self, m: &mut ModuleDef<'_>) {
        m.value(self.offered.clone());
        m.provide::<Observe>();
        m.controller::<KnownTypes>();
        m.controller::<BareService>();
        m.controller::<ShapesService>();
    }
}

async fn start() -> (Running, Offered) {
    let offered = Offered(Record::new());
    let app = Running::start(Root { offered: offered.clone() }, ulo_grpc::Server::new("127.0.0.1:0")).await;
    (app, offered)
}

fn request(count: u32, fail: bool) -> Ticks {
    Ticks { count, fail }
}

/// Every item of a reply stream, failing the test on an item error.
async fn collect(items: tonic::Streaming<Tick>) -> Vec<u32> {
    items.map(|item| item.unwrap_or_else(|status| panic!("a reply item failed: {status:?}")).n).collect().await
}

fn failed<T: fmt::Debug>(reply: Result<T, Status>) -> Status {
    match reply {
        Err(status) => status,
        Ok(reply) => panic!("expected the call to fail, got {reply:?}"),
    }
}

#[tokio::test]
async fn empty_is_answered_for_empty() {
    let (app, _) = start().await;
    let mut client = KnownClient::new(app.channel().await);
    let reply = client.ping(()).await.unwrap_or_else(|status| panic!("Ping failed: {status:?}"));
    assert_eq!(reply.into_inner(), ());
    app.stop().await;
}

#[tokio::test]
async fn a_wrapper_type_travels_as_the_rust_type_it_wraps() {
    let (app, _) = start().await;
    let mut client = KnownClient::new(app.channel().await);
    let reply = client.upper("ada".to_owned()).await.unwrap_or_else(|status| panic!("Upper failed: {status:?}"));
    assert_eq!(reply.into_inner(), "ADA");
    app.stop().await;
}

#[tokio::test]
async fn prost_types_messages_travel_both_ways() {
    let (app, _) = start().await;
    let mut client = KnownClient::new(app.channel().await);
    let at = prost_types::Timestamp { seconds: EPOCH + 90, nanos: 5 };
    let reply = client.since(at).await.unwrap_or_else(|status| panic!("Since failed: {status:?}"));
    assert_eq!(reply.into_inner(), prost_types::Duration { seconds: 90, nanos: 5 });
    app.stop().await;
}

#[tokio::test]
async fn a_proto_without_a_package_is_served_at_its_bare_service_path() {
    let (app, _) = start().await;
    assert_eq!(<bare::bare::Echo as Method>::PATH, "/Bare/Echo");
    let mut client = BareClient::new(app.channel().await);
    let reply = client.echo(Word { text: "hi".to_owned() }).await.unwrap_or_else(|status| panic!("Echo failed: {status:?}"));
    assert_eq!(reply.into_inner(), Word { text: "echo: hi".to_owned() });
    app.stop().await;
}

#[tokio::test]
async fn response_around_a_stream_carries_its_metadata_then_the_items() {
    let (app, _) = start().await;
    let mut client = ShapesClient::new(app.channel().await);
    let reply = client.watch(request(3, false)).await.unwrap_or_else(|status| panic!("Watch failed: {status:?}"));
    assert_eq!(reply.metadata().get("x-shape").and_then(|value| value.to_str().ok()), Some("response"));
    assert_eq!(collect(reply.into_inner()).await, vec![1, 2, 3]);
    app.stop().await;
}

#[tokio::test]
async fn a_result_of_a_stream_answers_ok_as_the_stream_and_err_by_its_kind() {
    let (app, _) = start().await;
    let mut client = ShapesClient::new(app.channel().await);
    let reply = client.watch_or_fail(request(2, false)).await.unwrap_or_else(|status| panic!("WatchOrFail failed: {status:?}"));
    assert_eq!(collect(reply.into_inner()).await, vec![1, 2]);

    let status = failed(client.watch_or_fail(request(2, true)).await);
    assert_eq!((status.code(), status.message()), (Code::InvalidArgument, "refused"));
    app.stop().await;
}

#[tokio::test]
async fn a_result_of_a_response_stream_with_a_status_error_answers_either_as_written() {
    let (app, _) = start().await;
    let mut client = ShapesClient::new(app.channel().await);
    let reply = client.watch_or_status(request(2, false)).await.unwrap_or_else(|status| panic!("WatchOrStatus failed: {status:?}"));
    assert_eq!(reply.metadata().get("x-shape").and_then(|value| value.to_str().ok()), Some("result of a response"));
    assert_eq!(collect(reply.into_inner()).await, vec![1, 2]);

    let status = failed(client.watch_or_status(request(2, true)).await);
    assert_eq!((status.code(), status.message()), (Code::ResourceExhausted, "no ticks left"));
    app.stop().await;
}

#[tokio::test]
async fn a_status_returned_as_the_error_reaches_the_error_handlers_and_then_the_wire_as_it_stands() {
    let (app, offered) = start().await;
    let mut client = ShapesClient::new(app.channel().await);
    let reply = client.lookup(request(4, false)).await.unwrap_or_else(|status| panic!("Lookup failed: {status:?}"));
    assert_eq!(reply.into_inner(), Tick { n: 4 });

    let status = failed(client.lookup(request(4, true)).await);
    // ALREADY_EXISTS is no kind's code: only the status as written carries it.
    assert_eq!((status.code(), status.message()), (Code::AlreadyExists, "tick taken"));
    assert_eq!(offered.0.snapshot(), vec![Some(Code::AlreadyExists)]);
    app.stop().await;
}

#[tokio::test]
async fn a_status_an_error_handler_returns_reaches_the_wire_as_it_stands() {
    let (app, _) = start().await;
    let mut client = ShapesClient::new(app.channel().await);
    let status = failed(client.rescue(request(1, true)).await);
    assert_eq!((status.code(), status.message()), (Code::AlreadyExists, "rescued"));
    app.stop().await;
}

#[tokio::test]
async fn an_error_handler_answers_a_domain_error_with_a_message_of_its_own() {
    let (app, _) = start().await;
    let mut client = ShapesClient::new(app.channel().await);
    let reply = client.substituted(request(5, true)).await.unwrap_or_else(|status| panic!("Substituted failed: {status:?}"));
    assert_eq!(reply.metadata().get("x-substituted").and_then(|value| value.to_str().ok()), Some("yes"));
    assert_eq!(reply.into_inner(), SUBSTITUTE);
    app.stop().await;
}

#[tokio::test]
async fn an_error_handler_answers_a_domain_error_with_a_stream_of_its_own() {
    let (app, _) = start().await;
    let mut client = ShapesClient::new(app.channel().await);
    let reply =
        client.substituted_stream(request(5, true)).await.unwrap_or_else(|status| panic!("SubstitutedStream failed: {status:?}"));
    assert_eq!(collect(reply.into_inner()).await, vec![1, 2, 3]);
    app.stop().await;
}

#[tokio::test]
async fn grpc_metadata_as_a_parameter_is_what_the_caller_sent() {
    let (app, _) = start().await;
    let mut client = ShapesClient::new(app.channel().await);
    let mut named = tonic::Request::new(());
    named.metadata_mut().insert("x-user", "ada".parse().unwrap_or_else(|error| panic!("bad metadata value: {error}")));
    let reply = client.whoami(named).await.unwrap_or_else(|status| panic!("Whoami failed: {status:?}"));
    assert_eq!(reply.into_inner(), "ada");
    let reply = client.whoami(()).await.unwrap_or_else(|status| panic!("Whoami failed: {status:?}"));
    assert_eq!(reply.into_inner(), "nobody");
    app.stop().await;
}
