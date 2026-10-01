//! Every public error type (§10.2). Each is `#[non_exhaustive]`, the structs included: code
//! outside the core reads these types and builds none of them. `Redacted` carries no attribute;
//! its private fields close it the same way.
//!
//! The outcomes a caller propagates out of `main`, `StartupError`, `LoadError`, `ShutdownError`
//! and `WiringErrors`, write their `Display` text under `Debug` too, so `main` returning
//! `Box<dyn Error>` prints the report rather than the structure.

pub(crate) mod wiring;

use std::error::Error;
use std::fmt;
use std::sync::Arc;
use std::time::Duration;

use crate::hooks::HookKind;
use crate::key::{BindingKind, KeyName, short_type_name};
use crate::module::ModuleName;
use crate::redact::Redacted;
use crate::signal::Signal;

pub use wiring::WiringErrors;

#[non_exhaustive]
pub enum StartupError {
    /// Everything from `wire()`.
    Wiring(WiringErrors),
    Connect(ConnectError),
    /// A transport's own error, or the core's `NoTimer`.
    Bind { transport: &'static str, source: Redacted },
}

impl fmt::Display for StartupError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            StartupError::Wiring(e) => fmt::Display::fmt(e, f),
            StartupError::Connect(e) => fmt::Display::fmt(e, f),
            StartupError::Bind { transport, source } => {
                write!(f, "transport `{}` failed to bind: {source}", short_type_name(*transport))
            }
        }
    }
}

impl fmt::Debug for StartupError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        fmt::Display::fmt(self, f)
    }
}

/// Transparent: the wrapped error's text is this error's text, so its source is the wrapped
/// error's source rather than the wrapped error, which a chain reporter would print twice.
impl Error for StartupError {
    fn source(&self) -> Option<&(dyn Error + 'static)> {
        match self {
            StartupError::Wiring(e) => e.source(),
            StartupError::Connect(e) => e.source(),
            StartupError::Bind { .. } => None,
        }
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
        match self {
            ConnectError::Construct { key, module, reason } => {
                write!(f, "failed to construct `{key}` in {module}: {reason}")
            }
            ConnectError::Readiness { key, attempts, reason } => {
                let noun = if *attempts == 1 { "attempt" } else { "attempts" };
                write!(f, "readiness check of `{key}` failed after {attempts} {noun}: {reason}")
            }
            ConnectError::Hook { hook, key, reason } => write!(f, "`{hook}` hook of `{key}` failed: {reason}"),
        }
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
        match self {
            FailureReason::Panicked(message) => write!(f, "panicked: {message}"),
            FailureReason::TimedOut { after, limit } => {
                let which = match limit {
                    Limit::Item => "its own bound",
                    Limit::Attempt => "the attempt bound",
                    Limit::Default => "the app default",
                    Limit::ShutdownCap => "`shutdown_timeout`",
                };
                write!(f, "timed out after {after:?} ({which})")
            }
            FailureReason::Skipped => f.write_str("skipped: `shutdown_timeout` had expired"),
            FailureReason::Errored(error) => fmt::Display::fmt(error, f),
        }
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
#[derive(Clone)]
pub struct ShutdownError {
    pub report: Shutdown,
    pub failures: Arc<[ShutdownFailure]>,
}

impl fmt::Display for ShutdownError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        let count = self.failures.len();
        let noun = if count == 1 { "failure" } else { "failures" };
        write!(f, "shutdown on `{}` finished with {count} {noun}", self.report.signal)?;
        for failure in self.failures.iter() {
            write!(f, "\n  × {failure}")?;
        }
        let abandoned = self.report.abandoned;
        if abandoned > 0 {
            let noun = if abandoned == 1 { "execution" } else { "executions" };
            write!(f, "\n  {abandoned} {noun} abandoned at the drain's end")?;
        }
        let skipped = self.report.terminal_skipped;
        if skipped > 0 {
            let noun = if skipped == 1 { "terminal execution" } else { "terminal executions" };
            write!(f, "\n  {skipped} {noun} refused or abandoned")?;
        }
        Ok(())
    }
}

impl fmt::Debug for ShutdownError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        fmt::Display::fmt(self, f)
    }
}

impl Error for ShutdownError {}

#[non_exhaustive]
#[derive(Debug)]
pub enum ShutdownFailure {
    /// `Errored` only for a closure hook whose dependency read failed: the shutdown hook traits and
    /// closures return `()`.
    Hook { hook: HookKind, key: KeyName, reason: FailureReason },
    Close { transport: &'static str, source: Redacted },
}

impl fmt::Display for ShutdownFailure {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            ShutdownFailure::Hook { hook, key, reason } => write!(f, "`{hook}` hook of `{key}`: {reason}"),
            ShutdownFailure::Close { transport, source } => {
                write!(f, "transport `{}` failed to close: {source}", short_type_name(*transport))
            }
        }
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
        match self {
            LoadError::Closed(closed) => write!(f, "cannot load a module: {closed}"),
            LoadError::Wiring(e) => fmt::Display::fmt(e, f),
            LoadError::Connect(e) => fmt::Display::fmt(e, f),
            LoadError::Refused(refusal) => fmt::Display::fmt(refusal, f),
        }
    }
}

impl fmt::Debug for LoadError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        fmt::Display::fmt(self, f)
    }
}

/// Transparent, like `StartupError`.
impl Error for LoadError {
    fn source(&self) -> Option<&(dyn Error + 'static)> {
        match self {
            LoadError::Closed(_) | LoadError::Refused(_) => None,
            LoadError::Wiring(e) => e.source(),
            LoadError::Connect(e) => e.source(),
        }
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
        match self {
            LoadRefusal::Controllers { module } => {
                write!(f, "cannot load {module}: it declares controllers, and routes are already bound")
            }
            LoadRefusal::Middleware { module } => write!(
                f,
                "cannot load {module}: it writes module metadata such as middleware, which transports read when they bind"
            ),
            LoadRefusal::Contribution { module, key } => write!(
                f,
                "cannot load {module}: it contributes to `{key}`, a collection it does not introduce, which bindings already read"
            ),
            LoadRefusal::GlobalExport { module, key } => {
                write!(f, "cannot load {module}: it exports `{key}` globally, and global exports are fixed at wiring")
            }
            LoadRefusal::Input { module, key } => write!(
                f,
                "cannot load {module}: it declares the execution input `{key}`; inputs belong to transports, and the per-handler input check ran at wiring"
            ),
        }
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
    /// `module` is the module type `app.module::<M>()` named, or the type of a key that several
    /// modules export, met by a lookup outside the root; `candidates` are those modules.
    AmbiguousModule { module: &'static str, candidates: Vec<ModuleName> },
    /// A build inside a call failed, panicked or timed out (§3.4).
    Construct { key: KeyName, reason: FailureReason },
    /// A singleton lookup from Destroying on (§9.5).
    Closed { key: KeyName },
}

impl fmt::Display for LookupError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            LookupError::NotFound { key, kind } => match kind {
                LookupKind::Binding => write!(f, "no binding for `{key}` is visible from this module"),
                LookupKind::Extension => write!(f, "the extension `{key}` was not written in this execution"),
                LookupKind::Input => write!(f, "the execution input `{key}` was not seeded in this execution"),
                LookupKind::Module => write!(f, "no module `{key}` is registered"),
            },
            LookupError::NotReady { key } => write!(f, "`{key}` is a singleton the connect walk has not built yet"),
            LookupError::WrongType { key, requested } => {
                write!(f, "`{key}` does not hold a `{}`", short_type_name(*requested))
            }
            LookupError::WrongKind { key, expected, found } => {
                write!(f, "`{key}` is {}, read as {}", kind_text(*found), kind_text(*expected))
            }
            LookupError::ExecutionRequired { key } => write!(f, "`{key}` needs an execution, and none is open here"),
            LookupError::AmbiguousModule { module, candidates } => {
                write!(f, "`{}` is ambiguous between ", short_type_name(*module))?;
                for (i, candidate) in candidates.iter().enumerate() {
                    if i > 0 {
                        f.write_str(", ")?;
                    }
                    write!(f, "{candidate}")?;
                }
                Ok(())
            }
            LookupError::Construct { key, reason } => write!(f, "building `{key}` inside the call failed: {reason}"),
            LookupError::Closed { key } => {
                write!(f, "`{key}` is a singleton, and the application has begun destroying its singletons")
            }
        }
    }
}

impl Error for LookupError {}

fn kind_text(kind: BindingKind) -> &'static str {
    match kind {
        BindingKind::Single => "a single binding",
        BindingKind::Collection => "a collection",
    }
}

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
