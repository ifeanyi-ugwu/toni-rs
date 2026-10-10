//! The tokio runtime a TCP link holds: one built on a thread with no runtime current takes the one
//! `with_handle` names and works from that thread, and one given none refuses, naming
//! `.with_handle(..)` before any I/O: in `connect`, as a server in `prepare`, as a client where
//! `RpcClient::new` builds it, and in a client module's init hook, failing the app's `connect`.

use std::error::Error;
use std::fmt;
use std::sync::Arc;
use std::thread;
use std::time::Duration;

use tokio::runtime::Handle;
use std::sync::Mutex;

use ulo::{App, Bound, ConnectError, FailureReason, Module, ModuleDef, ModuleIdentity, Signal, StartupError, injectable, routes};
use ulo_rpc::{Link, Payload, RpcClient, RpcClientModule};
use ulo_rpc_tcp::Tcp;
use ulo_tokio::Tokio;
use ulo_transport::{Classify, ErrorKind};

#[derive(Debug)]
struct Never;

impl fmt::Display for Never {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str("never raised")
    }
}

impl Error for Never {}

impl Classify for Never {
    fn classify(&self) -> ErrorKind {
        ErrorKind::Internal
    }
}

#[injectable]
struct Adder;

#[routes]
impl Adder {
    #[ulo_rpc::message("handle.add")]
    async fn add(&self, n: Payload<u32>) -> Result<u32, Never> {
        Ok(n.0 + 1)
    }
}

struct ServerRoot;

impl Module for ServerRoot {
    fn identity(&self) -> ModuleIdentity {
        ModuleIdentity::of_type::<Self>()
    }

    fn register(&self, m: &mut ModuleDef<'_>) {
        m.controller::<Adder>();
    }
}

#[test]
fn a_link_built_outside_a_runtime_and_given_none_refuses_to_connect() {
    assert!(Handle::try_current().is_err(), "the test's thread has a tokio runtime current");
    let link = Tcp::new("127.0.0.1:7000");
    match futures_executor::block_on(link.connect()) {
        Ok(_) => panic!("a link with no tokio runtime connected"),
        Err(refused) => assert!(refused.to_string().contains("`.with_handle(..)`"), "the refusal names `.with_handle(..)`: {refused}"),
    }
}

#[test]
fn a_client_on_a_link_built_outside_a_runtime_and_given_none_is_refused_where_it_is_built() {
    assert!(Handle::try_current().is_err(), "the test's thread has a tokio runtime current");
    let runtime = tokio::runtime::Builder::new_current_thread().build().expect("a tokio runtime for the client");
    let built = RpcClient::new(Tcp::new("127.0.0.1:7000"), Arc::new(Tokio::from_handle(runtime.handle().clone())));
    match built {
        Ok(_) => panic!("a client was built on a link with no tokio runtime"),
        Err(refused) => assert!(refused.to_string().contains("`.with_handle(..)`"), "the refusal names `.with_handle(..)`: {refused}"),
    }
}

/// Imports one `RpcClientModule` over the link it is given.
struct ClientRoot(Mutex<Option<Tcp>>);

impl Module for ClientRoot {
    fn identity(&self) -> ModuleIdentity {
        ModuleIdentity::of_type::<Self>()
    }

    fn register(&self, m: &mut ModuleDef<'_>) {
        if let Some(link) = self.0.lock().expect("the root's lock is not poisoned").take() {
            m.import(RpcClientModule::for_root(link));
        }
    }
}

#[tokio::test(flavor = "multi_thread")]
async fn a_client_module_on_a_link_built_outside_a_runtime_and_given_none_fails_connect() {
    let link = thread::spawn(|| Tcp::new("127.0.0.1:7000")).join().expect("building the link on a plain thread panicked");
    let wired = App::builder(ClientRoot(Mutex::new(Some(link)))).runtime(Tokio::current()).wire().expect("the client wires");
    match wired.connect().await {
        Err(StartupError::Connect(ConnectError::Hook { reason: FailureReason::Errored(refused), .. })) => {
            assert!(refused.to_string().contains("`.with_handle(..)`"), "the refusal names `.with_handle(..)`: {refused}");
        }
        Err(other) => panic!("expected the client module's init hook to fail `connect`, got: {other}"),
        Ok(app) => {
            let _ = app.close(Signal::new("handle")).await;
            panic!("a client module over a link with no tokio runtime connected");
        }
    }
}

#[tokio::test(flavor = "multi_thread")]
async fn a_server_link_built_outside_a_runtime_and_given_none_is_refused_in_prepare() {
    let link = thread::spawn(|| Tcp::new("127.0.0.1:0")).join().expect("building the link on a plain thread panicked");
    let bound = App::builder(ServerRoot)
        .runtime(Tokio::current())
        .wire()
        .expect("the server wires")
        .connect()
        .await
        .expect("the server connects")
        .bind(ulo_rpc::Server::new(link))
        .listen()
        .await;
    match bound {
        Err(StartupError::Configure(refused)) => {
            assert!(refused.to_string().contains("`.with_handle(..)`"), "the refusal names `.with_handle(..)`: {refused}");
        }
        Err(other) => panic!("expected a `Configure` refusal, got: {other}"),
        Ok(app) => {
            let _ = app.handle().close(Signal::new("handle")).await;
            panic!("a server link with no tokio runtime listened");
        }
    }
}

#[tokio::test(flavor = "multi_thread")]
async fn a_link_given_a_handle_on_a_plain_thread_is_called_from_there() {
    let server = App::builder(ServerRoot)
        .runtime(Tokio::current())
        .wire()
        .expect("the server wires")
        .connect()
        .await
        .expect("the server connects")
        .bind(ulo_rpc::Server::new(Tcp::new("127.0.0.1:0")))
        .listen()
        .await
        .expect("the server binds");
    let addr = server.addresses()[0].addr;
    let handle = server.handle();
    let serving = tokio::spawn(server.serve(std::future::pending::<Signal>()));

    let runtime = Handle::current();
    let thread = thread::spawn(move || {
        assert!(Handle::try_current().is_err(), "the plain thread has a tokio runtime current");
        let link = Tcp::new(addr).with_handle(runtime.clone());
        let client = RpcClient::new(link, Arc::new(Tokio::from_handle(runtime)))
            .expect("a link given a handle is usable")
            .timeout(Bound::After(Duration::from_secs(5)))
            .expect("a nonzero timeout is taken");
        futures_executor::block_on(async { client.request::<_, u32>("handle.add", &41u32).await })
    });
    let answer = tokio::task::spawn_blocking(move || thread.join()).await.expect("the join completes");
    assert_eq!(answer.expect("the plain thread panicked").expect("the call is answered"), 42);

    let _ = handle.close(Signal::new("handle")).await;
    let _ = serving.await;
}
