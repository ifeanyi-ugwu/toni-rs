//! MQTT v5 transport for the Toni RPC gateway.
//!
//! MQTT v5 carries request-response natively: a PUBLISH can name a
//! `response_topic` and `correlation_data`, and per-call metadata rides in the
//! v5 `user_properties`. So there is no envelope — the body is raw `RpcData`
//! bytes and the addressing lives in the PUBLISH properties.
//!
//! Both halves drive a rumqttc event loop: the loop must be polled
//! continuously for queued publishes to transmit and for incoming messages
//! (requests on the server, replies on the client) to arrive.
//!
//! - [`MqttAdapter`] — server side; subscribes one topic per registered
//!   pattern and publishes replies to the request's `response_topic`.
//! - [`MqttClientTransport`] — client side; subscribes a private reply topic
//!   and routes replies back by `correlation_data`.

mod mqtt_adapter;
mod mqtt_client_transport;
mod wire;

pub use mqtt_adapter::MqttAdapter;
pub use mqtt_client_transport::MqttClientTransport;
pub use toni::{RpcAdapter, RpcClient, RpcClientTransport};
