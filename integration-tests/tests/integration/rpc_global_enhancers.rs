//! Global RPC enhancers, registered on the factory rather than named on a
//! controller.
//!
//! `use_global_rpc_guards`, `use_global_rpc_interceptors` and
//! `use_global_rpc_error_handler` have existed without coverage, so nothing
//! would have said if they stopped reaching the pipeline. Ordering is global,
//! then the controller's, then the method's.

#![allow(dead_code)]

use std::sync::{Arc, Mutex};
use std::time::Duration;

use serial_test::serial;
use toni::async_trait;
use toni::context::RpcContext;
use toni::rpc::{RpcData, RpcError, RpcHandlerOutput, RpcHandlerResult};
use toni::toni_factory::ToniFactory;
use toni::traits_helpers::{ChainError, ErrorHandler, Guard, Interceptor, InterceptorNext};
use toni::{injectable, module};
use toni_macros::{controller, message_pattern, new, patterns, use_guards};

static SEEN: Mutex<Vec<String>> = Mutex::new(Vec::new());

fn record(what: &str) {
    SEEN.lock().unwrap().push(what.to_string());
}

fn seen() -> Vec<String> {
    SEEN.lock().unwrap().clone()
}

struct GlobalGuard;

#[async_trait]
impl Guard<RpcContext> for GlobalGuard {
    async fn can_activate(&self, _ctx: &RpcContext) -> bool {
        record("global:guard");
        true
    }
}

struct DenyingGlobalGuard;

#[async_trait]
impl Guard<RpcContext> for DenyingGlobalGuard {
    async fn can_activate(&self, _ctx: &RpcContext) -> bool {
        record("global:deny");
        false
    }
}

struct GlobalInterceptor;

#[async_trait]
impl Interceptor<RpcContext, RpcHandlerResult> for GlobalInterceptor {
    async fn intercept(
        &self,
        ctx: &RpcContext,
        next: Box<dyn InterceptorNext<RpcContext, RpcHandlerResult>>,
    ) -> RpcHandlerResult {
        record("global:before");
        let answer = next.run(ctx).await;
        record("global:after");
        answer
    }
}

struct GlobalErrorHandler;

#[async_trait]
impl ErrorHandler<RpcContext, RpcData> for GlobalErrorHandler {
    async fn handle_error(&self, _error: ChainError<'_>, _ctx: &RpcContext) -> Option<RpcData> {
        record("global:error_handler");
        Some(RpcData::from_serialize(&serde_json::json!({"claimed": "globally"})).unwrap())
    }
}

#[injectable]
pub struct ControllerGuard {}

#[async_trait]
impl Guard<RpcContext> for ControllerGuard {
    async fn can_activate(&self, _ctx: &RpcContext) -> bool {
        record("controller:guard");
        true
    }
}

#[controller]
pub struct GlobalsRpcController {}

#[patterns]
#[use_guards(ControllerGuard)]
impl GlobalsRpcController {
    #[new]
    pub fn new() -> Self {
        Self {}
    }

    #[message_pattern("globals.echo")]
    async fn echo(&self) -> RpcHandlerResult {
        record("handler");
        Ok(RpcHandlerOutput::Single(
            RpcData::from_serialize(&serde_json::json!({"echo": true})).unwrap(),
        ))
    }

    #[message_pattern("globals.fail")]
    async fn fail(&self) -> RpcHandlerResult {
        record("handler");
        Err(RpcError::Internal("handler said no".into()))
    }
}

#[module(controllers: [GlobalsRpcController], providers: [ControllerGuard])]
impl GlobalsRpcModule {}

async fn boot<F>(configure: F) -> u16
where
    F: FnOnce(&mut ToniFactory) + Send + 'static,
{
    let (port_tx, port_rx) = tokio::sync::oneshot::channel::<u16>();
    let local = tokio::task::LocalSet::new();
    local.spawn_local(async move {
        let mut factory = ToniFactory::new();
        configure(&mut factory);
        let mut app = factory.create_with(GlobalsRpcModule).await.unwrap();
        app.use_rpc_adapter(toni_tcp::TcpAdapter::new("127.0.0.1", 0))
            .unwrap();
        let bound = app.bind().await.unwrap();
        let _ = port_tx.send(bound.rpc.expect("rpc must bind").port());
        app.run().await;
    });
    tokio::task::spawn_local(async move { local.await });
    port_rx.await.expect("RPC server failed to bind")
}

async fn call(port: u16, pattern: &str) -> serde_json::Value {
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
        .expect("the connection must stay readable");
    serde_json::from_str(&line).expect("the reply must be JSON")
}

/// A guard the controller never names still runs, and runs first.
#[serial]
#[tokio_localset_test::localset_test]
async fn a_global_rpc_guard_runs_ahead_of_the_controller_s_own() {
    SEEN.lock().unwrap().clear();

    let port = boot(|f| {
        f.use_global_rpc_guards(Arc::new(GlobalGuard));
    })
    .await;
    call(port, "globals.echo").await;

    assert_eq!(seen(), vec!["global:guard", "controller:guard", "handler"]);
}

/// Rejecting from there answers the caller and never reaches the controller.
#[serial]
#[tokio_localset_test::localset_test]
async fn a_global_rpc_guard_rejecting_stops_the_call() {
    SEEN.lock().unwrap().clear();

    let port = boot(|f| {
        f.use_global_rpc_guards(Arc::new(DenyingGlobalGuard));
    })
    .await;
    let reply = call(port, "globals.echo").await;

    assert_eq!(reply["err"]["status"], "forbidden", "reply: {reply}");
    assert_eq!(seen(), vec!["global:deny"]);
}

#[serial]
#[tokio_localset_test::localset_test]
async fn a_global_rpc_interceptor_wraps_every_handler() {
    SEEN.lock().unwrap().clear();

    let port = boot(|f| {
        f.use_global_rpc_interceptors(Arc::new(GlobalInterceptor));
    })
    .await;
    call(port, "globals.echo").await;

    assert_eq!(
        seen(),
        vec![
            "controller:guard",
            "global:before",
            "handler",
            "global:after"
        ],
        "guards answer before the interceptor chain is entered"
    );
}

#[serial]
#[tokio_localset_test::localset_test]
async fn a_global_rpc_error_handler_claims_what_the_controller_leaves() {
    SEEN.lock().unwrap().clear();

    let port = boot(|f| {
        f.use_global_rpc_error_handler(Arc::new(GlobalErrorHandler));
    })
    .await;
    let reply = call(port, "globals.fail").await;

    assert_eq!(reply["response"]["claimed"], "globally", "reply: {reply}");
    assert!(seen().contains(&"global:error_handler".to_string()));
}
