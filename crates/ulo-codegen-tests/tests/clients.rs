//! `GrpcClientModule`: the generated client bound under its own type, built through the
//! `GrpcClient` impl `ulo-build` wrote, keyed so two endpoints of one service coexist, its URI
//! checked when the app wires and its channel connected lazily; and `outgoing`, which forwards an
//! execution's deadline as `grpc-timeout`.

mod support;

use std::time::Duration;

use tonic::Code;
use tonic::transport::Channel;
use ulo::{App, Dep, ExecOptions, Module, ModuleDef, ModuleIdentity, Signal, injectable, routes};
use ulo_codegen_tests::probe::clock_client::ClockClient;
use ulo_codegen_tests::probe::known_client::KnownClient;
use ulo_codegen_tests::probe::{self, Tick, Ticks};
use ulo_grpc::{GrpcClientModule, GrpcEndpoint, Message};

use support::{Running, within};

/// The name a server answers `Upper` with, so a caller can tell two servers apart.
struct ServerName(&'static str);

#[injectable]
struct Known {
    name: Dep<ServerName>,
}

#[routes]
impl Known {
    #[ulo_grpc::method(probe::known::Upper)]
    fn upper(&self, text: Message<String>) -> String {
        format!("{}:{}", self.name.0, text.0.to_uppercase())
    }
}

#[injectable]
struct Clock;

#[routes]
impl Clock {
    /// The time left before the call's deadline, in whole milliseconds rounded up; `0` for none.
    #[ulo_grpc::method(probe::clock::Quick)]
    fn quick(&self, _req: Message<Ticks>, exec: ulo::ExecutionRef) -> Tick {
        let left = exec.deadline().map_or(0, |deadline| {
            let left = deadline.saturating_duration_since(tokio::time::Instant::now().into_std());
            u32::try_from(left.as_millis()).unwrap_or(u32::MAX).saturating_add(1)
        });
        Tick { n: left }
    }

    #[ulo_grpc::method(probe::clock::Stall)]
    async fn stall(&self, _req: Message<Ticks>) -> Tick {
        tokio::time::sleep(Duration::from_secs(30)).await;
        Tick { n: 0 }
    }
}

struct ServerRoot(&'static str);

impl Module for ServerRoot {
    fn identity(&self) -> ModuleIdentity {
        ModuleIdentity::of_type::<Self>()
    }

    fn register(&self, m: &mut ModuleDef<'_>) {
        m.value(ServerName(self.0));
        m.controller::<Known>();
        m.controller::<Clock>();
    }
}

async fn server(name: &'static str) -> Running {
    Running::start(ServerRoot(name), ulo_grpc::Server::new("127.0.0.1:0")).await
}

struct Primary;
struct Secondary;

type KnownGrpc = KnownClient<Channel>;

/// Reaches both servers, each through its own keyed client.
#[injectable]
struct Caller {
    primary: Dep<KnownGrpc, Primary>,
    secondary: Dep<KnownGrpc, Secondary>,
}

/// The client app's root: the given client modules, and `Caller` when both keyed ones are among
/// them.
struct ClientRoot {
    imports: Vec<Import>,
}

#[derive(Clone)]
enum Import {
    Plain(String),
    Keyed(String, String),
}

impl Module for ClientRoot {
    fn identity(&self) -> ModuleIdentity {
        ModuleIdentity::of_type::<Self>()
    }

    fn register(&self, m: &mut ModuleDef<'_>) {
        for import in &self.imports {
            match import.clone() {
                Import::Plain(uri) => m.import(GrpcClientModule::<KnownGrpc>::for_root(GrpcEndpoint::new(uri))),
                Import::Keyed(primary, secondary) => {
                    m.import(GrpcClientModule::<KnownGrpc>::for_root(GrpcEndpoint::new(primary)).keyed::<Primary>());
                    m.import(GrpcClientModule::<KnownGrpc>::for_root(GrpcEndpoint::new(secondary)).keyed::<Secondary>());
                    m.provide::<Caller>();
                }
            }
        }
    }
}

/// The client app, connected; it binds no server.
async fn client_app(imports: Vec<Import>) -> App<ulo::Connected> {
    App::builder(ClientRoot { imports })
        .timer(ulo_tokio::Timer)
        .wire()
        .unwrap_or_else(|error| panic!("the client app did not wire: {error}"))
        .connect()
        .await
        .unwrap_or_else(|error| panic!("the client app did not connect: {error}"))
}

async fn upper(client: &KnownGrpc, text: &str) -> Result<String, tonic::Status> {
    let mut client = client.clone();
    within("the Upper call", client.upper(text.to_owned())).await.map(tonic::Response::into_inner)
}

#[tokio::test]
async fn the_bound_client_calls_its_endpoint() {
    let server = server("a").await;
    let app = client_app(vec![Import::Plain(server.origin())]).await;
    let client = app.get::<KnownGrpc>().await.unwrap_or_else(|error| panic!("the client did not resolve: {error}"));
    assert_eq!(upper(&client, "ada").await.map_err(|status| status.code()), Ok("a:ADA".to_owned()));
    let _ = app.close(Signal::new("test")).await;
    server.stop().await;
}

#[tokio::test]
async fn keyed_clients_of_one_service_each_reach_their_own_endpoint() {
    let first = server("first").await;
    let second = server("second").await;
    let app = client_app(vec![Import::Keyed(first.origin(), second.origin())]).await;
    let caller = app.get::<Caller>().await.unwrap_or_else(|error| panic!("Caller did not resolve: {error}"));
    assert_eq!(upper(&caller.primary, "x").await.map_err(|status| status.code()), Ok("first:X".to_owned()));
    assert_eq!(upper(&caller.secondary, "x").await.map_err(|status| status.code()), Ok("second:X".to_owned()));
    let _ = app.close(Signal::new("test")).await;
    first.stop().await;
    second.stop().await;
}

#[tokio::test]
async fn connect_does_no_network_io_and_the_first_call_finds_the_endpoint_down() {
    // A port that was free a moment ago and has no listener now.
    let port = std::net::TcpListener::bind("127.0.0.1:0")
        .and_then(|listener| listener.local_addr())
        .unwrap_or_else(|error| panic!("no free port: {error}"))
        .port();
    let app = client_app(vec![Import::Plain(format!("http://127.0.0.1:{port}"))]).await;
    let client = app.get::<KnownGrpc>().await.unwrap_or_else(|error| panic!("the client did not resolve: {error}"));
    assert_eq!(upper(&client, "ada").await.map_err(|status| status.code()), Err(Code::Unavailable));
    let _ = app.close(Signal::new("test")).await;
}

#[tokio::test]
async fn a_uri_that_does_not_parse_fails_wiring() {
    let wired = App::builder(ClientRoot { imports: vec![Import::Plain("not a uri".to_owned())] }).timer(ulo_tokio::Timer).wire();
    let error = match wired {
        Err(error) => error.to_string(),
        Ok(_) => panic!("an endpoint URI that does not parse wired"),
    };
    assert!(error.contains("`not a uri` is not a gRPC endpoint URI"), "the wiring error does not name the URI: {error}");
}

#[tokio::test]
async fn outgoing_forwards_the_execution_s_remaining_deadline() {
    let server = server("a").await;
    let app = client_app(Vec::new()).await;
    let (http2, origin) = server.plain_http2();
    let mut clock = ClockClient::with_origin(http2, origin);

    let deadline = tokio::time::Instant::now().into_std() + Duration::from_secs(2);
    let left = app
        .execute(ExecOptions::new().deadline(deadline), async |exec| {
            let request = ulo_grpc::outgoing(&exec.handle(), tonic::Request::new(Ticks::default()));
            clock.quick(request).await.map(|reply| reply.into_inner().n)
        })
        .await
        .unwrap_or_else(|error| panic!("the execution was refused: {error}"))
        .unwrap_or_else(|status| panic!("Quick failed: {status:?}"));
    assert!((1..=2000).contains(&left), "{left} ms left of a 2 s deadline");

    // A deadline already passed is sent as the shortest `grpc-timeout`, and the callee fails it at
    // once.
    let passed = tokio::time::Instant::now().into_std();
    let failed = app
        .execute(ExecOptions::new().deadline(passed), async |exec| {
            let request = ulo_grpc::outgoing(&exec.handle(), tonic::Request::new(Ticks::default()));
            within("the call past its deadline", clock.stall(request)).await.map(|reply| reply.into_inner().n)
        })
        .await
        .unwrap_or_else(|error| panic!("the execution was refused: {error}"));
    assert_eq!(failed.map_err(|status| status.code()), Err(Code::DeadlineExceeded));

    let _ = app.close(Signal::new("test")).await;
    server.stop().await;
}
