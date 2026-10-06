//! Scenarios: recovery.

use std::time::{Duration, Instant};

use crate::Broker;
use crate::cases::app::{ADD, Add, Fixture, Sum};

/// Calls succeed again within `Budget::recovery` after `Broker::disrupt`, on a broker.
pub async fn after_disrupt<B: Broker>() {
    let fixture = Fixture::<B>::start().await;
    fixture.broker.disrupt().await;
    let deadline = Instant::now() + fixture.broker.budget().recovery;
    loop {
        let outcome = fixture.rpc().request::<_, Sum>(ADD, &Add { a: 4, b: 5 }).timeout(Duration::from_secs(1)).await;
        match outcome {
            Ok(sum) => {
                assert_eq!(sum, Sum { sum: 9 });
                break;
            }
            outcome if Instant::now() >= deadline => panic!("calls did not recover within the budget: {outcome:?}"),
            _ => tokio::time::sleep(Duration::from_millis(200)).await,
        }
    }
    fixture.stop().await;
}
