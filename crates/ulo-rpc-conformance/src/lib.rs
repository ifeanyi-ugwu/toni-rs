//! The conformance suite every `ulo-rpc` link runs (transports DESIGN §5.2): one scenario list,
//! stamped per link by [`conformance_suite!`] from a [`Broker`] the link crate implements in its
//! `tests/`.
//!
//! ```ignore
//! // crates/ulo-rpc-nats/tests/conformance.rs
//! struct NatsBroker { url: String }
//!
//! impl ulo_rpc_conformance::Broker for NatsBroker {
//!     type Link = ulo_rpc_nats::Nats;
//!     async fn start() -> Self { /* a fresh broker or subject space */ }
//!     fn link(&self) -> ulo_rpc_nats::Nats { ulo_rpc_nats::Nats::url(&self.url) }
//!     async fn disrupt(&self) { /* sever the connection */ }
//! }
//!
//! ulo_rpc_conformance::conformance_suite!(NatsBroker);
//! ```
//!
//! A scenario asserts kinds and reason strings, never message text. A scenario a link's
//! capabilities exclude asserts the refusal the capability declares instead: a streamed shape on
//! UDP is refused at startup, a binary payload on a JSON link before any I/O and its handler at
//! startup, a miss on a link without `miss_signal` is the client's `Timeout`, and the
//! two-instance scenario runs one instance on an `Addressed` link, where a second cannot share the
//! address.

use std::future::Future;
use std::time::Duration;

use ulo_rpc::Link;

pub mod cases;

/// One link's environment for the suite: a broker, or nothing for TCP and UDP.
pub trait Broker: Sized + Send + Sync + 'static {
    type Link: Link;

    /// Starts the environment for one scenario, so no state leaks between scenarios. Every
    /// scenario uses the same patterns, and on a broker the same default group and control lane,
    /// and the stamped tests run in parallel, so each start answers a broker or a namespace no
    /// other scenario shares: a fresh container, or a fresh port on TCP and UDP.
    fn start() -> impl Future<Output = Self> + Send;

    /// A link for the half under test, server or client, configured for this environment; called
    /// once per server instance and once per client, and for the link's `capabilities`, which the
    /// scenarios read to choose what they assert. Every link it answers declares the same
    /// capabilities.
    fn link(&self) -> Self::Link;

    /// Severs the link's connection however the broker allows, for the recovery scenario.
    fn disrupt(&self) -> impl Future<Output = ()> + Send;

    /// How long this environment takes, which a slow broker raises.
    fn budget(&self) -> Budget {
        Budget::default()
    }
}

/// How long a scenario waits on its environment.
#[non_exhaustive]
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Budget {
    /// From `start` until the server answers its first call.
    pub boot: Duration,
    /// For a one-way effect to show: an event handled, a subscription in place.
    pub settle: Duration,
    /// From `disrupt` until calls succeed again.
    pub recovery: Duration,
}

impl Budget {
    pub const fn new(boot: Duration, settle: Duration, recovery: Duration) -> Self {
        Budget { boot, settle, recovery }
    }
}

impl Default for Budget {
    fn default() -> Self {
        Budget::new(Duration::from_secs(5), Duration::from_millis(500), Duration::from_secs(10))
    }
}

/// Stamps every scenario as a `#[tokio::test]` for the broker type `$broker`. The invoking crate
/// depends on `tokio` with `macros` and `rt-multi-thread`, which `#[tokio::test]` names.
#[macro_export]
macro_rules! conformance_suite {
    ($broker:ty) => {
        $crate::conformance_suite!(@cases $broker;
            unary_round_trip => unary::round_trip,
            domain_error_envelope => errors::domain_error,
            guard_refusal_is_forbidden => errors::guard_refusal,
            panic_is_internal => errors::panic,
            undecodable_payload_is_bad_request => errors::undecodable_payload,
            headers_reach_call_headers => headers::reach_call_headers,
            event_reaches_its_handler => events::reaches_handler,
            unhandled_pattern => misses::unhandled_pattern,
            unhandled_event_is_acknowledged => misses::unhandled_event,
            server_stream_in_order => streams::server_stream,
            client_stream => streams::client_stream,
            bidi_stream => streams::bidi_stream,
            cancel_mid_stream => cancel::mid_stream,
            deadline_ms_fires_deadline => deadlines::deadline_ms,
            client_timeout_is_timeout => deadlines::client_timeout,
            binary_payload => payloads::binary,
            oversized_payload => payloads::oversized,
            drain => drain::drain,
            recovery_after_disrupt => recovery::after_disrupt,
            two_instances => delivery::two_instances,
        );
    };
    (@cases $broker:ty; $($name:ident => $module:ident :: $case:ident),* $(,)?) => {
        $(
            #[::tokio::test(flavor = "multi_thread")]
            async fn $name() {
                $crate::cases::$module::$case::<$broker>().await;
            }
        )*
    };
}
