//! The MQTT v5 link for `ulo-rpc` (transports DESIGN §5.3): a topic per pattern, the payload as
//! the body with headers as user properties, replies through the Response Topic and Correlation
//! Data properties. QoS configurable; ordered per topic and QoS; the maximum packet size from
//! CONNACK; `Competing` through `$share/<group>/`, the group the application's root module's full
//! type path unless `.group(..)` names one, and a CONNACK announcing no shared subscriptions fails
//! `bind`; PUBACK or PUBREC 0x10 maps to `Unavailable` at QoS 1 and 2, and a miss at QoS 0 is the
//! client's `Timeout`; the drain unsubscribes; `mqtts://` selects TLS.
//!
//! ```ignore
//! app.bind(ulo_rpc::Server::new(ulo_rpc_mqtt::Mqtt::url("mqtt://broker:1883")))
//! ```

mod link;

pub use link::{Mqtt, QoS};
