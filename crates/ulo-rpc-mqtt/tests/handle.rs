//! A link built on a thread with no tokio runtime current, and given none with `with_handle`,
//! refuses to connect, and a client built on it is refused where it is built, each naming
//! `.with_handle(..)`, before any I/O.

use std::sync::Arc;

use ulo_rpc::{Link, RpcClient};
use ulo_rpc_mqtt::Mqtt;

#[test]
fn a_link_built_outside_a_runtime_and_given_none_refuses_to_connect() {
    assert!(tokio::runtime::Handle::try_current().is_err(), "the test's thread has a tokio runtime current");
    let link = Mqtt::url("mqtt://127.0.0.1:1883");
    match futures_executor::block_on(link.connect()) {
        Ok(_) => panic!("a link with no tokio runtime connected"),
        Err(refused) => assert!(refused.to_string().contains("`.with_handle(..)`"), "the refusal names `.with_handle(..)`: {refused}"),
    }
}

#[test]
fn a_client_on_a_link_built_outside_a_runtime_and_given_none_is_refused_where_it_is_built() {
    assert!(tokio::runtime::Handle::try_current().is_err(), "the test's thread has a tokio runtime current");
    let runtime = tokio::runtime::Builder::new_current_thread().build().expect("a tokio runtime for the client");
    let built = RpcClient::new(Mqtt::url("mqtt://127.0.0.1:1883"), Arc::new(ulo_tokio::Tokio::from_handle(runtime.handle().clone())));
    match built {
        Ok(_) => panic!("a client was built on a link with no tokio runtime"),
        Err(refused) => assert!(refused.to_string().contains("`.with_handle(..)`"), "the refusal names `.with_handle(..)`: {refused}"),
    }
}
