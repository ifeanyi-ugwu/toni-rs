//! Every public error type (§10.2). Each is `#[non_exhaustive]`, the structs included: code
//! outside the core reads these types and builds none of them. `Redacted` carries no attribute;
//! its private fields close it the same way.

pub(crate) mod wiring;

use std::error::Error;
use std::fmt;
use std::sync::Arc;
use std::time::Duration;

use crate::hooks::HookKind;
use crate::key::{BindingKind, KeyName};
use crate::module::ModuleName;
use crate::redact::Redacted;
use crate::signal::Signal;

pub use wiring::WiringErrors;

#[non_exhaustive]
#[derive(Debug)]
pub enum StartupError {
    /// Everything from `wire()`.
    Wiring(WiringErrors),
    Connect(ConnectError),
    /// A transport's own error, or the core's `NoTimer`.
    Bind { transport: &'static str, source: Redacted },
}

impl fmt::Display for StartupError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        todo!()
    }
}

impl Error for StartupError {
    fn source(&self) -> Option<&(dyn Error + 'static)> {
        todo!()
    }
}

impl From<WiringErrors> for StartupError {
    fn from(e: WiringErrors) -> Self {
        StartupError::Wiring(e)
    }
}

impl From<ConnectError> for StartupError {
    fn from(e: ConnectError) -> Self {
        StartupError::Connect(e)
    }
}

/// `listen()`'s refusal of an app that binds a transport with no `Timer` (§9.5), stored in
/// `StartupError::Bind`'s `source` like a transport's error; `source.downcast_ref::<NoTimer>()`
/// tells it from a port already taken.
#[non_exhaustive]
#[derive(Debug)]
pub struct NoTimer {
    pub transport: &'static str,
}

impl fmt::Display for NoTimer {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "transport `{}` is bound on an app with no `Timer`; set one with `.timer(..)`", self.transport)
    }
}

impl Error for NoTimer {}

/// A failure in the connect phase, on `StartupError::Connect` from `connect` and on
/// `LoadError::Connect` from `load`.
#[non_exhaustive]
#[derive(Debug)]
pub enum ConnectError {
    Construct { key: KeyName, module: ModuleName, reason: FailureReason },
    /// How the last attempt ended, or `TimedOut { limit: Item }` for the whole check (§9.3).
    Readiness { key: KeyName, attempts: u32, reason: FailureReason },
    Hook { hook: HookKind, key: KeyName, reason: FailureReason },
}

impl fmt::Display for ConnectError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        todo!()
    }
}

impl Error for ConnectError {}

/// Why a hook, a construction or a readiness check did not complete. One enum for every place
/// one can fail, so a timeout reads the same on an init hook, a destroy hook, a constructor and
/// a check.
#[non_exhaustive]
#[derive(Debug)]
pub enum FailureReason {
    /// The payload, converted to a message.
    Panicked(Redacted),
    /// `after` is the configured duration of the limit that fired.
    TimedOut { after: Duration, limit: Limit },
    /// Never started: `shutdown_timeout` had expired (§9.5); shutdown only.
    Skipped,
    /// Returned `Err`: a construction, an init or bootstrap hook, or the last attempt of a
    /// readiness check.
    Errored(Redacted),
}

impl fmt::Display for FailureReason {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        todo!()
    }
}

/// Which limit a `TimedOut` hit.
#[non_exhaustive]
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Limit {
    /// An explicit bound on the whole item: a trait const's `After(..)`, or `.timeout(..)` on a
    /// binding handle (§3.9).
    Item,
    /// An explicit `.attempt_timeout(..)` on a readiness check (§9.3).
    Attempt,
    /// The app default for this kind of item: `hook_timeout` for a hook, `construct_timeout`
    /// for a construction and for one attempt of a readiness check.
    Default,
    /// `shutdown_timeout` cutting a hook mid-run (§9.5).
    ShutdownCap,
}

/// The outcome of a shutdown, received by `serve` and by every `close` caller (§9.5).
#[non_exhaustive]
#[derive(Clone, Debug)]
pub struct Shutdown {
    /// The trigger that won.
    pub signal: Signal,
    /// Executions still alive at the drain's end: `drain_timeout`, or the earlier
    /// `shutdown_timeout` expiry.
    pub abandoned: usize,
    /// Terminal executions refused or abandoned (§9.5).
    pub terminal_skipped: usize,
}

/// One outcome, several receivers; the failures sit behind the `Arc`.
#[non_exhaustive]
#[derive(Clone, Debug)]
pub struct ShutdownError {
    pub report: Shutdown,
    pub failures: Arc<[ShutdownFailure]>,
}

impl fmt::Display for ShutdownError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        todo!()
    }
}

impl Error for ShutdownError {}

#[non_exhaustive]
#[derive(Debug)]
pub enum ShutdownFailure {
    /// Never `Errored`: the shutdown hook traits return `()`.
    Hook { hook: HookKind, key: KeyName, reason: FailureReason },
    Close { transport: &'static str, source: Redacted },
}

impl fmt::Display for ShutdownFailure {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        todo!()
    }
}

/// `execute` and `Execution::open` from Draining on; `load` carries it in `LoadError::Closed`
/// from Stopping on (§9.5). Constructed by the core alone.
#[non_exhaustive]
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Closed;

impl Closed {
    pub(crate) const fn new() -> Self {
        Closed
    }
}

impl fmt::Display for Closed {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str("the application is shutting down")
    }
}

impl Error for Closed {}

/// Why `load` did not return a `ModuleRef` (§8.6).
#[non_exhaustive]
#[derive(Debug)]
pub enum LoadError {
    Closed(Closed),
    /// The module wired against the frozen graph, every error collected.
    Wiring(WiringErrors),
    /// A construction, readiness check or init hook of the module failed.
    Connect(ConnectError),
    Refused(LoadRefusal),
}

impl fmt::Display for LoadError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        todo!()
    }
}

impl Error for LoadError {
    fn source(&self) -> Option<&(dyn Error + 'static)> {
        todo!()
    }
}

/// What a lazily loaded module may not bring, because the graph has been handed out (§8.6).
#[non_exhaustive]
#[derive(Debug)]
pub enum LoadRefusal {
    Controllers { module: ModuleName },
    Middleware { module: ModuleName },
    /// To a collection the module does not introduce.
    Contribution { module: ModuleName, key: KeyName },
    GlobalExport { module: ModuleName, key: KeyName },
    /// Inputs belong to transports; the per-handler check ran at wiring time (§6.4).
    Input { module: ModuleName, key: KeyName },
}

impl fmt::Display for LoadRefusal {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        todo!()
    }
}

#[non_exhaustive]
#[derive(Debug)]
pub enum LookupError {
    /// A binding, an extension, an input or a module.
    NotFound { key: KeyName, kind: LookupKind },
    /// A singleton the connect walk has not built yet (§8.5).
    NotReady { key: KeyName },
    /// An erased `Key` asked for a `T` it does not hold.
    WrongType { key: KeyName, requested: &'static str },
    /// Single read as collection, or the reverse.
    WrongKind { key: KeyName, expected: BindingKind, found: BindingKind },
    ExecutionRequired { key: KeyName },
    AmbiguousModule { module: &'static str, candidates: Vec<ModuleName> },
    /// A build inside a call failed, panicked or timed out (§3.4).
    Construct { key: KeyName, reason: FailureReason },
    /// A singleton lookup from Destroying on (§9.5).
    Closed { key: KeyName },
}

impl fmt::Display for LookupError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        todo!()
    }
}

impl Error for LookupError {}

/// What a `LookupError::NotFound` did not find.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum LookupKind {
    Binding,
    Extension,
    Input,
    /// `app.module::<M>()` with no module of type `M` registered.
    Module,
}

/// A guard's `Ok(false)`, as the error handlers see it (§7). Unclaimed, the transport renders
/// its forbidden status.
#[non_exhaustive]
#[derive(Clone, Copy, Debug)]
pub struct GuardRejected {
    pub guard: &'static str,
}

impl GuardRejected {
    pub(crate) fn new(guard: &'static str) -> Self {
        GuardRejected { guard }
    }
}

impl fmt::Display for GuardRejected {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "guard `{}` refused the call", self.guard)
    }
}

impl Error for GuardRejected {}
