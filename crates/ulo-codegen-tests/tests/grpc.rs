//! `#[ulo_grpc::method]` handlers bound to the markers `ulo-build` generated, one per call shape
//! and one on a generic controller for a second service, served by `ulo_grpc::Server` and called
//! through the clients `ulo-build` generated.

use std::error::Error;
use std::fmt;
use std::marker::PhantomData;
use std::net::SocketAddr;

use futures_util::{Stream, StreamExt, stream};
use tonic::transport::{Channel, Endpoint};
use ulo::app::Bound as Serving;
use ulo::{App, AppHandle, Module, ModuleDef, ModuleIdentity, Signal, injectable, routes};
use ulo_codegen_tests::pb::counter_client::CounterClient;
use ulo_codegen_tests::pb::scale_client::ScaleClient;
use ulo_codegen_tests::pb::{self, AddRequest, CountRequest, Number, Sum};
use ulo_grpc::{GrpcClient, Message, Request, Response, Streaming};
use ulo_transport::Classify;

/// The metadata key `add` echoes from the request onto its reply.
const ECHO: &str = "x-echo";

/// A handler's own failure: a negative operand, or a streamed item that did not decode.
#[derive(Debug, Classify)]
#[classify(bad_request)]
struct Refusal;

impl fmt::Display for Refusal {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str("the counter refused the input")
    }
}

impl Error for Refusal {}

#[injectable]
struct CounterService;

#[routes]
impl CounterService {
    #[ulo_grpc::method(pb::counter::Add)]
    async fn add(&self, req: Request<AddRequest>) -> Result<Response<Sum>, Refusal> {
        let AddRequest { a, b } = *req.message();
        if a < 0 || b < 0 {
            return Err(Refusal);
        }
        let echo = req.metadata().get(ECHO).unwrap_or("").to_owned();
        Ok(Response::new(Sum { sum: a + b }).metadata(ECHO, &echo))
    }

    #[ulo_grpc::method(pb::counter::Count)]
    async fn count(&self, req: Message<CountRequest>) -> impl Stream<Item = Result<Number, Refusal>> {
        stream::iter((1..=i64::from(req.0.up_to)).map(|value| Ok(Number { value })))
    }

    #[ulo_grpc::method(pb::counter::Total)]
    async fn total(&self, items: Streaming<Number>) -> Result<Sum, Refusal> {
        let mut items = items;
        let mut sum = 0;
        while let Some(item) = items.next().await {
            sum += item.map_err(|_| Refusal)?.value;
        }
        Ok(Sum { sum })
    }

    #[ulo_grpc::method(pb::counter::Double)]
    fn double(&self, items: Streaming<Number>) -> impl Stream<Item = Result<Number, Refusal>> {
        items.map(|item| item.map(|n| Number { value: n.value * 2 }).map_err(|_| Refusal))
    }
}

/// The factor `Scaled<F>` multiplies by, a type parameter of the controller.
trait Factor: Send + Sync + 'static {
    const BY: i64;
}

struct Triple;

impl Factor for Triple {
    const BY: i64 = 3;
}

/// A generic controller: its hidden methods and the checks inside them sit in a generic impl.
#[injectable]
struct Scaled<F: Factor> {
    #[injectable(default)]
    _factor: PhantomData<F>,
}

#[routes]
impl<F: Factor> Scaled<F> {
    #[ulo_grpc::method(pb::scale::Apply)]
    async fn apply(&self, n: Message<Number>) -> Number {
        Number { value: n.0.value * F::BY }
    }
}

struct Root;

impl Module for Root {
    fn identity(&self) -> ModuleIdentity {
        ModuleIdentity::of_type::<Self>()
    }

    fn register(&self, m: &mut ModuleDef<'_>) {
        m.controller::<CounterService>();
        m.controller::<Scaled<Triple>>();
    }
}

/// The app listening on an ephemeral port, serving until the test ends.
struct Running {
    addr: SocketAddr,
    handle: AppHandle,
    serving: tokio::task::JoinHandle<()>,
}

impl Running {
    async fn start() -> Running {
        let server = ulo_grpc::Server::new("127.0.0.1:0").file_descriptor_set(pb::FILE_DESCRIPTOR_SET);
        let app: App<Serving> = App::builder(Root)
            .timer(ulo_tokio::Timer)
            .wire()
            .unwrap_or_else(|error| panic!("the gRPC app did not wire: {error}"))
            .connect()
            .await
            .unwrap_or_else(|error| panic!("the gRPC app did not connect: {error}"))
            .bind(server)
            .listen()
            .await
            .unwrap_or_else(|error| panic!("the gRPC app did not listen: {error}"));
        let addr = match app.addresses().as_slice() {
            [bound] => bound.addr,
            other => panic!("expected one bound address, got {other:?}"),
        };
        let handle = app.handle();
        let serving = tokio::spawn(async move {
            let _ = app.serve(std::future::pending::<Signal>()).await;
        });
        Running { addr, handle, serving }
    }

    /// The generated client over a connected channel, built through its `GrpcClient` impl.
    async fn client<C: GrpcClient>(&self) -> C {
        let channel = Endpoint::from_shared(format!("http://{}", self.addr))
            .unwrap_or_else(|error| panic!("bad endpoint: {error}"))
            .connect()
            .await
            .unwrap_or_else(|error| panic!("the client did not connect to {}: {error}", self.addr));
        C::from_channel(channel)
    }

    async fn stop(self) {
        let _ = self.handle.close(Signal::new("test")).await;
        let _ = self.serving.await;
    }
}

fn numbers(values: &[i64]) -> impl Stream<Item = Number> + Send + 'static {
    stream::iter(values.iter().map(|&value| Number { value }).collect::<Vec<_>>())
}

#[tokio::test]
async fn unary_answers_the_message_and_its_reply_metadata() {
    let app = Running::start().await;
    let mut client: CounterClient<Channel> = app.client().await;
    let mut request = tonic::Request::new(AddRequest { a: 2, b: 3 });
    request.metadata_mut().insert(ECHO, "ping".parse().unwrap_or_else(|error| panic!("bad metadata value: {error}")));
    let reply = client.add(request).await.unwrap_or_else(|status| panic!("Add failed: {status:?}"));
    assert_eq!(reply.metadata().get(ECHO).and_then(|value| value.to_str().ok()), Some("ping"));
    assert_eq!(reply.into_inner(), Sum { sum: 5 });
    app.stop().await;
}

#[tokio::test]
async fn unary_error_answers_its_kind_as_the_status_code() {
    let app = Running::start().await;
    let mut client: CounterClient<Channel> = app.client().await;
    let status = match client.add(AddRequest { a: -1, b: 3 }).await {
        Err(status) => status,
        Ok(reply) => panic!("expected InvalidArgument, got {:?}", reply.into_inner()),
    };
    assert_eq!(status.code(), tonic::Code::InvalidArgument, "status: {status:?}");
    app.stop().await;
}

#[tokio::test]
async fn server_streaming_answers_each_message() {
    let app = Running::start().await;
    let mut client: CounterClient<Channel> = app.client().await;
    let replies =
        client.count(CountRequest { up_to: 3 }).await.unwrap_or_else(|status| panic!("Count failed: {status:?}"));
    let values: Vec<i64> = replies
        .into_inner()
        .map(|item| item.unwrap_or_else(|status| panic!("a Count item failed: {status:?}")).value)
        .collect()
        .await;
    assert_eq!(values, vec![1, 2, 3]);
    app.stop().await;
}

#[tokio::test]
async fn client_streaming_reads_every_message() {
    let app = Running::start().await;
    let mut client: CounterClient<Channel> = app.client().await;
    let reply = client.total(numbers(&[4, 5, 6])).await.unwrap_or_else(|status| panic!("Total failed: {status:?}"));
    assert_eq!(reply.into_inner(), Sum { sum: 15 });
    app.stop().await;
}

#[tokio::test]
async fn bidi_answers_each_message_as_it_arrives() {
    let app = Running::start().await;
    let mut client: CounterClient<Channel> = app.client().await;
    let replies = client.double(numbers(&[1, 2, 3])).await.unwrap_or_else(|status| panic!("Double failed: {status:?}"));
    let values: Vec<i64> = replies
        .into_inner()
        .map(|item| item.unwrap_or_else(|status| panic!("a Double item failed: {status:?}")).value)
        .collect()
        .await;
    assert_eq!(values, vec![2, 4, 6]);
    app.stop().await;
}

#[tokio::test]
async fn generic_controller_serves_its_method() {
    let app = Running::start().await;
    let mut client: ScaleClient<Channel> = app.client().await;
    let reply = client.apply(Number { value: 7 }).await.unwrap_or_else(|status| panic!("Apply failed: {status:?}"));
    assert_eq!(reply.into_inner(), Number { value: 21 });
    app.stop().await;
}
