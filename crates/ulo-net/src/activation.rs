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
use std::sync::{Arc, Mutex, MutexGuard, OnceLock, PoisonError};

use crate::endpoint::ListenerName;

/// The sockets this process inherited, read from the environment once per process. Each is taken
/// at most once: a server takes its listener in `bind`, after checking in `prepare` that it is
/// there.
///
/// Every inherited descriptor must be a listening TCP socket; one that is not fails the whole
/// activation with [`ActivationError::NotListening`], and none is adopted. Nothing else in the
/// process may adopt the descriptors as well, since each is closed when its listener drops.
pub struct Activation {
    sockets: Mutex<Vec<Inherited>>,
}

/// One inherited socket, until a server takes it.
#[cfg_attr(not(unix), allow(dead_code))]
struct Inherited {
    index: usize,
    name: Option<String>,
    listener: Option<TcpListener>,
}

impl Inherited {
    fn answers_to(&self, name: &ListenerName) -> bool {
        match name {
            ListenerName::Named(wanted) => self.name.as_deref() == Some(&**wanted),
            ListenerName::Index(index) => self.index == *index,
        }
    }
}

static ACTIVATION: OnceLock<Result<Activation, ActivationError>> = OnceLock::new();

impl Activation {
    /// The process's inherited sockets, `FD_CLOEXEC` set on each. Errs when `LISTEN_PID` names
    /// another process, when the variables are malformed, or on a platform without socket
    /// activation; a process started without them has none, which is not an error.
    ///
    /// Call it only for an [`Endpoint::Inherited`](crate::Endpoint::Inherited): a process spawned
    /// by a socket-activated one inherits the variables with a `LISTEN_PID` that is not its own,
    /// and answers `PidMismatch` here whether or not it wants a socket.
    pub fn get() -> Result<&'static Activation, ActivationError> {
        ACTIVATION.get_or_init(Activation::from_env).as_ref().map_err(Clone::clone)
    }

    #[cfg(unix)]
    fn from_env() -> Result<Activation, ActivationError> {
        unix::inherited().map(|sockets| Activation { sockets: Mutex::new(sockets) })
    }

    #[cfg(not(unix))]
    fn from_env() -> Result<Activation, ActivationError> {
        Err(ActivationError::Unsupported)
    }

    /// Whether `name` is an inherited socket not yet taken, for a server's `prepare`.
    ///
    /// Several descriptors may carry one `LISTEN_FDNAMES` name (systemd names every socket of a
    /// unit after it by default); a name answers for each of them in order.
    pub fn contains(&self, name: &ListenerName) -> bool {
        self.sockets().iter().any(|socket| socket.listener.is_some() && socket.answers_to(name))
    }

    /// How many inherited sockets not yet taken `name` answers for, so a server's `prepare` can
    /// refuse a name its endpoints list more often than it was inherited.
    pub fn count(&self, name: &ListenerName) -> usize {
        self.sockets().iter().filter(|socket| socket.listener.is_some() && socket.answers_to(name)).count()
    }

    /// Takes the inherited socket `name`, non-blocking: the first not yet taken among those the
    /// name answers for.
    pub fn take(&self, name: &ListenerName) -> Result<TcpListener, ActivationError> {
        let listener = self
            .sockets()
            .iter_mut()
            .filter(|socket| socket.answers_to(name))
            .find_map(|socket| socket.listener.take())
            .ok_or_else(|| ActivationError::Missing(name.clone()))?;
        listener.set_nonblocking(true).map_err(|error| ActivationError::Io(Arc::new(error)))?;
        Ok(listener)
    }

    fn sockets(&self) -> MutexGuard<'_, Vec<Inherited>> {
        self.sockets.lock().unwrap_or_else(PoisonError::into_inner)
    }
}

#[cfg(unix)]
mod unix {
    use std::env::{self, VarError};
    use std::net::{SocketAddr, TcpListener};
    use std::os::fd::{BorrowedFd, FromRawFd, OwnedFd, RawFd};

    use socket2::{SockRef, Socket, Type};

    use super::{ActivationError, Inherited};

    /// `SD_LISTEN_FDS_START`: the protocol passes its sockets from descriptor 3 on.
    const FIRST_FD: RawFd = 3;

    /// Reads the variables as `sd_listen_fds_with_names` does: no `LISTEN_PID` or no `LISTEN_FDS`
    /// means no sockets, and a `LISTEN_FDNAMES` must name every descriptor when present. A
    /// `LISTEN_PID` naming another process is an error rather than no sockets, so a server asking
    /// for one learns why it has none (transports DESIGN §12).
    pub(super) fn inherited() -> Result<Vec<Inherited>, ActivationError> {
        let Some(pid) = var("LISTEN_PID")? else { return Ok(Vec::new()) };
        let found = pid.parse::<u32>().map_err(|_| malformed("LISTEN_PID"))?;
        let expected = std::process::id();
        if found != expected {
            return Err(ActivationError::PidMismatch { expected, found });
        }

        let Some(count) = var("LISTEN_FDS")? else { return Ok(Vec::new()) };
        let count = count
            .parse::<usize>()
            .ok()
            .filter(|&count| count <= (RawFd::MAX - FIRST_FD) as usize)
            .ok_or_else(|| malformed("LISTEN_FDS"))?;
        if count == 0 {
            return Ok(Vec::new());
        }

        let names = match var("LISTEN_FDNAMES")? {
            Some(names) => {
                let names: Vec<Option<String>> = names.split(':').map(|name| Some(name.to_owned())).collect();
                if names.len() != count {
                    return Err(malformed("LISTEN_FDNAMES"));
                }
                names
            }
            None => vec![None; count],
        };

        // Every descriptor is vetted before any is adopted, so a set the variables misdescribe
        // leaves each descriptor open and unowned rather than closing one that belongs to
        // something else in the process.
        let mut refused = None;
        for index in 0..count {
            // SAFETY: the descriptor is at least 3, so never -1, and the borrow lasts for the
            // check alone. With `LISTEN_PID` naming this process the protocol's descriptors were
            // open at exec and nothing else in the process owns them, since this runs once per
            // process under `Activation::get`'s `OnceLock`. A descriptor the variables misdescribe
            // fails the check and is neither adopted nor closed.
            let borrowed = unsafe { BorrowedFd::borrow_raw(fd(index)) };
            if !vet(borrowed) && refused.is_none() {
                refused = Some(index);
            }
        }
        if let Some(index) = refused {
            return Err(ActivationError::NotListening { index });
        }

        Ok(names
            .into_iter()
            .enumerate()
            .map(|(index, name)| {
                // SAFETY: the descriptor passed `vet`, so it is open and a listening socket,
                // which needs no cleanup but `close`; it is adopted once, here, and by nothing
                // else in the process (see `Activation`).
                let owned = unsafe { OwnedFd::from_raw_fd(fd(index)) };
                Inherited { index, name, listener: Some(TcpListener::from(owned)) }
            })
            .collect())
    }

    fn fd(index: usize) -> RawFd {
        FIRST_FD + index as RawFd
    }

    /// Sets `FD_CLOEXEC` on `fd`, whatever it turns out to be, and answers whether it is a
    /// listening TCP socket.
    fn vet(fd: BorrowedFd<'_>) -> bool {
        let socket = SockRef::from(&fd);
        let cloexec = socket.set_cloexec(true).is_ok();
        cloexec
            && socket.r#type().is_ok_and(|kind| kind == Type::STREAM)
            && socket
                .local_addr()
                .ok()
                .and_then(|addr| addr.as_socket())
                .is_some_and(|addr| listening(&socket, addr))
    }

    #[cfg(any(
        target_os = "aix",
        target_os = "android",
        target_os = "freebsd",
        target_os = "fuchsia",
        target_os = "linux",
    ))]
    fn listening(socket: &Socket, _: SocketAddr) -> bool {
        socket.is_listener().unwrap_or(false)
    }

    // socket2 reads `SO_ACCEPTCONN` on the targets above only. Elsewhere, macOS among them, a
    // bound port is the closest check: a bound socket that never called `listen` passes, and
    // fails at its first accept instead.
    #[cfg(not(any(
        target_os = "aix",
        target_os = "android",
        target_os = "freebsd",
        target_os = "fuchsia",
        target_os = "linux",
    )))]
    fn listening(_: &Socket, addr: SocketAddr) -> bool {
        addr.port() != 0
    }

    fn var(name: &'static str) -> Result<Option<String>, ActivationError> {
        match env::var(name) {
            Ok(value) => Ok(Some(value)),
            Err(VarError::NotPresent) => Ok(None),
            Err(VarError::NotUnicode(_)) => Err(malformed(name)),
        }
    }

    fn malformed(variable: &'static str) -> ActivationError {
        ActivationError::Malformed { variable }
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
