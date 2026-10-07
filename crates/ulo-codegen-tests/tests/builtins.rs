//! The services the gRPC server answers beside the handlers, each called through its own tonic
//! client: `grpc.health.v1`, SERVING for every service a handler's path names after `bind`,
//! settable through `GrpcHealth::module`, NOT_SERVING from the drain on; and
//! `grpc.reflection.v1` and `v1alpha` over the descriptor set `ulo-build` wrote, absent under
//! `reflection(false)`.

mod support;

use futures_util::{StreamExt, stream};
use prost::Message as _;
use tonic::Code;
use tonic_health::ServingStatus;
use tonic_health::pb::health_check_response::ServingStatus as Wire;
use tonic_health::pb::health_client::HealthClient;
use tonic_health::pb::{HealthCheckRequest, HealthCheckResponse};
use ulo::{Module, ModuleDef, ModuleIdentity, injectable, routes};
use ulo_codegen_tests::probe::{self, Tick, Ticks};
use ulo_grpc::{GrpcHealth, Message};

use support::{Running, within};

const KNOWN: &str = "codegen.probe.v1.Known";
const VAULT: &str = "codegen.probe.v1.Vault";

#[injectable]
struct Known;

#[routes]
impl Known {
    #[ulo_grpc::method(probe::known::Upper)]
    fn upper(&self, text: Message<String>) -> String {
        text.0.to_uppercase()
    }
}

#[injectable]
struct Vault;

#[routes]
impl Vault {
    #[ulo_grpc::method(probe::vault::Open)]
    fn open(&self, req: Message<Ticks>) -> Tick {
        Tick { n: req.0.count }
    }
}

/// The app, with `GrpcHealth::module` imported when `health_module` is set.
struct Root {
    health_module: bool,
}

impl Module for Root {
    fn identity(&self) -> ModuleIdentity {
        ModuleIdentity::of_type::<Self>()
    }

    fn register(&self, m: &mut ModuleDef<'_>) {
        if self.health_module {
            m.import(GrpcHealth::module());
        }
        m.controller::<Known>();
        m.controller::<Vault>();
    }
}

async fn start(health_module: bool, reflection: bool) -> Running {
    let server = ulo_grpc::Server::new("127.0.0.1:0").file_descriptor_set(probe::FILE_DESCRIPTOR_SET).reflection(reflection);
    Running::start(Root { health_module }, server).await
}

async fn check(client: &mut HealthClient<tonic::transport::Channel>, service: &str) -> Result<Wire, tonic::Status> {
    let reply = client.check(HealthCheckRequest { service: service.to_owned() }).await?;
    Ok(reply.into_inner().status())
}

#[tokio::test]
async fn health_reports_serving_for_every_known_service_and_the_server() {
    let app = start(false, true).await;
    let mut client = HealthClient::new(app.channel().await);
    for service in [KNOWN, VAULT, ""] {
        assert_eq!(check(&mut client, service).await.map_err(|status| status.code()), Ok(Wire::Serving), "service {service:?}");
    }
    let unknown = check(&mut client, "codegen.probe.v1.Clock").await;
    assert_eq!(unknown.map_err(|status| status.code()), Err(Code::NotFound), "a service no handler names");
    app.stop().await;
}

#[tokio::test]
async fn a_status_set_through_the_health_module_is_what_health_answers() {
    let app = start(true, true).await;
    let health = app.handle.get::<GrpcHealth>().await.unwrap_or_else(|error| panic!("GrpcHealth did not resolve: {error}"));
    health.set_named(VAULT, ServingStatus::NotServing).await;
    let mut client = HealthClient::new(app.channel().await);
    assert_eq!(check(&mut client, VAULT).await.map_err(|status| status.code()), Ok(Wire::NotServing));
    assert_eq!(check(&mut client, KNOWN).await.map_err(|status| status.code()), Ok(Wire::Serving));
    app.stop().await;
}

#[tokio::test]
async fn health_turns_not_serving_when_the_drain_begins() {
    let app = start(false, true).await;
    let mut client = HealthClient::new(app.channel().await);
    let mut watches = Vec::new();
    for service in [KNOWN, ""] {
        let watch = client.watch(HealthCheckRequest { service: service.to_owned() }).await;
        let mut updates = watch.unwrap_or_else(|status| panic!("Watch {service:?} failed: {status:?}")).into_inner();
        let first = within("the first health update", updates.next()).await;
        assert_eq!(first.map(|update| update.map(|update| update.status())).transpose().ok().flatten(), Some(Wire::Serving), "{service:?}");
        watches.push((service, updates));
    }
    let stopping = tokio::spawn(app.stop());
    for (service, updates) in &mut watches {
        let next: Option<Result<HealthCheckResponse, tonic::Status>> = within("the drain's health update", updates.next()).await;
        assert_eq!(next.map(|update| update.map(|update| update.status()).map_err(|status| status.code())), Some(Ok(Wire::NotServing)), "{service:?}");
    }
    drop(watches);
    within("the app's close once the watches ended", stopping).await.unwrap_or_else(|error| panic!("{error}"));
}

/// The services a reflection client of `version` lists, and the file it finds `symbol` in.
macro_rules! reflect {
    ($name:ident, $version:ident) => {
        async fn $name(app: &Running, symbol: &str) -> Result<(Vec<String>, Vec<String>), tonic::Status> {
            use tonic_reflection::pb::$version::server_reflection_client::ServerReflectionClient;
            use tonic_reflection::pb::$version::server_reflection_request::MessageRequest;
            use tonic_reflection::pb::$version::server_reflection_response::MessageResponse;
            use tonic_reflection::pb::$version::ServerReflectionRequest;

            let request = |message: MessageRequest| ServerReflectionRequest { host: String::new(), message_request: Some(message) };
            let requests = vec![request(MessageRequest::ListServices(String::new())), request(MessageRequest::FileContainingSymbol(symbol.to_owned()))];
            let mut client = ServerReflectionClient::new(app.channel().await);
            let mut replies = client.server_reflection_info(stream::iter(requests)).await?.into_inner();
            let mut services = Vec::new();
            let mut files = Vec::new();
            for _ in 0..2 {
                let reply = within("a reflection reply", replies.next()).await.unwrap_or_else(|| panic!("the reflection stream ended early"))?;
                match reply.message_response {
                    Some(MessageResponse::ListServicesResponse(list)) => services.extend(list.service.into_iter().map(|service| service.name)),
                    Some(MessageResponse::FileDescriptorResponse(found)) => {
                        for encoded in found.file_descriptor_proto {
                            let file = prost_types::FileDescriptorProto::decode(encoded.as_slice())
                                .unwrap_or_else(|error| panic!("reflection answered a file descriptor that does not decode: {error}"));
                            files.push(file.name.unwrap_or_default());
                        }
                    }
                    other => panic!("unexpected reflection reply {other:?}"),
                }
            }
            Ok((services, files))
        }
    };
}

reflect!(reflect_v1, v1);
reflect!(reflect_v1alpha, v1alpha);

fn assert_reflected(version: &str, reflected: Result<(Vec<String>, Vec<String>), tonic::Status>) {
    let (services, files) = reflected.unwrap_or_else(|status| panic!("reflection {version} failed: {status:?}"));
    for service in [KNOWN, VAULT, "codegen.probe.v1.Clock", "codegen.v1.Counter", "Bare"] {
        assert!(services.iter().any(|listed| listed == service), "reflection {version} does not list {service}: {services:?}");
    }
    assert!(files.iter().any(|file| file == "probe.proto"), "reflection {version} found `{VAULT}` in {files:?}");
}

#[tokio::test]
async fn reflection_v1_lists_the_descriptor_set_s_services_and_finds_a_symbol_s_file() {
    let app = start(false, true).await;
    assert_reflected("v1", reflect_v1(&app, VAULT).await);
    app.stop().await;
}

#[tokio::test]
async fn reflection_v1alpha_lists_the_descriptor_set_s_services_and_finds_a_symbol_s_file() {
    let app = start(false, true).await;
    assert_reflected("v1alpha", reflect_v1alpha(&app, VAULT).await);
    app.stop().await;
}

#[tokio::test]
async fn reflection_off_is_unimplemented_and_health_still_answers() {
    let app = start(false, false).await;
    let refused = reflect_v1(&app, VAULT).await;
    assert_eq!(refused.map_err(|status| status.code()).map(|_| ()), Err(Code::Unimplemented));
    let mut client = HealthClient::new(app.channel().await);
    assert_eq!(check(&mut client, KNOWN).await.map_err(|status| status.code()), Ok(Wire::Serving));
    app.stop().await;
}
