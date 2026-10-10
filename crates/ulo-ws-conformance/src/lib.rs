//! The conformance suite every `ulo-ws` server runs (transports DESIGN §3.5, §4): one scenario
//! list, stamped per server by [`ws_conformance_suite!`] from a [`Host`] the server's crate
//! implements in its `tests/`. `ulo-ws-hyper`'s standalone server is the reference; the upgrade
//! hand-off on the HTTP server's port runs the same list.
//!
//! ```ignore
//! // crates/ulo-ws-hyper/tests/conformance.rs
//! struct Standalone;
//!
//! impl ulo_ws_conformance::Host for Standalone {
//!     type Runtime = ulo_tokio::Tokio;
//!     type Stream = ulo_http::Upgraded;
//!     const PORT: ulo_ws::Port = ulo_ws::Port::Own;
//!     const CLOSES_IDLE_AT_DRAIN: bool = true; // hyper's graceful shutdown
//!     fn runtime() -> ulo_tokio::Tokio { ulo_tokio::Tokio::current() }
//!     fn block_on<F: Future>(fut: F) -> F::Output { /* a runtime of its own per scenario */ }
//!     fn bind(app: App<Connected>) -> App<Connected> { app.bind(ulo_ws_hyper::Server::new("127.0.0.1:0")) }
//!     async fn connect(addresses: &[BoundAddr]) -> io::Result<ulo_http::Upgraded> { /* a TCP connection */ }
//! }
//!
//! ulo_ws_conformance::ws_conformance_suite!(Standalone);
//! ```
//!
//! The suite names no runtime and depends on none. A scenario runs inside [`Host::block_on`] on
//! the app's runtime, [`Host::runtime`], whose `Timer` bounds every wait, and speaks to the server
//! as a client over the byte stream [`Host::connect`] opens: it writes the upgrade request itself
//! and reads the answer as the server wrote it, then runs `async-tungstenite`'s client on the same
//! stream. A server on smol answers an `async-net` stream; one on tokio wraps its `TcpStream` in a
//! `futures-io` adapter.
//!
//! The suite's application declares every gateway twice, once per [`Port`], and serves the set
//! [`Host::PORT`] names, so a standalone server and a host serving the HTTP server's port run the
//! same scenarios against the same gateways.
//!
//! A scenario asserts an answer it can observe: a status and its headers, an envelope, a Close
//! frame's code and reason, a Ping, or what the application's hooks recorded. None passes on
//! silence. A wait that runs out fails the scenario after [`PATIENCE`], and a scenario about an
//! absence first waits for a positive signal that the moment has passed: a stream that reports
//! nothing is read once its execution has ended, and a message the server must not read yet is
//! checked after a round trip on another connection. A scenario that cannot apply to a host is
//! declared not applicable in [`ws_conformance_suite!`] and reported as ignored; run on that host,
//! it fails.
//!
//! A host short of memory bounds how many scenarios of one suite run at once by setting
//! [`PARALLEL_VAR`], `ULO_CONFORMANCE_PARALLEL`, to a positive integer, as the HTTP and RPC suites
//! read it. Unset or empty, it bounds nothing, and libtest's `--test-threads` still applies on top
//! of it.

use std::error::Error;
use std::future::Future;
use std::io;
use std::time::Duration;

use futures_io::{AsyncRead, AsyncWrite};
use ulo::app::Connected;
use ulo::{App, BoundAddr, Runtime};
use ulo_ws::Port;

mod app;
pub mod cases;
mod client;
mod failures;

pub use failures::failures_dir;

/// How long a scenario waits for anything it expects before failing.
pub const PATIENCE: Duration = Duration::from_secs(5);

/// The suite app's drain window: long enough for a scenario to act while the drain runs.
pub const DRAIN: Duration = Duration::from_secs(4);

/// The environment variable through which a host bounds how many scenarios of one suite run at
/// once; the HTTP and RPC suites read the same one. A value that is not a positive integer fails
/// every scenario through [`startup_failed!`].
pub const PARALLEL_VAR: &str = "ULO_CONFORMANCE_PARALLEL";

/// One WebSocket server as the suite drives it.
///
/// The type carries no state: each scenario builds its own app, binds it through [`bind`], and
/// connects through [`connect`] to the addresses the app's `listen()` reported.
///
/// [`bind`]: Host::bind
/// [`connect`]: Host::connect
pub trait Host: Sized + 'static {
    /// The app's runtime, on which the server's connections and the scenario's own waits run.
    type Runtime: Runtime;

    /// The client end of one connection to the server, on `futures-io`'s traits.
    type Stream: AsyncRead + AsyncWrite + Unpin + 'static;

    /// The gateways this server serves: `Port::Own` for a standalone server, `Port::Http` for one
    /// serving the HTTP server's port through the hand-off `WsModule` registers.
    const PORT: Port;

    /// Whether the server closes a connection that is idle between requests when its drain
    /// begins, rather than keeping it open and answering a request that arrives on it after.
    /// hyper's graceful shutdown closes it. `false` unless a host declares it.
    ///
    /// `handshake_refuses_during_the_drain` reads it: on a host keeping the connection, an
    /// upgrade request sent on it during the drain must be answered 503; on one closing it, the
    /// connection must end with nothing written.
    const CLOSES_IDLE_AT_DRAIN: bool = false;

    /// A runtime value; each scenario takes its own, built inside the future [`block_on`] runs, so
    /// a runtime that captures the executor it runs on finds it.
    ///
    /// [`block_on`]: Host::block_on
    fn runtime() -> Self::Runtime;

    /// Runs `fut` to its end from a plain `#[test]` thread, on an executor that runs the tasks the
    /// runtime spawns while `fut` waits. Each scenario calls it once.
    fn block_on<F: Future>(fut: F) -> F::Output;

    /// Binds the server under test to the suite's app, connected and not yet listening, on a port
    /// the OS chooses, so no two scenarios contend for one. The suite runs `listen()`, serves, and
    /// closes the app.
    fn bind(app: App<Connected>) -> App<Connected>;

    /// Opens a connection to the server: `addresses` holds every address the app bound,
    /// `App<Bound>::addresses()` in the order the servers started.
    fn connect(addresses: &[BoundAddr]) -> impl Future<Output = io::Result<Self::Stream>>;
}

/// `error` and every source under it, joined by `: `, for a failure message: a scenario that fails
/// while its server starts says what failed, the app's wiring, its `listen` or the connection, and
/// why. A source whose text the message already carries is not repeated.
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

/// Stamps one `#[test]` per scenario in [`cases`] for the host type `$host`, each running its
/// scenario through [`Host::block_on`].
///
/// A scenario that cannot apply to a host is declared with its reason, and stamped
/// `#[ignore = "not applicable: <reason>"]`, so the test report counts it as ignored rather than
/// passed:
///
/// ```ignore
/// ulo_ws_conformance::ws_conformance_suite!(SmolServer; not_applicable {
///     subprotocol_choice: "F999: the server drops `Sec-WebSocket-Protocol` from the 101",
/// });
/// ```
///
/// A declared name that is no scenario fails to compile. A scenario run on a host it does not apply
/// to fails rather than passing, so a host leaving out a declaration it needs fails too;
/// `cargo test -- --ignored` runs the declared ones, which then fail the same way.
#[macro_export]
macro_rules! ws_conformance_suite {
    ($host:ty $(,)?) => {
        $crate::ws_conformance_suite!($host; not_applicable {});
    };
    ($host:ty; not_applicable { $($skip:ident : $why:literal),* $(,)? } $(,)?) => {
        $crate::ws_conformance_suite!(@stamper [$] $host; $($skip : $why),*);
    };
    (@stamper [$d:tt] $host:ty; $($skip:ident : $why:literal),*) => {
        macro_rules! __ulo_ws_conformance_stamp {
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
        // A declared name that is no scenario names no function here.
        $(const _: fn() = $skip;)*
        $crate::ws_conformance_suite!(@cases $host;
            handshake_switches_with_the_accept_key => handshake::switches,
            handshake_refuses_a_path_without_a_gateway => handshake::no_gateway,
            handshake_refuses_a_method_other_than_get => handshake::method,
            handshake_refuses_a_malformed_upgrade => handshake::malformed,
            handshake_refuses_another_version => handshake::version,
            handshake_refuses_during_the_drain => handshake::draining,
            subprotocol_choice => handshake::subprotocol,
            refuse_handshake_answers_401_or_403 => handshake::refused_before_upgrade,
            event_round_trip => messages::round_trip,
            unknown_event_is_unimplemented => messages::unknown_event,
            frame_naming_no_event_is_bad_request => messages::no_event,
            control_frame_is_answered_with_nothing => messages::control_frame,
            message_limit_closes_with_1009 => messages::message_limit,
            reply_stream_is_written_whole => messages::reply_stream,
            connect_guard_refusal_closes_with_1008 => close_codes::guard_refusal,
            rate_limited_connect_closes_with_1013 => close_codes::rate_limited,
            faulted_connect_closes_with_1011 => close_codes::faulted,
            connect_refusal_closes_with_its_own_code => close_codes::own_code,
            drain_closes_with_1001 => close_codes::drain,
            ping_interval_sends_pings => keep_alive::pings,
            pong_timeout_ends_the_connection_as_lost => keep_alive::pong_timeout,
            on_connect_refusal_never_reaches_on_disconnect => hooks::on_connect_refusal,
            on_disconnect_fires_once => hooks::on_disconnect_once,
            max_connections_closes_the_next_with_1013 => limits::max_connections,
            max_inflight_stops_reading => limits::max_inflight,
            max_outbound_closes_a_slow_consumer => limits::max_outbound,
            max_outbound_holds_a_reply_stream => limits::max_outbound_stream,
            written_stream_reports_completed => stream_end::written,
            discarded_stream_reports_nothing => stream_end::discarded,
        );
    };
    (@cases $host:ty; $($name:ident => $module:ident :: $case:ident),* $(,)?) => {
        static __ULO_WS_CONFORMANCE_SLOTS: $crate::__private::Slots = $crate::__private::Slots::new();
        $(
            __ulo_ws_conformance_stamp!($name, fn $name() {
                let _slot = __ULO_WS_CONFORMANCE_SLOTS.hold();
                <$host as $crate::Host>::block_on($crate::cases::$module::$case::<$host>());
            });
        )*
    };
}

#[doc(hidden)]
pub mod __private {
    use std::num::NonZeroUsize;
    use std::sync::{Condvar, Mutex, OnceLock, PoisonError};

    pub use crate::failures::startup_failed;
    use crate::PARALLEL_VAR;

    /// The slots one stamped suite's scenarios share. Each scenario runs on its own test thread,
    /// so a slot is waited for by blocking that thread before the scenario's runtime starts.
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
            host_parallel().map(|parallel| self.hold_under(parallel))
        }

        /// A slot under the bound `parallel`, waited for while that many are held.
        fn hold_under(&'static self, parallel: NonZeroUsize) -> Slot {
            let mut held = self.held.lock().unwrap_or_else(PoisonError::into_inner);
            while *held >= parallel.get() {
                held = self.freed.wait(held).unwrap_or_else(PoisonError::into_inner);
            }
            *held += 1;
            Slot(self)
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

    #[cfg(test)]
    mod tests {
        use std::num::NonZeroUsize;
        use std::sync::Arc;
        use std::sync::atomic::{AtomicUsize, Ordering};
        use std::time::Duration;

        use super::Slots;

        #[test]
        fn a_bound_of_one_admits_one_holder_at_a_time() {
            static SLOTS: Slots = Slots::new();
            let one = NonZeroUsize::MIN;
            let inside = Arc::new(AtomicUsize::new(0));
            let most = Arc::new(AtomicUsize::new(0));
            let threads: Vec<_> = (0..3)
                .map(|_| {
                    let (inside, most) = (Arc::clone(&inside), Arc::clone(&most));
                    std::thread::spawn(move || {
                        let slot = SLOTS.hold_under(one);
                        let now = inside.fetch_add(1, Ordering::SeqCst) + 1;
                        most.fetch_max(now, Ordering::SeqCst);
                        std::thread::sleep(Duration::from_millis(20));
                        inside.fetch_sub(1, Ordering::SeqCst);
                        drop(slot);
                    })
                })
                .collect();
            for thread in threads {
                thread.join().expect("a holder panicked");
            }
            assert_eq!(most.load(Ordering::SeqCst), 1, "two holders were inside a bound of one at once");
            assert_eq!(*SLOTS.held.lock().expect("the slots' lock"), 0, "a slot was not given back");
        }
    }
}
