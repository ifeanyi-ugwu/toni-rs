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
//!
//! On the wire a request is the payload published on the pattern's topic with a Response Topic
//! and Correlation Data, an event the same without them. Each reply message carries one envelope
//! frame, `res`, `err`, `item` or `end`. A streamed request opens with an empty body and the user
//! property `ulo-t: open`; the server answers it on the Response Topic with `ulo-t: opened` before
//! the caller sends anything more, and the request's items, its end and every `cancel` travel on
//! `ulo/rpc/control`, which every server instance subscribes to outside the shared subscription,
//! carrying the call's Correlation Data.

mod link;

pub use link::{Mqtt, QoS};
