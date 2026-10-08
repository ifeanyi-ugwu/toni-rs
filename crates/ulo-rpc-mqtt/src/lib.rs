//! The MQTT v5 link for `ulo-rpc` (transports DESIGN §5.3): a topic per pattern, the payload as
//! the body with headers as user properties, replies through the Response Topic and Correlation
//! Data properties. QoS configurable; ordered per topic and QoS; the maximum packet size from
//! CONNACK; `Competing` through `$share/<group>/`, the group the application's root module's full
//! type path unless `.group(..)` names one, and a CONNACK announcing no shared subscriptions fails
//! `bind`; PUBACK or PUBREC 0x10 maps to `Unavailable` at QoS 1 and 2, and a miss at QoS 0 is the
//! client's `Timeout`; a call waiting when the client's connection drops is `Unavailable`, since a
//! clean session loses what reached the reply topic meanwhile; `mqtts://` selects TLS.
//!
//! The drain unsubscribes and returns once the broker has acknowledged each UNSUBSCRIBE, after
//! every request it routed to the instance before. Both sides announce a Receive Maximum of
//! 65,535, MQTT 5's largest, so the broker holds back no publish for its flow control until that
//! many are unacknowledged; Mosquitto otherwise applies its own `max_inflight_messages`, 20 unset.
//! A request can still be lost at shutdown only when more requests are outstanding to the instance
//! than the broker's flow control allows, the announced 65,535 or a smaller cap the broker applies
//! itself: MQTT 5 §3.10.4 lets the broker deliver a publish it held back after the UNSUBACK, and
//! one read after `close` has queued the DISCONNECT gets no answer, its caller seeing its own
//! `Timeout`.
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
