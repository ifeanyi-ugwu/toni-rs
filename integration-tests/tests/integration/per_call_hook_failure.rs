//! A dispatch target built per call runs its startup hooks on every instance, and a hook
//! returning `Err` fails the call it was built for as a `HookFailed`, which the error chain sees.
//!
//! A singleton's hook `Err` fails startup; a per-call target is built after startup, so the call
//! is the one thing that can fail.

#![allow(dead_code)]

use std::sync::Arc;
use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};
use std::time::Duration;

use serial_test::serial;
use ulo::dispatch::Items;
use ulo::enhancer::Guard;
use ulo::errors::HookFailed;
use ulo::extract::Payload;
use ulo::grpc::extract::Inbound;
use ulo::http::{Body, HttpContext, HttpHandlerResult, HttpResponse};
use ulo::rpc::{RpcContext, RpcData, RpcHandlerResult};
use ulo::{UloFactory, async_trait, catch, injectable, module, on_module_init};
use ulo_macros::{
    controller, get, grpc_methods, message_pattern, patterns, routes, use_error_handlers,
    use_guards,
};

use crate::common::{NotServed, TestServer};

mod hook_pb {
    tonic::include_proto!("ulo_test.orders");
}

use hook_pb::orders_client::OrdersClient;

#[derive(Debug)]
struct NotReady;

impl std::fmt::Display for NotReady {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str("not ready")
    }
}

impl std::error::Error for NotReady {}

/// Fails while `fails` is set, counting every run.
fn flaky(fails: &AtomicBool, runs: &AtomicUsize) -> ulo::di::InitResult {
    runs.fetch_add(1, Ordering::SeqCst);
    if fails.load(Ordering::SeqCst) {
        Err(Box::new(NotReady))
    } else {
        Ok(())
    }
}

static HTTP_FAILS: AtomicBool = AtomicBool::new(true);
static HTTP_INITS: AtomicUsize = AtomicUsize::new(0);

#[injectable]
pub struct NeedsKey {}

#[async_trait]
impl Guard<HttpContext> for NeedsKey {
    async fn can_activate(&self, ctx: &HttpContext) -> bool {
        ctx.request().headers.contains_key("x-key")
    }
}

#[controller("/flaky", scope = "execution")]
pub struct FlakyController {}

impl FlakyController {
    #[on_module_init]
    async fn init(&self) -> ulo::di::InitResult {
        flaky(&HTTP_FAILS, &HTTP_INITS)
    }
}

#[routes]
#[use_guards(NeedsKey)]
impl FlakyController {
    #[get("/")]
    fn ok(&self) -> Body {
        Body::text("built".to_string())
    }
}

#[catch(HookFailed)]
async fn http_catcher(err: &HookFailed, _ctx: &HttpContext) -> HttpHandlerResult {
    let mut answer = HttpResponse::new();
    answer.status = 503;
    answer
        .headers
        .push(("x-hook".to_string(), err.hook.to_string()));
    Ok(answer)
}

#[controller("/caught", scope = "execution")]
pub struct CaughtController {}

impl CaughtController {
    #[on_module_init]
    async fn init(&self) -> ulo::di::InitResult {
        Err(Box::new(NotReady))
    }
}

#[routes]
#[use_error_handlers(value http_catcher)]
impl CaughtController {
    #[get("/")]
    fn ok(&self) -> Body {
        Body::text("built".to_string())
    }
}

#[module(controllers: [FlakyController, CaughtController], providers: [NeedsKey])]
impl HttpHookModule {}

async fn http_get(server: &TestServer, path: &str, key: bool) -> reqwest::Response {
    let mut request = server.client().get(server.url(path));
    if key {
        request = request.header("x-key", "1");
    }
    request.send().await.unwrap()
}

#[serial]
#[tokio::test]
async fn a_failed_hook_fails_only_the_http_call_it_was_built_for() {
    let server = TestServer::start(HttpHookModule).await;
    HTTP_FAILS.store(true, Ordering::SeqCst);

    let failed = http_get(&server, "/flaky", true).await;
    assert_eq!(failed.status(), 500);
    let body = failed.text().await.unwrap();
    assert!(body.contains("on_module_init"), "body: {body}");
    assert!(
        !body.contains("not ready"),
        "the hook's error stays out: {body}"
    );
    assert!(
        !body.contains("FlakyController"),
        "the target's path stays out: {body}"
    );

    HTTP_FAILS.store(false, Ordering::SeqCst);
    let built = http_get(&server, "/flaky", true).await;
    assert_eq!(built.status(), 200);
    assert_eq!(built.text().await.unwrap(), "built");
}

#[serial]
#[tokio::test]
async fn a_refused_http_call_never_builds_the_target() {
    let server = TestServer::start(HttpHookModule).await;
    let before = HTTP_INITS.load(Ordering::SeqCst);

    assert_eq!(http_get(&server, "/flaky", false).await.status(), 403);
    assert_eq!(HTTP_INITS.load(Ordering::SeqCst), before);
}

#[serial]
#[tokio::test]
async fn a_failed_http_hook_reaches_the_chain_as_its_event() {
    let server = TestServer::start(HttpHookModule).await;

    let answer = http_get(&server, "/caught", false).await;
    assert_eq!(answer.status(), 503);
    assert_eq!(answer.headers()["x-hook"], "on_module_init");
}

#[catch(HookFailed)]
async fn rpc_catcher(err: &HookFailed, _ctx: &RpcContext) -> RpcHandlerResult {
    Ok(
        RpcData::from_serialize(&serde_json::json!({ "caught": err.hook }))
            .unwrap()
            .into(),
    )
}

#[controller(scope = "execution")]
pub struct FlakyRpc {}

impl FlakyRpc {
    #[on_module_init]
    async fn init(&self) -> ulo::di::InitResult {
        Err(Box::new(NotReady))
    }
}

#[patterns]
impl FlakyRpc {
    #[message_pattern("hook.flaky")]
    async fn flaky(&self) -> RpcHandlerResult {
        Ok(Items::One(RpcData::text("built")))
    }
}

#[controller(scope = "execution")]
pub struct PanickyRpc {}

impl PanickyRpc {
    #[on_module_init]
    async fn init(&self) -> ulo::di::InitResult {
        panic!("the hook panicked")
    }
}

#[patterns]
impl PanickyRpc {
    #[message_pattern("hook.panicky")]
    async fn panicky(&self) -> RpcHandlerResult {
        Ok(Items::One(RpcData::text("built")))
    }
}

#[module(controllers: [FlakyRpc])]
impl CaughtRpcModule {}

#[module(controllers: [PanickyRpc])]
impl PanickyRpcModule {}

async fn rpc_server(module: impl ulo::di::ModuleMetadata + 'static, catcher: bool) -> u16 {
    let (port_tx, port_rx) = tokio::sync::oneshot::channel::<u16>();
    tokio::spawn(async move {
        let mut factory = UloFactory::new();
        if catcher {
            factory.use_global_rpc_error_handler(Arc::new(rpc_catcher));
        }
        let mut app = factory.create_with(module).await.unwrap();
        app.use_rpc_adapter(ulo_rpc_tcp::TcpAdapter::new("127.0.0.1", 0))
            .unwrap();
        let bound = app.bind().await.unwrap();
        let _ = port_tx.send(bound.rpc.expect("rpc must bind").port());
        app.run().await;
    });
    port_rx.await.unwrap()
}

async fn rpc_call(port: u16, pattern: &str) -> serde_json::Value {
    use tokio::io::{AsyncBufReadExt, AsyncWriteExt, BufReader};

    let stream = tokio::net::TcpStream::connect(format!("127.0.0.1:{}", port))
        .await
        .unwrap();
    let (reader, mut writer) = stream.into_split();
    let mut reader = BufReader::new(reader);
    let mut frame = serde_json::json!({"pattern": pattern, "data": {}, "id": "1"}).to_string();
    frame.push('\n');
    writer.write_all(frame.as_bytes()).await.unwrap();

    let mut line = String::new();
    tokio::time::timeout(Duration::from_secs(2), reader.read_line(&mut line))
        .await
        .expect("a reply must arrive")
        .unwrap();
    serde_json::from_str(&line).unwrap()
}

#[serial]
#[tokio::test]
async fn a_failed_rpc_hook_reaches_the_chain_as_its_event() {
    let port = rpc_server(CaughtRpcModule, true).await;

    let reply = rpc_call(port, "hook.flaky").await;
    assert_eq!(
        reply["response"]["caught"], "on_module_init",
        "reply: {reply}"
    );
}

#[serial]
#[tokio::test]
async fn a_panicking_rpc_hook_answers_as_a_panicking_handler() {
    let port = rpc_server(PanickyRpcModule, false).await;

    let reply = rpc_call(port, "hook.panicky").await;
    assert_eq!(reply["response"]["status"], "error", "reply: {reply}");
    assert_eq!(reply["response"]["kind"], "Internal", "reply: {reply}");
}

#[controller(scope = "execution")]
pub struct FlakyGrpcService {}

impl FlakyGrpcService {
    #[on_module_init]
    async fn init(&self) -> ulo::di::InitResult {
        Err(Box::new(NotReady))
    }
}

#[grpc_methods(hook_pb::orders_server::Orders)]
impl FlakyGrpcService {
    #[grpc_method]
    async fn create(
        &self,
        Payload(_req): Payload<hook_pb::CreateOrderRequest>,
    ) -> Result<hook_pb::CreateOrderResponse, NotServed> {
        Err(NotServed)
    }

    #[grpc_stream]
    async fn watch_progress(
        &self,
        Payload(_req): Payload<hook_pb::WatchRequest>,
    ) -> Result<
        impl futures_util::Stream<Item = Result<hook_pb::ProgressEvent, NotServed>> + Send + 'static,
        NotServed,
    > {
        Ok(futures_util::stream::empty())
    }

    #[grpc_method]
    async fn bulk_create(
        &self,
        _inbound: Inbound<hook_pb::CreateOrderRequest>,
    ) -> Result<hook_pb::BulkCreateResponse, NotServed> {
        Err(NotServed)
    }

    #[grpc_stream]
    async fn chat(
        &self,
        _inbound: Inbound<hook_pb::ChatMessage>,
    ) -> Result<
        impl futures_util::Stream<Item = Result<hook_pb::ChatMessage, NotServed>> + Send + 'static,
        NotServed,
    > {
        Ok(futures_util::stream::empty())
    }
}

#[module(controllers: [FlakyGrpcService])]
impl GrpcHookModule {}

#[serial]
#[tokio::test]
async fn a_failed_grpc_hook_fails_the_call_internal() {
    let addr: std::net::SocketAddr = "127.0.0.1:0".parse().unwrap();
    let adapter = ulo_grpc::GrpcAdapter::new(addr);
    let (port_tx, port_rx) = tokio::sync::oneshot::channel::<u16>();
    tokio::spawn(async move {
        let mut app = UloFactory::new().create_with(GrpcHookModule).await.unwrap();
        app.use_grpc_adapter(adapter).unwrap();
        let bound = app.bind().await.unwrap();
        let _ = port_tx.send(bound.grpc.expect("grpc must bind").port());
        app.run().await;
    });
    let port = port_rx.await.unwrap();

    let mut client = OrdersClient::new(
        tonic::transport::Endpoint::from_shared(format!("http://127.0.0.1:{}", port))
            .unwrap()
            .connect()
            .await
            .expect("connect"),
    );

    let err = client
        .create(hook_pb::CreateOrderRequest {
            item: "keyboard".to_string(),
            qty: 1,
        })
        .await
        .expect_err("a call whose target's hook failed must fail");

    assert_eq!(err.code(), tonic::Code::Internal);
    assert!(
        err.message().contains("on_module_init"),
        "{}",
        err.message()
    );
    assert!(!err.message().contains("not ready"), "{}", err.message());
}
