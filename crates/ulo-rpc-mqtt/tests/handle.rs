//! A link built on a thread with no tokio runtime current, and given none with `with_handle`,
//! refuses to connect, naming `.with_handle(..)`, before any I/O.

use ulo_rpc::Link;
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
