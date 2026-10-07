//! Scenarios: unary.

use std::time::Duration;

use ulo_rpc::Link;

use crate::Broker;
use crate::cases::app::{ADD, Add, CONTEXT, Fixture, HEADER, Sum};

/// A unary call answered by one reply.
pub async fn round_trip<B: Broker>() {
    let fixture = Fixture::<B>::start().await;
    let sum = fixture.rpc().request::<_, Sum>(ADD, &Add { a: 2, b: 3 }).timeout(Duration::from_secs(5)).await;
    assert_eq!(sum.expect("the unary call is answered"), Sum { sum: 5 });
    fixture.stop().await;
}

/// A handler taking `cx: RpcCx` as a parameter reads its own call's context.
pub async fn context_parameter<B: Broker>() {
    let fixture = Fixture::<B>::start().await;
    let seen = fixture
        .rpc()
        .request::<_, (String, String, Option<String>)>(CONTEXT, &())
        .header(HEADER, "context-value")
        .timeout(Duration::from_secs(5))
        .await
        .expect("the context call is answered");
    assert_eq!(seen, (CONTEXT.to_owned(), B::Link::NAME.to_owned(), Some("context-value".to_owned())));
    fixture.stop().await;
}
