//! The embedding conformance suite (transports DESIGN §3.8): one scenario list run against the
//! hyper backend, the reference [`Host`], and against every embedding adapter in both modes,
//! nested and fallback, stamped per host by [`http_conformance_suite!`].
//!
//! ```ignore
//! // crates/ulo-http-axum/tests/conformance.rs
//! struct AxumHost { /* the host server, its address, the app */ }
//!
//! impl ulo_http_conformance::Host for AxumHost {
//!     async fn start(app: App<Connected>, mode: Mode) -> Self { /* bind Embedded<Axum>, listen, serve the router */ }
//!     fn base_url(&self) -> String { /* .. */ }
//!     fn limits() -> EmbedLimits { <ulo_http_axum::Axum as Embed>::limits() }
//!     async fn stop(self) { /* .. */ }
//! }
//!
//! ulo_http_conformance::http_conformance_suite!(AxumHost);
//! ```
//!
//! The suite asserts byte-identical responses wherever a scenario depends on no declared limit:
//! status, the headers the app writes and the body, compared against the reference host in the
//! same mode. `Routing` is asserted apart, through the header [`ROUTING_HEADER`] a host's test
//! middleware writes. The limits are checked in both directions: a host that passes a scenario it
//! declares unsupported fails, and one declaring `forward_miss` that answers a `Forwardable` 404
//! itself fails the same way.
//!
//! No scenario passes on silence. A request the client's own timeout ends fails the scenario,
//! since it is neither an answer nor a refusal, and so does a wait for a close or a frame that
//! does not arrive in time. A scenario that cannot apply to a host is declared not applicable in
//! [`http_conformance_suite!`] and reported as ignored; run on that host, it fails.

use std::error::Error;
use std::future::Future;
use std::time::Duration;

use ulo::App;
use ulo::app::Connected;
use ulo_http::Routing;
use ulo_http::embed::EmbedLimits;

mod app;
pub mod cases;
mod reference;
mod wire;

#[cfg(feature = "rocket")]
pub mod rocket_fairing;

pub use app::{HOST_VALUE_HEADER, HostValue, ORIGIN};
pub use reference::HyperHost;

/// Where the host mounts the app.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum Mode {
    /// Under [`PREFIX`], the host answering everything else.
    Nested,
    /// As the host's fallback, answering whatever the host did not match.
    Fallback,
}

/// The suite app's drain window, which the hosts that take a stop bound are given by their `run`.
/// Short, since a host declaring `DrainAbandoned::Window` takes all of it, and long enough that
/// half of it covers a `Released` host dropping a response at its next write.
const DRAIN: Duration = Duration::from_secs(4);

/// The prefix a nested host mounts the app under.
pub const PREFIX: &str = "/api";

/// The response header a host's test middleware writes from the `Routing` in the app's response,
/// as [`routing_label`] spells it, since the client sees no response extensions.
pub const ROUTING_HEADER: &str = "x-ulo-routing";

/// One host the suite runs against: the hyper backend, or an embedding adapter's host framework.
///
/// What `start` sets up, beyond serving the app:
///
/// - nested, the app mounted under [`PREFIX`] with `.nested_at(PREFIX)`; as the fallback, mounted
///   as the host's fallback with no `.nested_at`;
/// - a [`HostValue`] from the [`HOST_VALUE_HEADER`] request header, where the request carries it:
///   on a host declaring `host_extensions: true`, written into the host's own request store by a
///   host middleware; on one declaring `false`, copied by `Embedded::forward::<HostValue>(..)`;
/// - on every response carrying a `Routing`, the header [`ROUTING_HEADER`] holding
///   [`routing_label`] of it, written by a host middleware, or on rocket by
///   `rocket_fairing::RoutingFairing`;
/// - the host served through the adapter's `run`, so the app owns the shutdown.
pub trait Host: Sized + Send + Sync + 'static {
    /// Binds the suite's app, connected and not yet bound, into this host in `mode`, runs
    /// `listen()`, and starts serving it.
    fn start(app: App<Connected>, mode: Mode) -> impl Future<Output = Self> + Send;

    /// The URL the host serves on, without a trailing slash: `http://127.0.0.1:41823`.
    fn base_url(&self) -> String;

    /// What the host declares, which decides the scenarios it runs and the refusals it must show.
    fn limits() -> EmbedLimits;

    /// Shuts the app and the host down.
    fn stop(self) -> impl Future<Output = ()> + Send;
}

/// `error` and every source under it, joined by `: `, for a failure message: a scenario that fails
/// while its host starts says what failed, the app's wiring, its `listen`, the host's listener or
/// its launch, and why. A source whose text the message already carries is not repeated.
pub fn report(error: &(dyn Error + 'static)) -> String {
    let mut text = error.to_string();
    let mut source = error.source();
    while let Some(cause) = source {
        let line = cause.to_string();
        if !text.contains(&line) {
            text.push_str(": ");
            text.push_str(&line);
        }
        source = cause.source();
    }
    text
}

/// The application every scenario runs: its controller, error handler, upgrade handler and
/// pre-dispatch entries, wired against the app's `Timer`, as a host that can do everything takes
/// it.
pub async fn app() -> App<Connected> {
    app_for(EmbedLimits::NONE).await
}

/// The suite's application as a host declaring `limits` takes it: without the upgrade handler
/// where `upgrades` is `false`, which `prepare` would otherwise refuse.
pub async fn app_for(limits: EmbedLimits) -> App<Connected> {
    App::builder(app::SuiteModule { limits })
        .timer(ulo_tokio::Timer)
        .drain_timeout(DRAIN)
        .wire()
        .unwrap_or_else(|error| panic!("the suite's app did not wire: {}", report(&error)))
        .connect()
        .await
        .unwrap_or_else(|error| panic!("the suite's app did not connect: {}", report(&error)))
}

/// How [`ROUTING_HEADER`] spells a `Routing`: `matched <route>`, `options <route>`, `not-found`,
/// `method-not-allowed` or `unrouted`.
pub fn routing_label(routing: &Routing) -> String {
    match routing {
        Routing::Matched { route, .. } => format!("matched {route}"),
        Routing::Options { route } => format!("options {route}"),
        Routing::NotFound => "not-found".to_owned(),
        Routing::MethodNotAllowed => "method-not-allowed".to_owned(),
        Routing::Unrouted => "unrouted".to_owned(),
        _ => "other".to_owned(),
    }
}

/// Stamps every scenario in both modes as a `#[tokio::test]` for the host type `$host`. The
/// invoking crate depends on `tokio` with `macros` and `rt-multi-thread`.
///
/// A scenario that cannot apply to a host is declared with its reason, and stamped
/// `#[ignore = "not applicable: <reason>"]`, so the test report counts it as ignored rather than
/// passed:
///
/// ```ignore
/// ulo_http_conformance::http_conformance_suite!(HyperHost; not_applicable {
///     routing_extension: "the reference has no host around the app to read `Routing`",
/// });
/// ```
///
/// A declared name that is no scenario fails to compile. A scenario run on a host it does not
/// apply to fails rather than passing, so a host leaving out a declaration it needs fails too;
/// `cargo test -- --ignored` runs the declared ones, which then fail the same way.
#[macro_export]
macro_rules! http_conformance_suite {
    ($host:ty $(,)?) => {
        $crate::http_conformance_suite!($host; not_applicable {});
    };
    ($host:ty; not_applicable { $($skip:ident : $why:literal),* $(,)? } $(,)?) => {
        $crate::http_conformance_suite!(@stamper [$] $host; $($skip : $why),*);
    };
    (@stamper [$d:tt] $host:ty; $($skip:ident : $why:literal),*) => {
        macro_rules! __ulo_http_conformance_stamp {
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
        $crate::http_conformance_suite!(@cases $host; [$($skip),*];
            routing_hit => routing::hit,
            not_found_with_no_route => routing::not_found,
            method_not_allowed_with_allow => routing::method_not_allowed,
            options_answered_204 => routing::options,
            head_answered_from_get => routing::head,
            extraction_failures => extraction::failures,
            reshaped_error => errors::reshaped,
            recovered_error => errors::recovered,
            unscoped_entry_answers_preflight => pre_dispatch::preflight,
            sse_error_event => sse::error_event,
            disconnect_mid_stream => disconnect::mid_stream,
            upgrade_echoes_a_frame => upgrade::echo,
            host_value_present_and_absent => host_values::present_and_absent,
            routing_extension => routing_ext::routing,
            unavailable_before_listen_and_after_close => lifecycle::unavailable,
            drain_http1 => drain::http1,
            drain_http2 => drain::http2,
            drain_goaway => drain::goaway,
            drain_abandoned => drain::abandoned,
        );
    };
    (@cases $host:ty; [$($skip:ident),*]; $($name:ident => $module:ident :: $case:ident),* $(,)?) => {
        mod nested {
            use super::*;
            // A declared name that is no scenario names no function here.
            $(const _: fn() = $skip;)*
            $(
                __ulo_http_conformance_stamp!($name, async fn $name() {
                    $crate::cases::$module::$case::<$host>($crate::Mode::Nested).await;
                });
            )*
        }
        mod fallback {
            use super::*;
            $(
                __ulo_http_conformance_stamp!($name, async fn $name() {
                    $crate::cases::$module::$case::<$host>($crate::Mode::Fallback).await;
                });
            )*
        }
    };
}
