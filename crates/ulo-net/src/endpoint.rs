use std::borrow::Cow;
use std::error::Error;
use std::fmt;
use std::net::SocketAddr;
use std::str::FromStr;

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
    pub fn resolve(&self) -> Result<Endpoint, EndpointError> {
        match &self.0 {
            Spec::Parsed(endpoint) => Ok(endpoint.clone()),
            Spec::Text(text) => Endpoint::parse(text),
        }
    }
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
