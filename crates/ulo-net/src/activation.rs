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
    /// A process spawned by a socket-activated one inherits the variables with a `LISTEN_PID` that
    /// is not its own, and answers `PidMismatch` here whether or not it wants a socket, so a
    /// caller reads it only where a socket is wanted: a server's `prepare` for an
    /// [`Endpoint::Inherited`](crate::Endpoint::Inherited), and
    /// [`EndpointSpec::resolve`](crate::EndpointSpec::resolve) for an
    /// [`Endpoint::Addr`](crate::Endpoint::Addr) under `ULO_DEV=1`, which reads `PidMismatch` as
    /// no socket held and binds the address as written.
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
            if !vet(index, borrowed) && refused.is_none() {
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
    /// listening TCP socket. `index` is the descriptor's position, for the log.
    fn vet(index: usize, fd: BorrowedFd<'_>) -> bool {
        let socket = SockRef::from(&fd);
        let cloexec = socket.set_cloexec(true).is_ok();
        cloexec
            && socket.r#type().is_ok_and(|kind| kind == Type::STREAM)
            && socket
                .local_addr()
                .ok()
                .and_then(|addr| addr.as_socket())
                .is_some_and(|addr| listening(index, &socket, addr))
    }

    // socket2 reads `SO_ACCEPTCONN` on these targets.
    #[cfg(any(
        target_os = "aix",
        target_os = "android",
        target_os = "freebsd",
        target_os = "fuchsia",
        target_os = "linux",
    ))]
    fn listening(_: usize, socket: &Socket, _: SocketAddr) -> bool {
        socket.is_listener().unwrap_or(false)
    }

    // socket2 does not read `SO_ACCEPTCONN` on these targets.
    #[cfg(any(target_os = "dragonfly", target_os = "netbsd", target_os = "openbsd"))]
    fn listening(index: usize, socket: &Socket, addr: SocketAddr) -> bool {
        use std::os::fd::AsRawFd;

        let mut accepting: libc::c_int = 0;
        let mut len = std::mem::size_of::<libc::c_int>() as libc::socklen_t;
        // SAFETY: `getsockopt` writes at most `len` bytes through the value pointer and one
        // `socklen_t` through the length pointer; both point at locals of those sizes that outlive
        // the call. The descriptor is the one `vet` borrows, open for the borrow's lifetime.
        let result = unsafe {
            libc::getsockopt(
                socket.as_raw_fd(),
                libc::SOL_SOCKET,
                libc::SO_ACCEPTCONN,
                (&raw mut accepting).cast::<libc::c_void>(),
                &raw mut len,
            )
        };
        if result == -1 {
            return unanswered(index, addr, std::io::Error::last_os_error());
        }
        accepting != 0
    }

    // XNU defines `SO_ACCEPTCONN` and refuses it in `getsockopt` with `ENOPROTOOPT`, listening or
    // not. `TCP_CONNECTION_INFO` reads the TCP state instead: `TCPS_LISTEN` once `listen` ran,
    // `TCPS_CLOSED` for a socket that is only bound.
    #[cfg(target_vendor = "apple")]
    fn listening(index: usize, socket: &Socket, addr: SocketAddr) -> bool {
        use std::os::fd::AsRawFd;

        // `TCPS_LISTEN` in `<netinet/tcp_fsm.h>`, which libc does not export.
        const TCPS_LISTEN: u8 = 1;

        // SAFETY: `tcp_connection_info` holds integers and `MaybeUninit` padding only, for which
        // all-zero bytes are a valid value.
        let mut info: libc::tcp_connection_info = unsafe { std::mem::zeroed() };
        // libc's layout is longer than the kernel's, spelling the C bitfields after `tcpi_rttvar`
        // as whole `u32`s, so the kernel fills a prefix of it; `tcpi_state` is its first byte.
        let mut len = std::mem::size_of::<libc::tcp_connection_info>() as libc::socklen_t;
        // SAFETY: `getsockopt` writes at most `len` bytes through the value pointer and one
        // `socklen_t` through the length pointer; both point at locals of those sizes that outlive
        // the call. The descriptor is the one `vet` borrows, open for the borrow's lifetime.
        let result = unsafe {
            libc::getsockopt(
                socket.as_raw_fd(),
                libc::IPPROTO_TCP,
                libc::TCP_CONNECTION_INFO,
                (&raw mut info).cast::<libc::c_void>(),
                &raw mut len,
            )
        };
        if result == -1 {
            return unanswered(index, addr, std::io::Error::last_os_error());
        }
        info.tcpi_state == TCPS_LISTEN
    }

    #[cfg(not(any(
        target_os = "aix",
        target_os = "android",
        target_os = "freebsd",
        target_os = "fuchsia",
        target_os = "linux",
        target_os = "dragonfly",
        target_os = "netbsd",
        target_os = "openbsd",
        target_vendor = "apple",
    )))]
    fn listening(index: usize, _: &Socket, addr: SocketAddr) -> bool {
        bound_port(index, addr)
    }

    /// A failed listen-state read: a kernel that does not serve the option falls back to the
    /// bound port, and any other failure refuses the descriptor, as the socket2 read does.
    #[cfg(any(target_os = "dragonfly", target_os = "netbsd", target_os = "openbsd", target_vendor = "apple"))]
    fn unanswered(index: usize, addr: SocketAddr, error: std::io::Error) -> bool {
        error.raw_os_error() == Some(libc::ENOPROTOOPT) && bound_port(index, addr)
    }

    /// The check where nothing reports the listen state: a nonzero bound port. A bound socket
    /// that never called `listen` passes, and fails at its first accept in the serve loop.
    #[cfg(not(any(
        target_os = "aix",
        target_os = "android",
        target_os = "freebsd",
        target_os = "fuchsia",
        target_os = "linux",
    )))]
    fn bound_port(index: usize, addr: SocketAddr) -> bool {
        let port = addr.port();
        if port == 0 {
            return false;
        }
        tracing::warn!(
            "inherited descriptor #{index} is accepted on its bound port {port}: this platform reports no listen state, so a socket that never called `listen` fails at its first accept instead"
        );
        true
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

    #[cfg(test)]
    mod tests {
        use std::net::{Ipv4Addr, SocketAddr};
        use std::os::fd::AsFd;

        use socket2::{Domain, SockAddr, Socket, Type};

        use super::vet;

        fn bound() -> Socket {
            let socket = Socket::new(Domain::IPV4, Type::STREAM, None).expect("a TCP socket");
            socket.bind(&SockAddr::from(SocketAddr::from((Ipv4Addr::LOCALHOST, 0)))).expect("bound to loopback");
            socket
        }

        // Where the bound port stands in for the listen state, a socket only bound passes.
        #[cfg(any(
            target_os = "aix",
            target_os = "android",
            target_os = "freebsd",
            target_os = "fuchsia",
            target_os = "linux",
            target_os = "dragonfly",
            target_os = "netbsd",
            target_os = "openbsd",
            target_vendor = "apple",
        ))]
        #[test]
        fn a_socket_only_bound_is_refused() {
            assert!(!vet(0, bound().as_fd()), "a bound socket that never called `listen` passed the check");
        }

        #[test]
        fn a_listening_socket_is_accepted() {
            let socket = bound();
            socket.listen(16).expect("listening");
            assert!(vet(0, socket.as_fd()), "a listening socket was refused");
        }
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
