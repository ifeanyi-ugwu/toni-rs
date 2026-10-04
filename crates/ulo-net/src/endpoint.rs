use std::borrow::Cow;
use std::error::Error;
use std::fmt;
use std::net::SocketAddr;
use std::str::FromStr;

use crate::activation::{Activation, ActivationError};

/// Where a server listens.
#[derive(Clone, Debug, PartialEq, Eq, Hash)]
pub enum Endpoint {
    /// `"0.0.0.0:8080"`; port 0 lets the OS choose, and `Server::bound` reports its choice.
    Addr(SocketAddr),
    /// A socket from `LISTEN_FDS` (systemd socket activation, or `ulo dev` holding it across
    /// restarts), accepted only when `LISTEN_PID` is this process's ID.
    Inherited(ListenerName),
}

/// Which inherited socket: by its `LISTEN_FDNAMES` name, or by its position among `LISTEN_FDS`.
#[derive(Clone, Debug, PartialEq, Eq, Hash)]
pub enum ListenerName {
    Named(Cow<'static, str>),
    Index(usize),
}

impl Endpoint {
    /// `"0.0.0.0:0"`, `"[::1]:8080"`: a socket address, no name resolution.
    pub fn parse(text: &str) -> Result<Endpoint, EndpointError> {
        text.parse::<SocketAddr>()
            .map(Endpoint::Addr)
            .map_err(|_| EndpointError { text: text.to_owned() })
    }

    /// The inherited socket `LISTEN_FDNAMES` names `name`.
    pub fn inherited(name: impl Into<Cow<'static, str>>) -> Endpoint {
        Endpoint::Inherited(ListenerName::Named(name.into()))
    }

    /// The inherited socket at `index` among `LISTEN_FDS`, from 0 (file descriptor 3).
    pub fn inherited_index(index: usize) -> Endpoint {
        Endpoint::Inherited(ListenerName::Index(index))
    }
}

impl FromStr for Endpoint {
    type Err = EndpointError;

    fn from_str(text: &str) -> Result<Self, Self::Err> {
        Endpoint::parse(text)
    }
}

impl From<SocketAddr> for Endpoint {
    fn from(addr: SocketAddr) -> Self {
        Endpoint::Addr(addr)
    }
}

impl fmt::Display for Endpoint {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Endpoint::Addr(addr) => fmt::Display::fmt(addr, f),
            Endpoint::Inherited(ListenerName::Named(name)) => write!(f, "inherited socket `{name}`"),
            Endpoint::Inherited(ListenerName::Index(index)) => write!(f, "inherited socket #{index}"),
        }
    }
}

/// Text that is not a socket address.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct EndpointError {
    text: String,
}

impl fmt::Display for EndpointError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "`{}` is not a socket address such as `0.0.0.0:8080`", self.text)
    }
}

impl Error for EndpointError {}

/// What a server builder takes: an [`Endpoint`], a `SocketAddr`, or text, which the server parses
/// in `prepare` so a bad address is a `StartupError::Configure` beside every other configuration
/// error rather than a failure in `main`.
#[derive(Clone, Debug)]
pub struct EndpointSpec(Spec);

#[derive(Clone, Debug)]
enum Spec {
    Parsed(Endpoint),
    Text(String),
}

impl EndpointSpec {
    /// The endpoint a server listens on, for its `prepare`.
    ///
    /// Under `ULO_DEV=1`, which `ulo dev` sets in every child it starts, an [`Endpoint::Addr`]
    /// resolves to the inherited socket named after that address when this process holds one, so
    /// an application written `Server::new("0.0.0.0:8080")` adopts the socket `ulo dev --listen
    /// 0.0.0.0:8080` holds across restarts and binds the address itself in production. The
    /// address is compared as parsed, so `"0.0.0.0:8080"`, the `SocketAddr` and
    /// `Endpoint::Addr(..)` match alike. Without the variable, with no socket named after the
    /// address, or with inherited sockets this process cannot adopt (a `LISTEN_PID` naming another
    /// process among them), the address resolves as written. An [`Endpoint::Inherited`] resolves
    /// as written in either case.
    pub fn resolve(&self) -> Result<Endpoint, EndpointError> {
        let endpoint = self.resolve_as_written()?;
        Ok(match endpoint {
            Endpoint::Addr(addr) if dev_mode() => held_by_dev(addr).unwrap_or(endpoint),
            endpoint => endpoint,
        })
    }

    /// The endpoint as written, without the `ULO_DEV` lookup [`resolve`](Self::resolve) makes:
    /// for a client, which connects to the address, and for a socket the activation cannot carry,
    /// such as a UDP link's.
    pub fn resolve_as_written(&self) -> Result<Endpoint, EndpointError> {
        match &self.0 {
            Spec::Parsed(endpoint) => Ok(endpoint.clone()),
            Spec::Text(text) => Endpoint::parse(text),
        }
    }
}

const DEV_VAR: &str = "ULO_DEV";

fn dev_mode() -> bool {
    std::env::var_os(DEV_VAR).is_some_and(|value| value == "1")
}

/// The inherited socket `ulo dev --listen` named after `addr`, when this process holds it.
fn held_by_dev(addr: SocketAddr) -> Option<Endpoint> {
    let activation = match Activation::get() {
        Ok(activation) => activation,
        // A process the application spawns inherits `ULO_DEV` beside a `LISTEN_PID` that is not
        // its own; off Unix nothing is ever inherited.
        Err(error @ (ActivationError::PidMismatch { .. } | ActivationError::Unsupported)) => {
            tracing::debug!("{DEV_VAR}=1, and no inherited socket is this process's ({error}); {addr} binds as written");
            return None;
        }
        Err(error) => {
            tracing::warn!("{DEV_VAR}=1, and the inherited sockets cannot be adopted ({error}); {addr} binds as written");
            return None;
        }
    };
    let name = ListenerName::Named(Cow::Owned(dev_socket_name(addr)));
    activation.contains(&name).then_some(Endpoint::Inherited(name))
}

/// The `LISTEN_FDNAMES` name `ulo dev --listen` gives the socket it holds on `addr`: the address's
/// text with `%` written `%25` and `:` written `%3A`. `LISTEN_FDNAMES` separates names with `:`,
/// which every socket address contains. `ulo-cli`'s `commands/dev.rs` writes the same encoding.
fn dev_socket_name(addr: SocketAddr) -> String {
    let text = addr.to_string();
    let mut name = String::with_capacity(text.len() + 2);
    for c in text.chars() {
        match c {
            '%' => name.push_str("%25"),
            ':' => name.push_str("%3A"),
            c => name.push(c),
        }
    }
    name
}

impl From<Endpoint> for EndpointSpec {
    fn from(endpoint: Endpoint) -> Self {
        EndpointSpec(Spec::Parsed(endpoint))
    }
}

impl From<SocketAddr> for EndpointSpec {
    fn from(addr: SocketAddr) -> Self {
        EndpointSpec(Spec::Parsed(Endpoint::Addr(addr)))
    }
}

impl From<&str> for EndpointSpec {
    fn from(text: &str) -> Self {
        EndpointSpec(Spec::Text(text.to_owned()))
    }
}

impl From<String> for EndpointSpec {
    fn from(text: String) -> Self {
        EndpointSpec(Spec::Text(text))
    }
}
