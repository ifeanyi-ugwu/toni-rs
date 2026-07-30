use std::fmt;
use std::io;
use std::net::TcpListener;

/// Where an HTTP adapter should listen: an address to bind, or a socket that
/// is already bound and listening.
///
/// Passed to [`use_http_adapter`](crate::ToniApplication::use_http_adapter)
/// and threaded into [`HttpAdapter::into_lifecycle`](crate::adapter::HttpAdapter::into_lifecycle).
/// Converts from the common shapes:
///
/// ```no_run
/// # use toni::BindTarget;
/// let by_addr: BindTarget = ("127.0.0.1", 3000).into();
///
/// let listener = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
/// let by_listener: BindTarget = listener.into();
/// ```
///
/// The `Listener` form serves callers that acquire the socket before the
/// application does: test harnesses that bind port 0 themselves, and
/// supervisors that pass sockets across restarts (systemd socket activation,
/// systemfd — pair with the `listenfd` crate to claim the inherited fd).
///
/// Not every adapter can adopt a pre-bound listener; those that cannot
/// (rocket) return an error from `bind()` when given one. The per-adapter
/// capability matrix in the docs tracks support.
#[non_exhaustive]
#[derive(Debug)]
pub enum BindTarget {
    /// Bind a fresh socket at `hostname:port`. Port 0 asks the OS for a free
    /// port; read the assigned address from
    /// [`BoundAdapters`](crate::BoundAdapters).
    Addr { hostname: String, port: u16 },
    /// Adopt a socket that is already bound and listening. The adapter must
    /// not bind; it converts the listener to its native type — for tokio-based
    /// adapters, `set_nonblocking(true)` then `from_std`.
    Listener(TcpListener),
}

impl BindTarget {
    /// Resolve to a bound std listener: binds for `Addr`, passes `Listener`
    /// through untouched. The listener is left in blocking mode either way;
    /// the adapter applies whatever mode its runtime needs.
    pub fn into_std_listener(self) -> io::Result<TcpListener> {
        match self {
            BindTarget::Addr { hostname, port } => TcpListener::bind((hostname.as_str(), port)),
            BindTarget::Listener(listener) => Ok(listener),
        }
    }

    /// The port this target will listen on, where knowable without binding:
    /// the declared port for `Addr` (0 = OS-assigned, not yet known), the
    /// bound port for `Listener`.
    pub(crate) fn port_hint(&self) -> Option<u16> {
        match self {
            BindTarget::Addr { port, .. } => Some(*port),
            BindTarget::Listener(listener) => listener.local_addr().ok().map(|a| a.port()),
        }
    }
}

impl fmt::Display for BindTarget {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            BindTarget::Addr { hostname, port } => write!(f, "{hostname}:{port}"),
            BindTarget::Listener(listener) => match listener.local_addr() {
                Ok(addr) => write!(f, "pre-bound listener on {addr}"),
                Err(_) => write!(f, "pre-bound listener"),
            },
        }
    }
}

impl From<(&str, u16)> for BindTarget {
    fn from((hostname, port): (&str, u16)) -> Self {
        BindTarget::Addr {
            hostname: hostname.to_string(),
            port,
        }
    }
}

impl From<(String, u16)> for BindTarget {
    fn from((hostname, port): (String, u16)) -> Self {
        BindTarget::Addr { hostname, port }
    }
}

impl From<TcpListener> for BindTarget {
    fn from(listener: TcpListener) -> Self {
        BindTarget::Listener(listener)
    }
}
