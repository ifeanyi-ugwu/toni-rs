use std::net::SocketAddr;

use http::request::Parts;
use http::{HeaderMap, Method, Uri, Version};
use ulo::{Inputs, Transport};

use crate::cx::HttpCx;
use crate::response::Response;

/// The HTTP transport, key `"http"`: `#[guards(http = AuthGuard)]` scopes an entry to its
/// handlers. Its inputs, [`RequestHead`] and [`ClientAddr`], are seeded into every request's
/// execution and declared by the transport itself, so an application imports no module for them.
pub struct Http;

impl Transport for Http {
    const KEY: &'static str = "http";

    type Cx = HttpCx;
    type Reply = Response;

    fn inputs(d: &mut Inputs) {
        d.input::<RequestHead>().input::<ClientAddr>();
    }
}

/// The request line and headers, as an execution input: `Dep<RequestHead>` in an execution-scoped
/// service. Read as `Option<Dep<RequestHead>>` in a service shared with another transport, where
/// it is `None`.
#[derive(Clone, Debug)]
pub struct RequestHead {
    pub(crate) parts: Parts,
}

impl RequestHead {
    pub fn method(&self) -> &Method {
        &self.parts.method
    }

    pub fn uri(&self) -> &Uri {
        &self.parts.uri
    }

    pub fn version(&self) -> Version {
        self.parts.version
    }

    pub fn headers(&self) -> &HeaderMap {
        &self.parts.headers
    }

    /// The path as the client sent it, before any pre-dispatch rewrite.
    pub fn path(&self) -> &str {
        self.parts.uri.path()
    }

    pub fn parts(&self) -> &Parts {
        &self.parts
    }
}

/// The peer's address, as an execution input. Behind a proxy it is the proxy's; a
/// `Forwarded`-aware middleware writes the client's into the extensions.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct ClientAddr(pub SocketAddr);
