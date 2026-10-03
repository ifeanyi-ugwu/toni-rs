//! Support for the code `ulo-http-macros` generates. Not part of the public API: names and shapes
//! here change with the macros.

use std::marker::PhantomData;
use std::sync::Arc;

use serde::de::DeserializeOwned;
use ulo::{BoxError, BoxFuture};

pub use http::Method;
pub use ulo_transport as transport;

pub use crate::extract::PathCheck;

use crate::cx::HttpCx;
use crate::extract::Path;
use crate::response::Response;

/// A route's call: extraction, the controller, the handler and the reply probe, run by `dispatch`
/// after every guard admits.
pub type HandlerFn = Arc<dyn Fn(HttpCx) -> BoxFuture<'static, Result<Response, BoxError>> + Send + Sync>;

/// The HTTP handler value an attribute passes to `HandlerSpec::new`, which the server reads back
/// from `MountedHandler::handler` in `prepare`: method, pattern as written, call, and the
/// `Path<T>` checks of its parameters.
pub struct HttpHandler {
    pub(crate) method: Method,
    pub(crate) path: &'static str,
    pub(crate) call: HandlerFn,
    pub(crate) path_checks: Vec<PathCheck>,
}

impl HttpHandler {
    pub fn new<F>(method: Method, path: &'static str, call: F) -> Self
    where
        F: Fn(HttpCx) -> BoxFuture<'static, Result<Response, BoxError>> + Send + Sync + 'static,
    {
        HttpHandler { method, path, call: Arc::new(call), path_checks: Vec::new() }
    }

    /// One parameter's check, from `PathProbe`: `None` for a parameter that is not a `Path<T>`.
    pub fn path_check(mut self, check: Option<PathCheck>) -> Self {
        self.path_checks.extend(check);
        self
    }

    pub(crate) fn method(&self) -> &Method {
        &self.method
    }
}

/// `(&&PathProbe::<P>::new()).check()`: `Some(PathCheck::of::<T>())` for a `Path<T>` parameter,
/// `None` for any other, ranked by autoref at the concrete parameter type, so an alias of `Path`
/// is checked as one.
pub struct PathProbe<P>(PhantomData<fn() -> P>);

impl<P> PathProbe<P> {
    pub fn new() -> Self {
        PathProbe(PhantomData)
    }
}

impl<P> Default for PathProbe<P> {
    fn default() -> Self {
        PathProbe::new()
    }
}

pub trait ViaPath {
    fn check(&self) -> Option<PathCheck>;
}

impl<T: DeserializeOwned> ViaPath for &PathProbe<Path<T>> {
    fn check(&self) -> Option<PathCheck> {
        Some(PathCheck::of::<T>())
    }
}

pub trait NotPath {
    fn check(&self) -> Option<PathCheck>;
}

impl<P> NotPath for PathProbe<P> {
    fn check(&self) -> Option<PathCheck> {
        None
    }
}
