//! The client app's close ends its TCP connection cleanly: `RpcClientModule`'s destroy hook calls
//! the link's `close`, which shuts the socket the client opened, and the peer reads its end with
//! nothing written after the call's frame. The conformance suite's `client_close` scenario asserts
//! the rest on every link: the waiting call failing `Unavailable`, the connection staying closed,
//! and a later call connecting again. Its relay counts a connection reset as ended too, which this
//! raw peer tells apart.

use std::sync::Mutex;
use std::time::Duration;

use tokio::io::AsyncReadExt;
use tokio::net::TcpListener;
use ulo::{App, Bound, Module, ModuleDef, ModuleIdentity, Signal};
use ulo_rpc::{RpcClient, RpcClientModule};
use ulo_rpc_tcp::Tcp;

/// Longer than any wait below, so the call is still waiting at the close and no timeout's `cancel`
/// reaches the peer.
const CALL_TIMEOUT: Duration = Duration::from_secs(30);

/// How long the close has to show at the peer.
const PATIENCE: Duration = Duration::from_secs(5);

struct ClientRoot {
    link: Mutex<Option<Tcp>>,
}

impl Module for ClientRoot {
    fn identity(&self) -> ModuleIdentity {
        ModuleIdentity::of_type::<Self>()
    }

    fn register(&self, m: &mut ModuleDef<'_>) {
        if let Some(link) = self.link.lock().unwrap().take() {
            m.import(RpcClientModule::for_root(link).timeout(Bound::After(CALL_TIMEOUT)));
        }
    }
}

#[tokio::test(flavor = "multi_thread")]
async fn closing_the_client_app_ends_its_connection_cleanly() {
    // A peer that reads the call and never answers it.
    let peer = TcpListener::bind("127.0.0.1:0").await.expect("a loopback port");
    let addr = peer.local_addr().expect("the peer's address");
    let app = App::builder(ClientRoot { link: Mutex::new(Some(Tcp::new(addr))) })
        .runtime(ulo_tokio::Tokio::current())
        .wire()
        .expect("the client app wires")
        .connect()
        .await
        .expect("the client app connects");
    let rpc = (*app.get::<RpcClient>().await.expect("the app holds an `RpcClient`")).clone();
    let _call = tokio::spawn(async move { rpc.request::<String, String>("held", &"x".to_owned()).await });

    let (mut connection, _) = tokio::time::timeout(PATIENCE, peer.accept())
        .await
        .expect("the call connects")
        .expect("the peer accepts");
    let mut prefix = [0u8; 4];
    connection.read_exact(&mut prefix).await.expect("the call's frame arrives");
    let mut frame = vec![0u8; u32::from_be_bytes(prefix) as usize];
    connection.read_exact(&mut frame).await.expect("the call's frame arrives whole");

    if let Err(error) = app.close(Signal::new("client_close")).await {
        panic!("the client app's close reported a failure: {error}");
    }

    let mut rest = [0u8; 1];
    let read = tokio::time::timeout(PATIENCE, connection.read(&mut rest))
        .await
        .unwrap_or_else(|_| panic!("the client's connection stayed open {PATIENCE:?} after its app closed"));
    assert_eq!(read.expect("the connection ends cleanly"), 0, "the connection ends at the app's close, with nothing more sent");
}
