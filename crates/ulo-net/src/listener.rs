use std::error::Error;
use std::fmt;
use std::io;
use std::net::{SocketAddr, TcpListener};

use crate::activation::{Activation, ActivationError};
use crate::endpoint::Endpoint;

/// A listening socket a server bound or adopted, non-blocking, with the address it listens on.
pub struct BoundListener {
    listener: TcpListener,
    endpoint: Endpoint,
    local: SocketAddr,
}

impl BoundListener {
    /// Binds `endpoint`, or takes the inherited socket it names.
    pub fn bind(endpoint: &Endpoint) -> Result<BoundListener, BindError> {
        let os_error = |source: io::Error| BindError::Io { endpoint: endpoint.clone(), source };
        let listener = match endpoint {
            Endpoint::Addr(addr) => {
                let listener = TcpListener::bind(addr).map_err(os_error)?;
                listener.set_nonblocking(true).map_err(os_error)?;
                listener
            }
            Endpoint::Inherited(name) => Activation::get()
                .and_then(|activation| activation.take(name))
                .map_err(BindError::Activation)?,
        };
        let local = listener.local_addr().map_err(os_error)?;
        Ok(BoundListener { listener, endpoint: endpoint.clone(), local })
    }

    /// The actual address: port 0 reports the port the OS chose.
    pub fn local_addr(&self) -> SocketAddr {
        self.local
    }

    pub fn endpoint(&self) -> &Endpoint {
        &self.endpoint
    }

    /// The socket, for a backend to adopt with its runtime's `from_std`.
    pub fn into_std(self) -> TcpListener {
        self.listener
    }
}

/// Binds every endpoint, all-or-nothing: when one fails, the listeners already opened are closed
/// before the error returns, so a server's `bind` leaves nothing half-bound.
pub fn bind_all(endpoints: &[Endpoint]) -> Result<Vec<BoundListener>, BindError> {
    let mut bound = Vec::with_capacity(endpoints.len());
    for endpoint in endpoints {
        bound.push(BoundListener::bind(endpoint)?);
    }
    Ok(bound)
}

/// Why an endpoint could not be bound.
#[non_exhaustive]
#[derive(Debug)]
pub enum BindError {
    /// The OS refused the address: in use, not available, not permitted.
    Io { endpoint: Endpoint, source: io::Error },
    Activation(ActivationError),
}

impl fmt::Display for BindError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            BindError::Io { endpoint, source } => write!(f, "cannot listen on {endpoint}: {source}"),
            BindError::Activation(error) => fmt::Display::fmt(error, f),
        }
    }
}

impl Error for BindError {
    fn source(&self) -> Option<&(dyn Error + 'static)> {
        match self {
            BindError::Io { source, .. } => Some(source),
            BindError::Activation(error) => Some(error),
        }
    }
}
