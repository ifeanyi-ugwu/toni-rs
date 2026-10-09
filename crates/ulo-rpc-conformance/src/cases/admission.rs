//! Scenarios: the server's in-flight bound.

use std::future::IntoFuture;
use std::time::Duration;

use futures_util::future::join_all;
use ulo_rpc::RpcError;
use ulo_transport::{Count, ErrorKind};

use crate::Broker;
use crate::cases::app::{BOUNDED, BOUNDED_TOO, Fixture};

/// The server's bound in this scenario.
const BOUND: u32 = 2;

/// Calls sent at once, half to each bounded pattern.
const CALLS: usize = 6;

/// How long each bounded call holds its place.
const HOLD: Duration = Duration::from_secs(1);

/// Six calls at once, three to each of two patterns, against a server bounded at two calls in
/// flight, each holding its place for a second. A link declaring `native_backpressure` stops
/// taking requests from its broker at the bound, so every call waits there and is answered, never
/// more than two running at once, across both patterns. Any other link refuses what is over the
/// bound: two calls are answered and four refused `unavailable`, each with a `RetryAfter` detail.
pub async fn over_the_bound<B: Broker>() {
    let fixture = Fixture::<B>::start_bounded(Count::Max(BOUND)).await;
    let capabilities = fixture.capabilities();
    let millis = u64::try_from(HOLD.as_millis()).unwrap_or(u64::MAX);
    // Each wave holds for `HOLD`, and a holding link serves the calls in `CALLS / BOUND` waves.
    let wait = HOLD * u32::try_from(CALLS).unwrap_or(u32::MAX) + fixture.broker.budget().settle * 4 + Duration::from_secs(5);
    let calls = (0..CALLS).map(|index| {
        let pattern = if index % 2 == 0 { BOUNDED } else { BOUNDED_TOO };
        fixture.rpc().request::<_, u64>(pattern, &millis).timeout(wait).into_future()
    });
    let answers: Vec<Result<u64, RpcError>> = join_all(calls).await;

    let served = answers.iter().filter(|answer| answer.as_ref().is_ok_and(|held| *held == millis)).count();
    let peak = fixture.server.probe.bounded_peak();
    assert!(peak <= BOUND as usize, "{peak} calls ran at once against a bound of {BOUND}: {answers:?}");
    if capabilities.native_backpressure {
        assert_eq!(served, CALLS, "a link declaring `native_backpressure` did not serve every call over the bound: {answers:?}");
    } else {
        for answer in answers.iter().filter(|answer| answer.is_err()) {
            match answer {
                Err(error) if error.kind() == ErrorKind::Unavailable && error.details().retry_after().is_some() => {}
                other => panic!("a call over the bound was not refused `unavailable` with `RetryAfter`, got: {other:?}"),
            }
        }
        assert_eq!(served, BOUND as usize, "a refusing link did not serve exactly the calls within the bound: {answers:?}");
    }
    fixture.stop().await;
}
