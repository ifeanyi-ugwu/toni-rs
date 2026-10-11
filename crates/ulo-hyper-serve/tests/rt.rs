//! hyper's runtime edges over the app's, on smol: HTTP/2's stream tasks spawned through
//! `RuntimeExecutor`, the HTTP/1.1 header-read timeout timed by `RuntimeTimer`, and a deadline
//! measured on the app's clock rather than the system's.

mod support;

use std::convert::Infallible;
use std::sync::{Arc, Mutex, PoisonError};
use std::time::{Duration, Instant};

use async_net::{TcpListener, TcpStream};
use bytes::Bytes;
use futures::{AsyncReadExt, AsyncWriteExt};
use http_body_util::{BodyExt, Empty, Full};
use hyper::rt::Timer as _;
use hyper::server::conn::{http1, http2};
use ulo::{BoxFuture, Runtime, Timer};
use ulo_hyper_serve::{FuturesIo, RuntimeExecutor, RuntimeTimer};

use support::{Counting, on_smol, within};

/// One accepted connection and the client's end of it.
async fn pair() -> (TcpStream, TcpStream) {
    let listener = TcpListener::bind("127.0.0.1:0").await.expect("a port");
    let addr = listener.local_addr().expect("its address");
    let client = TcpStream::connect(addr).await.expect("the listener accepts");
    let (server, _) = listener.accept().await.expect("the connection");
    (server, client)
}

#[test]
fn http2_s_tasks_are_spawned_through_the_runtime_executor() {
    on_smol(|smol| async move {
        let counting = Counting::new(smol.clone());
        let server_runtime: Arc<dyn Runtime> = Arc::new(counting.clone());
        let client_runtime: Arc<dyn Runtime> = Arc::new(smol);
        let (server, client) = pair().await;
        let service = hyper::service::service_fn(|_req| async { Ok::<_, Infallible>(http::Response::new(Full::new(Bytes::from("over h2")))) });
        let serving = http2::Builder::new(RuntimeExecutor::new(Arc::clone(&server_runtime))).serve_connection(FuturesIo::new(server), service);
        drop(client_runtime.spawn(Box::pin(async move {
            let _ = serving.await;
        })));

        let (mut sender, connection) =
            hyper::client::conn::http2::handshake(RuntimeExecutor::new(Arc::clone(&client_runtime)), FuturesIo::new(client))
                .await
                .expect("the client's handshake");
        drop(client_runtime.spawn(Box::pin(async move {
            let _ = connection.await;
        })));
        let request = http::Request::get("http://rt.test/").body(Empty::<Bytes>::new()).expect("a request");
        let response = within("the HTTP/2 answer", sender.send_request(request)).await.expect("an answer");
        assert_eq!(response.status().as_u16(), 200);
        let body = within("the HTTP/2 body", response.into_body().collect()).await.expect("the body").to_bytes();
        assert_eq!(body, Bytes::from("over h2"));
        assert!(counting.spawned() >= 1, "the server's HTTP/2 stream ran without a task spawned through its executor");
    });
}

#[test]
fn the_header_read_timeout_is_timed_by_the_runtime_timer() {
    on_smol(|smol| async move {
        let runtime: Arc<dyn Runtime> = Arc::new(smol);
        let (server, mut client) = pair().await;
        let mut builder = http1::Builder::new();
        builder.timer(RuntimeTimer::new(Arc::clone(&runtime) as Arc<dyn Timer>)).header_read_timeout(Duration::from_millis(150));
        let service = hyper::service::service_fn(|_req| async { Ok::<_, Infallible>(http::Response::new(Full::new(Bytes::from("never")))) });
        let serving = builder.serve_connection(FuturesIo::new(server), service);
        drop(runtime.spawn(Box::pin(async move {
            let _ = serving.await;
        })));

        client.write_all(b"GET / HTTP/1.1\r\nHost: rt.test\r\n").await.expect("half a head is written");
        let mut answered = Vec::new();
        within("the server closing a connection whose head stopped arriving", client.read_to_end(&mut answered)).await.ok();
        assert!(
            !String::from_utf8_lossy(&answered).contains("never"),
            "a request whose head never finished was answered: {:?}",
            String::from_utf8_lossy(&answered)
        );
    });
}

/// A clock an hour ahead of the system's, recording each sleep it is asked for and answering at
/// once.
#[derive(Clone, Default)]
struct Ahead {
    asked: Arc<Mutex<Vec<Duration>>>,
}

const AHEAD: Duration = Duration::from_secs(3600);

impl Timer for Ahead {
    fn sleep(&self, d: Duration) -> BoxFuture<'static, ()> {
        self.asked.lock().unwrap_or_else(PoisonError::into_inner).push(d);
        Box::pin(async {})
    }

    fn now(&self) -> Instant {
        Instant::now() + AHEAD
    }
}

#[test]
fn a_deadline_is_measured_on_the_app_s_clock() {
    on_smol(|_smol| async move {
        let ahead = Ahead::default();
        let timer = RuntimeTimer::new(Arc::new(ahead.clone()));
        let now = timer.now();
        assert!(now >= Instant::now() + AHEAD - Duration::from_secs(1), "`now` is not the app clock's");
        timer.sleep_until(now + Duration::from_millis(50)).await;
        let asked = ahead.asked.lock().unwrap_or_else(PoisonError::into_inner).clone();
        assert_eq!(asked.len(), 1, "one sleep for one deadline: {asked:?}");
        assert!(
            asked[0] <= Duration::from_millis(50),
            "a deadline 50 ms ahead on the app's clock asked it for {:?}, measured against another clock",
            asked[0]
        );
    });
}
