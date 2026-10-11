//! The embedding conformance suite (transports DESIGN §3.8): one scenario list run against the
//! hyper backend, the reference [`Host`], and against every embedding adapter in both modes,
//! nested and fallback, stamped per host by [`http_conformance_suite!`].
//!
//! ```ignore
//! // crates/ulo-http-axum/tests/conformance.rs
//! struct AxumHost { /* the host server, its address, the app */ }
//!
//! impl ulo_http_conformance::Host for AxumHost {
//!     type Harness = ulo_http_conformance::OnTokio;
//!     async fn start(app: App<Connected>, mode: Mode) -> Self { /* bind Embedded<Axum>, listen, serve the router */ }
//!     fn base_url(&self) -> String { /* .. */ }
//!     fn limits() -> EmbedLimits { <ulo_http_axum::Axum as Embed>::limits() }
//!     fn connections_read(&self) -> Option<usize> { /* a `ReadCount` over what its listener accepts */ }
//!     async fn stop(self) { /* .. */ }
//! }
//!
//! ulo_http_conformance::http_conformance_suite!(AxumHost);
//! ```
//!
//! The suite names no runtime. A host's [`Harness`] is its runtime half, as the runtime suite's
//! and the WebSocket suite's are: the app's runtime, a `block_on` each scenario runs inside, the
//! listener the reference host accepts on, and the client's connection. Every wait a scenario
//! makes is bounded by the app's `Timer`, every task it starts is spawned on the app's `Runtime`,
//! and its client speaks HTTP/1.1 and HTTP/2 through hyper's and `h2`'s client connections over
//! the stream the harness opens. `OnTokio`, behind the `tokio` feature, is the harness of every
//! host whose framework runs on tokio; `OnSmol`, behind `smol`, runs the hyper backend's scenarios
//! on smol with smol's sockets.
//!
//! The suite asserts byte-identical responses wherever a scenario depends on no declared limit:
//! status, the headers the app writes and the body, compared against the reference host in the
//! same mode, on the same runtime. `Routing` is asserted apart, through the header
//! [`ROUTING_HEADER`] a host's test middleware writes. The limits are checked in both directions:
//! a host that passes a scenario it declares unsupported fails, and one declaring `forward_miss`
//! that answers a `Forwardable` 404 itself fails the same way.
//!
//! No scenario passes on silence. A request the client's own timeout ends fails the scenario,
//! since it is neither an answer nor a refusal, and so does a wait for a close or a frame that
//! does not arrive in time. A scenario that cannot apply to a host is declared not applicable in
//! [`http_conformance_suite!`] and reported as ignored; run on that host, it fails.
//!
//! A host short of memory bounds how many scenarios of one suite run at once by setting
//! [`PARALLEL_VAR`], `ULO_CONFORMANCE_PARALLEL`, to a positive integer, as the RPC suite reads it.
//! Unset or empty, it bounds nothing, and libtest's `--test-threads` still applies on top of it.

use std::error::Error;
use std::future::Future;
use std::io;
use std::net::SocketAddr;
use std::time::Duration;

use futures_io::{AsyncRead, AsyncWrite};
use ulo::app::Connected;
use ulo::{App, Runtime};
use ulo_http::Routing;
use ulo_http::embed::EmbedLimits;

mod app;
pub mod cases;
mod count;
mod failures;
mod reference;
#[cfg(feature = "smol")]
mod smol_harness;
#[cfg(feature = "tokio")]
mod tokio_harness;
mod wire;

#[cfg(feature = "rocket")]
pub mod rocket_fairing;

pub use app::{HOST_VALUE_HEADER, HostValue, ORIGIN};
#[cfg(feature = "tokio")]
pub use count::Counted;
pub use count::ReadCount;
pub use failures::failures_dir;
pub use reference::HyperHost;
#[cfg(feature = "smol")]
pub use smol_harness::OnSmol;
#[cfg(feature = "tokio")]
pub use tokio_harness::OnTokio;

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

/// The runtime a host's scenarios run on, as the suite drives it: the app's runtime, the executor
/// each scenario is run to its end on, the listener the reference host accepts on, and a client
/// connection to a host.
pub trait Harness: Send + Sync + 'static {
    /// The app's runtime, on which the host's connections and the scenario's own waits and tasks
    /// run.
    type Runtime: Runtime;

    /// The listener the reference host, the hyper backend, accepts on: this runtime's sockets,
    /// so a host is compared with the reference on the runtime it runs on.
    type Listener: ulo_hyper_serve::Listener;

    /// The client end of one connection to a host, on `futures-io`'s traits.
    type Stream: AsyncRead + AsyncWrite + Unpin + Send + 'static;

    /// A runtime value; each scenario takes its own, built inside the future [`block_on`] runs, so
    /// a runtime that captures the executor it runs on finds it.
    ///
    /// [`block_on`]: Harness::block_on
    fn runtime() -> Self::Runtime;

    /// Runs `fut` to its end from a plain `#[test]` thread, on an executor that runs the tasks the
    /// runtime spawns while `fut` waits. Each scenario calls it once.
    fn block_on<F: Future>(fut: F) -> F::Output;

    /// Opens a TCP connection to `addr`.
    fn connect(addr: SocketAddr) -> impl Future<Output = io::Result<Self::Stream>> + Send;
}

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
    /// The runtime the host's scenarios run on; the reference host it is compared with runs on the
    /// same one.
    type Harness: Harness;

    /// Binds the suite's app, connected and not yet bound, into this host in `mode`, runs
    /// `listen()`, and starts serving it.
    fn start(app: App<Connected>, mode: Mode) -> impl Future<Output = Self> + Send;

    /// The URL the host serves on, without a trailing slash: `http://127.0.0.1:41823`.
    fn base_url(&self) -> String;

    /// What the host declares, which decides the scenarios it runs and the refusals it must show.
    fn limits() -> EmbedLimits;

    /// How many connections the host's server has accepted and read from so far, each counted at
    /// its first read, or `None` from a server that cannot count them. An embedding host wraps
    /// what its listener accepts with `ReadCount::wrap`, behind the `tokio` feature.
    ///
    /// `drain_http1` reads it to know the host has begun reading a connection carrying half a
    /// request before the drain begins: a host closing its listener at the drain resets a
    /// connection still in the backlog, and hyper's graceful shutdown closes as idle a connection
    /// it has read nothing from. On a host answering `None` the scenario fails; such a host
    /// declares it not applicable, with the reason its server cannot count.
    fn connections_read(&self) -> Option<usize>;

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

/// Fails a scenario whose environment did not start, as `panic!` with the same arguments would,
/// after writing the message to a file of its own under [`failures_dir`]; the panic names the file.
/// Every startup path in the suite fails through it, and a [`Host`] implementation's own startup
/// should too.
#[macro_export]
macro_rules! startup_failed {
    ($($arg:tt)+) => {
        $crate::__private::startup_failed(::std::format!($($arg)+))
    };
}

/// The environment variable through which a host bounds how many scenarios of one suite run at
/// once; the RPC suite reads the same one. A value that is not a positive integer fails every
/// scenario through [`startup_failed!`].
pub const PARALLEL_VAR: &str = "ULO_CONFORMANCE_PARALLEL";

#[doc(hidden)]
pub mod __private {
    use std::num::NonZeroUsize;
    use std::sync::{Condvar, Mutex, OnceLock, PoisonError};

    pub use crate::failures::startup_failed;
    use crate::PARALLEL_VAR;

    /// The slots one stamped suite's scenarios share, both modes counted together. Each scenario
    /// runs on its own test thread, so a slot is waited for by blocking that thread before the
    /// scenario's runtime starts.
    pub struct Slots {
        held: Mutex<usize>,
        freed: Condvar,
    }

    impl Slots {
        pub const fn new() -> Self {
            Slots { held: Mutex::new(0), freed: Condvar::new() }
        }

        /// A slot when [`PARALLEL_VAR`] bounds the suite, held until the guard drops; `None`
        /// otherwise.
        pub fn hold(&'static self) -> Option<Slot> {
            let parallel = host_parallel()?;
            let mut held = self.held.lock().unwrap_or_else(PoisonError::into_inner);
            while *held >= parallel.get() {
                held = self.freed.wait(held).unwrap_or_else(PoisonError::into_inner);
            }
            *held += 1;
            Some(Slot(self))
        }
    }

    impl Default for Slots {
        fn default() -> Self {
            Slots::new()
        }
    }

    /// One held slot, given back on drop, a scenario's panic included.
    pub struct Slot(&'static Slots);

    impl Drop for Slot {
        fn drop(&mut self) {
            *self.0.held.lock().unwrap_or_else(PoisonError::into_inner) -= 1;
            self.0.freed.notify_one();
        }
    }

    /// [`PARALLEL_VAR`] as the host set it, read once per test binary: `None` unset or empty.
    fn host_parallel() -> Option<NonZeroUsize> {
        static HOST: OnceLock<Option<NonZeroUsize>> = OnceLock::new();
        *HOST.get_or_init(|| match std::env::var(PARALLEL_VAR) {
            Err(std::env::VarError::NotPresent) => None,
            Ok(value) if value.trim().is_empty() => None,
            Ok(value) => match value.trim().parse::<NonZeroUsize>() {
                Ok(parallel) => Some(parallel),
                Err(_) => startup_failed(format!("{PARALLEL_VAR} is `{value}`, not a positive integer")),
            },
            Err(std::env::VarError::NotUnicode(value)) => startup_failed(format!("{PARALLEL_VAR} is {value:?}, not a positive integer")),
        })
    }
}

/// The application every scenario runs: its controller, error handler, upgrade handler and
/// pre-dispatch entries, wired against `runtime`, as a host that can do everything takes it.
pub async fn app(runtime: impl Runtime) -> App<Connected> {
    app_for(EmbedLimits::NONE, runtime).await
}

/// The suite's application as a host declaring `limits` takes it, on `runtime`: without the
/// upgrade handler where `upgrades` is `false`, which `prepare` would otherwise refuse.
pub async fn app_for(limits: EmbedLimits, runtime: impl Runtime) -> App<Connected> {
    App::builder(app::SuiteModule { limits })
        .runtime(runtime)
        .drain_timeout(DRAIN)
        .wire()
        .unwrap_or_else(|error| crate::startup_failed!("the suite's app did not wire: {}", report(&error)))
        .connect()
        .await
        .unwrap_or_else(|error| crate::startup_failed!("the suite's app did not connect: {}", report(&error)))
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

/// Stamps every scenario in both modes as a `#[test]` for the host type `$host`, each running its
/// scenario through the host's [`Harness::block_on`].
///
/// A scenario that cannot apply to a host is declared with its reason, and stamped
/// `#[ignore = "not applicable: <reason>"]`, so the test report counts it as ignored rather than
/// passed:
///
/// ```ignore
/// ulo_http_conformance::http_conformance_suite!(HyperHost<OnTokio>; not_applicable {
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
                    #[test]
                    #[ignore = concat!("not applicable: ", $why)]
                    $d($d test)*
                };
            )*
            ($d other:ident, $d($d test:tt)*) => {
                #[test]
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
        static __ULO_HTTP_CONFORMANCE_SLOTS: $crate::__private::Slots = $crate::__private::Slots::new();
        mod nested {
            use super::*;
            // A declared name that is no scenario names no function here.
            $(const _: fn() = $skip;)*
            $(
                __ulo_http_conformance_stamp!($name, fn $name() {
                    let _slot = super::__ULO_HTTP_CONFORMANCE_SLOTS.hold();
                    <<$host as $crate::Host>::Harness as $crate::Harness>::block_on(
                        $crate::cases::$module::$case::<$host>($crate::Mode::Nested),
                    );
                });
            )*
        }
        mod fallback {
            use super::*;
            $(
                __ulo_http_conformance_stamp!($name, fn $name() {
                    let _slot = super::__ULO_HTTP_CONFORMANCE_SLOTS.hold();
                    <<$host as $crate::Host>::Harness as $crate::Harness>::block_on(
                        $crate::cases::$module::$case::<$host>($crate::Mode::Fallback),
                    );
                });
            )*
        }
    };
}
