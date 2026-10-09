//! The types [`Server`](crate::Server)'s and [`Embedded`](crate::embed::Embedded)'s builders take
//! that another `ulo` crate defines, re-exported so an application configuring HTTP imports them
//! from one place and needs no `ulo-net` or `ulo-transport` dependency of its own:
//!
//! ```ignore
//! use std::time::Duration;
//! use ulo_http::config::{Bound, Count, Endpoint, Tls};
//!
//! ulo_http_hyper::Server::new(Endpoint::inherited("http"))
//!     .tls(Tls::from_pem_files("cert.pem", "key.pem"))
//!     .max_inflight(Count::Max(256))
//!     .header_timeout(Bound::After(Duration::from_secs(10)))
//! ```
//!
//! Each name is the defining crate's type, not a copy: `ulo_http::config::Count` and
//! `ulo_transport::Count` are one type, and either path fits wherever the other is expected. The
//! defining crates stay the canonical homes and document each type: [`Bound`] is `ulo`'s,
//! [`Count`] is `ulo-transport`'s, and [`Endpoint`], [`EndpointSpec`] and [`Tls`] are `ulo-net`'s.

#[doc(no_inline)]
pub use ulo::Bound;
#[doc(no_inline)]
pub use ulo_net::{Endpoint, EndpointSpec, Tls};
#[doc(no_inline)]
pub use ulo_transport::Count;
