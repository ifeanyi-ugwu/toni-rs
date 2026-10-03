//! Socket activation: listening sockets a supervisor binds and hands to the process through
//! `LISTEN_FDS`, `LISTEN_FDNAMES` and `LISTEN_PID`, starting at file descriptor 3 (transports
//! DESIGN §2.7, §9). Unix only.
//!
//! An inherited socket is accepted only when `LISTEN_PID` equals the process's ID, as the systemd
//! protocol requires. `FD_CLOEXEC` is set on every inherited socket, so a subprocess the app
//! spawns does not inherit it. The three variables are left in the environment: in edition 2024
//! `std::env::remove_var` is `unsafe` because it races with other threads, and under
//! `#[tokio::main]` the runtime's threads exist before user code runs. A child spawned later reads
//! a `LISTEN_PID` that is not its own and ignores them.

use std::error::Error;
use std::fmt;
use std::io;
use std::net::TcpListener;
use std::sync::{Arc, Mutex, OnceLock};

use crate::endpoint::ListenerName;

/// The sockets this process inherited, read from the environment once per process. Each is taken
/// at most once: a server takes its listener in `bind`, after checking in `prepare` that it is
/// there.
pub struct Activation {
    sockets: Mutex<Vec<Inherited>>,
}

/// One inherited socket, until a server takes it.
struct Inherited {
    index: usize,
    name: Option<String>,
    listener: Option<TcpListener>,
}

static ACTIVATION: OnceLock<Result<Activation, ActivationError>> = OnceLock::new();

impl Activation {
    /// The process's inherited sockets, `FD_CLOEXEC` set on each. Errs when `LISTEN_PID` names
    /// another process, when the variables are malformed, or on a platform without socket
    /// activation; a process started without them has none, which is not an error.
    pub fn get() -> Result<&'static Activation, ActivationError> {
        ACTIVATION.get_or_init(Activation::from_env).as_ref().map_err(Clone::clone)
    }

    fn from_env() -> Result<Activation, ActivationError> {
        todo!("read LISTEN_PID, LISTEN_FDS, LISTEN_FDNAMES; adopt fds 3.. with `FD_CLOEXEC`, checking each is a listening TCP socket")
    }

    /// Whether `name` is an inherited socket not yet taken, for a server's `prepare`.
    pub fn contains(&self, name: &ListenerName) -> bool {
        let _ = name;
        todo!("match by `LISTEN_FDNAMES` name or by index")
    }

    /// Takes the inherited socket `name`, non-blocking.
    pub fn take(&self, name: &ListenerName) -> Result<TcpListener, ActivationError> {
        let _ = name;
        todo!("remove the socket from its slot; `Missing` when absent or taken")
    }
}

/// Why an inherited socket is unavailable. `Clone`, since the environment is read once per process
/// and every server asking afterwards receives the same answer.
#[non_exhaustive]
#[derive(Clone, Debug)]
pub enum ActivationError {
    /// `LISTEN_PID` names another process: the variables were meant for a parent or a sibling.
    PidMismatch { expected: u32, found: u32 },
    /// `LISTEN_PID`, `LISTEN_FDS` or `LISTEN_FDNAMES` is not what the protocol writes.
    Malformed { variable: &'static str },
    /// No inherited socket answers to this name or index.
    Missing(ListenerName),
    /// An inherited descriptor that is not a listening TCP socket.
    NotListening { index: usize },
    /// Socket activation is Unix-only.
    Unsupported,
    Io(Arc<io::Error>),
}

impl fmt::Display for ActivationError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            ActivationError::PidMismatch { expected, found } => {
                write!(f, "LISTEN_PID is {found}, and this process is {expected}; the inherited sockets are not this process's")
            }
            ActivationError::Malformed { variable } => write!(f, "{variable} is malformed"),
            ActivationError::Missing(ListenerName::Named(name)) => write!(f, "no inherited socket is named `{name}`"),
            ActivationError::Missing(ListenerName::Index(index)) => write!(f, "no inherited socket #{index}"),
            ActivationError::NotListening { index } => write!(f, "inherited descriptor #{index} is not a listening TCP socket"),
            ActivationError::Unsupported => f.write_str("socket activation is supported on Unix only"),
            ActivationError::Io(error) => fmt::Display::fmt(error, f),
        }
    }
}

impl Error for ActivationError {
    fn source(&self) -> Option<&(dyn Error + 'static)> {
        match self {
            ActivationError::Io(error) => Some(&**error),
            _ => None,
        }
    }
}
