//! An adapter built on a thread with no tokio runtime current, and given none with `with_handle`,
//! fails its publish naming `.with_handle(..)` and ends its subscription at once, before any I/O.

use bytes::Bytes;
use futures_util::StreamExt;
use ulo_ws::{Audience, BroadcastAdapter, NodeId, Target};
use ulo_ws_redis::Redis;

#[test]
fn an_adapter_built_outside_a_runtime_and_given_none_refuses() {
    assert!(tokio::runtime::Handle::try_current().is_err(), "the test's thread has a tokio runtime current");
    let adapter = Redis::url("redis://127.0.0.1:6379");
    let published = futures_executor::block_on(adapter.publish(Target::new(None, Audience::All, Vec::new()), Bytes::from_static(b"{}")));
    let refused = published.expect_err("an adapter with no tokio runtime published");
    assert!(refused.to_string().contains("`.with_handle(..)`"), "the refusal names `.with_handle(..)`: {refused}");
    assert!(futures_executor::block_on(adapter.subscribe(NodeId::current()).next()).is_none(), "the subscription of an adapter with no runtime yielded");
}
