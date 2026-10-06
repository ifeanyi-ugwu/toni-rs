//! Scenarios: unary.

use std::time::Duration;

use crate::Broker;
use crate::cases::app::{ADD, Add, Fixture, Sum};

/// A unary call answered by one reply.
pub async fn round_trip<B: Broker>() {
    let fixture = Fixture::<B>::start().await;
    let sum = fixture.rpc().request::<_, Sum>(ADD, &Add { a: 2, b: 3 }).timeout(Duration::from_secs(5)).await;
    assert_eq!(sum.expect("the unary call is answered"), Sum { sum: 5 });
    fixture.stop().await;
}
