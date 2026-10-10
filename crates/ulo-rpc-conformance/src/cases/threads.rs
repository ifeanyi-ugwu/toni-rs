//! Scenarios: a client called from a thread no runtime runs.

use std::any::Any;
use std::sync::Arc;
use std::time::Duration;

use futures_util::{StreamExt, stream};
use ulo::Runtime;
use ulo_rpc::{Link, RpcClient, RpcError};

use crate::Broker;
use crate::cases::app::{ADD, Add, COUNT, EVENT, Fixture, SUM, Sum, eventually, streams, within};

/// Each call's own timeout.
const WAIT: Duration = Duration::from_secs(10);

/// How long the thread may take for every call it makes.
const PATIENCE: Duration = Duration::from_secs(60);

/// What the thread's calls answered.
struct Answers {
    sum: Result<Sum, RpcError>,
    emitted: Result<(), RpcError>,
    /// `None` on a link whose shapes exclude the streamed ones.
    counted: Option<Result<Vec<Result<u32, RpcError>>, RpcError>>,
    summed: Option<Result<i64, RpcError>>,
}

/// A client built with `RpcClient::new` and called from a plain `std::thread` with no tokio
/// runtime current, driven by `futures::executor::block_on`: a unary call is answered, an event
/// reaches its handler, and on a link carrying the streamed shapes a streamed reply and a streamed
/// request are answered. The link is built where the scenario runs, so it holds the scenario's
/// runtime and runs its I/O there; the client spawns on that runtime too, held by its handle. The
/// client is dropped on the thread.
///
/// A link whose futures need a tokio runtime current where they are polled fails here: the thread
/// panics, and the scenario reports the panic.
pub async fn plain_thread<B: Broker>() {
    let fixture = Fixture::<B>::start().await;
    let link = fixture.broker.client_link(&fixture.server.addresses);
    let carries_streams = streams(&link.capabilities());
    let runtime: Arc<dyn Runtime> = Arc::new(ulo_tokio::Tokio::current());

    let thread = std::thread::spawn(move || {
        assert!(tokio::runtime::Handle::try_current().is_err(), "the client's thread has a tokio runtime current");
        let client = RpcClient::new(link, runtime);
        let answers = futures_executor::block_on(async {
            let sum = client.request::<_, Sum>(ADD, &Add { a: 2, b: 3 }).timeout(WAIT).await;
            let emitted = client.emit(EVENT, &11u32).await;
            let (counted, summed) = if carries_streams {
                let counted = match client.stream::<_, u32>(COUNT, &3u32).timeout(WAIT).await {
                    Ok(replies) => Ok(replies.collect::<Vec<_>>().await),
                    Err(error) => Err(error),
                };
                let summed = client.send_stream::<_, i64>(SUM, stream::iter(vec![1i64, 2, 3, 4])).timeout(WAIT).await;
                (Some(counted), Some(summed))
            } else {
                (None, None)
            };
            Answers { sum, emitted, counted, summed }
        });
        drop(client);
        answers
    });
    let joined = within(PATIENCE, "the client's thread", tokio::task::spawn_blocking(move || thread.join()))
        .await
        .expect("the join of the client's thread completes");
    let answers = joined.unwrap_or_else(|panic| panic!("the client's thread panicked: {}", message(&*panic)));

    assert_eq!(answers.sum.expect("the unary call from the thread is answered"), Sum { sum: 5 });
    answers.emitted.expect("the event from the thread is published");
    let settle = fixture.broker.budget().settle;
    let probe = fixture.server.probe.clone();
    assert!(eventually(settle, || probe.last_event() == Some(11)).await, "the event from the thread did not reach its handler within {settle:?}");
    if let Some(counted) = answers.counted {
        let items: Vec<u32> =
            counted.expect("the stream from the thread opens").into_iter().map(|item| item.expect("every item arrives")).collect();
        assert_eq!(items, vec![1, 2, 3]);
    }
    if let Some(summed) = answers.summed {
        assert_eq!(summed.expect("the streamed request from the thread is answered"), 10);
    }
    fixture.stop().await;
}

fn message(panic: &(dyn Any + Send)) -> String {
    panic
        .downcast_ref::<String>()
        .cloned()
        .or_else(|| panic.downcast_ref::<&str>().map(|text| (*text).to_owned()))
        .unwrap_or_else(|| "a payload that is not text".to_owned())
}
