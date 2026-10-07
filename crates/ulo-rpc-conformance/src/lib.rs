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
//!
//! No scenario passes on silence. The client's own `Timeout` passes only where it is the declared
//! answer: a scenario about the client's timeout or `deadline-ms`, a call nothing takes on a link
//! without `miss_signal`, and a call reaching a draining server on a link that declares
//! `holds_unserved`, where an event emitted then has to reach the next instance. A scenario that cannot apply to a link is declared not applicable
//! in [`conformance_suite!`] and reported as ignored; run on that link, it fails.

use std::future::Future;
use std::time::Duration;

use ulo_rpc::Link;

pub mod cases;
pub mod relay;

/// One link's environment for the suite: a broker, or nothing for TCP and UDP.
pub trait Broker: Sized + Send + Sync + 'static {
    type Link: Link;

    /// Starts the environment for one scenario, so no state leaks between scenarios. Every
    /// scenario uses the same patterns, and on a broker the same default group and control lane,
    /// and the stamped tests run in parallel, so each start answers a broker or a namespace no
    /// other scenario shares: a fresh container, or a fresh port on TCP and UDP.
    fn start() -> impl Future<Output = Self> + Send;

    /// A link for the server, configured for this environment; called once per server instance,
    /// and for the link's `capabilities`, which the scenarios read to choose what they assert.
    /// Every link it answers declares the same capabilities.
    fn link(&self) -> Self::Link;

    /// A link for the client, called once per client: the server's link unless the environment
    /// puts something between the two, such as a proxy `disrupt` cuts. It declares the
    /// capabilities `link` does.
    fn client_link(&self) -> Self::Link {
        self.link()
    }

    /// Severs the client's connection however the environment allows, for the recovery scenario:
    /// the calls waiting on it fail `Unavailable`, and the client connects again for the next.
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
///
/// A scenario that cannot apply to a link is declared with its reason, and stamped
/// `#[ignore = "not applicable: <reason>"]`, so the test report counts it as ignored rather than
/// passed:
///
/// ```ignore
/// ulo_rpc_conformance::conformance_suite!(UdpLoopback; not_applicable {
///     recovery_after_disrupt: "UDP holds no connection to lose",
/// });
/// ```
///
/// A declared name that is no scenario fails to compile. A scenario run on a link it does not
/// apply to fails rather than passing, so a link leaving out a declaration it needs fails too;
/// `cargo test -- --ignored` runs the declared ones, which then fail the same way.
#[macro_export]
macro_rules! conformance_suite {
    ($broker:ty $(,)?) => {
        $crate::conformance_suite!($broker; not_applicable {});
    };
    ($broker:ty; not_applicable { $($skip:ident : $why:literal),* $(,)? } $(,)?) => {
        $crate::conformance_suite!(@stamper [$] $broker; $($skip : $why),*);
    };
    (@stamper [$d:tt] $broker:ty; $($skip:ident : $why:literal),*) => {
        macro_rules! __ulo_rpc_conformance_stamp {
            $(
                ($skip, $d($d test:tt)*) => {
                    #[::tokio::test(flavor = "multi_thread")]
                    #[ignore = concat!("not applicable: ", $why)]
                    $d($d test)*
                };
            )*
            ($d other:ident, $d($d test:tt)*) => {
                #[::tokio::test(flavor = "multi_thread")]
                $d($d test)*
            };
        }
        // A declared name that is no scenario names no function here.
        $(const _: fn() = $skip;)*
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
            __ulo_rpc_conformance_stamp!($name, async fn $name() {
                $crate::cases::$module::$case::<$broker>().await;
            });
        )*
    };
}
