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
//! The suite asserts byte-identical responses wherever a scenario depends on no declared limit,
//! `Routing` apart. The limits are checked in both directions: a host that passes a scenario it
//! declares unsupported fails, and one declaring `forward_miss` that answers a `Forwardable` 404
//! itself fails the same way.

use std::future::Future;

use ulo::App;
use ulo::app::Connected;
use ulo_http::embed::EmbedLimits;

pub mod cases;
mod reference;

#[cfg(feature = "rocket")]
pub mod rocket_fairing;

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

/// One host the suite runs against: the hyper backend, or an embedding adapter's host framework.
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

/// The application every scenario runs: its controllers, error handlers, gateways and
/// pre-dispatch entries, wired against the app's `Timer`.
pub async fn app() -> App<Connected> {
    todo!()
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
