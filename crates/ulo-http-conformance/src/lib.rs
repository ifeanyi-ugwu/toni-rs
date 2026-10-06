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

use std::future::Future;

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
        .wire()
        .expect("the suite's app wires")
        .connect()
        .await
        .expect("the suite's app connects")
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
#[macro_export]
macro_rules! http_conformance_suite {
    ($host:ty) => {
        $crate::http_conformance_suite!(@cases $host;
            routing_hit => routing::hit,
            not_found_with_no_route => routing::not_found,
            method_not_allowed_with_allow => routing::method_not_allowed,
            options_answered_204 => routing::options,
            head_answered_from_get => routing::head,
            extraction_failures => extraction::failures,
            reshaped_error => errors::reshaped,
            unscoped_entry_answers_preflight => pre_dispatch::preflight,
            sse_error_event => sse::error_event,
            disconnect_mid_stream => disconnect::mid_stream,
            upgrade_echoes_a_frame => upgrade::echo,
            host_value_present_and_absent => host_values::present_and_absent,
            forward_copy => host_values::forward_copy,
            routing_extension => routing_ext::routing,
            unavailable_before_listen_and_after_close => lifecycle::unavailable,
            drain => drain::drain,
        );
    };
    (@cases $host:ty; $($name:ident => $module:ident :: $case:ident),* $(,)?) => {
        mod nested {
            use super::*;
            $(
                #[::tokio::test(flavor = "multi_thread")]
                async fn $name() {
                    $crate::cases::$module::$case::<$host>($crate::Mode::Nested).await;
                }
            )*
        }
        mod fallback {
            use super::*;
            $(
                #[::tokio::test(flavor = "multi_thread")]
                async fn $name() {
                    $crate::cases::$module::$case::<$host>($crate::Mode::Fallback).await;
                }
            )*
        }
    };
}
