use std::any::{TypeId, type_name};
use std::future::Future;
use std::ops::Deref;

use ulo_transport::{ExtractError, FromCall};

use crate::cx::HttpCx;
use crate::transport::Http;

const HOST: &str = "host";

/// A value the server hosting the application put in the request's `http::Extensions`: what a
/// host's auth layer hands an embedded app, `Host<CurrentUser>`. Read from the request as the
/// pre-dispatch stage left it, so a pre-dispatch tower layer's insertion counts too.
///
/// Absent is `ExtractError::HostMissing`, a deployment fault: it renders 500 with its message
/// withheld. `Option<Host<T>>` is the spelling for a value the host may leave out, `None` then.
/// An embedding whose adapter declares `host_extensions: false` refuses a handler reading either
/// form when the app listens, unless something declares it supplies `T`: a pre-dispatch
/// `.supplies::<T>()` after the entry that inserts it, or `Embedded::supplies::<T>()` for a value
/// the adapter inserts. Guards and services read a host value as `Ext<T>` once a pre-dispatch
/// `adopt::<T>()` entry has copied it into the execution.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct Host<T>(pub T);

impl<T> Deref for Host<T> {
    type Target = T;

    fn deref(&self) -> &T {
        &self.0
    }
}

impl<T: Clone + Send + Sync + 'static> FromCall<Http> for Host<T> {
    fn from_call(cx: &HttpCx) -> impl Future<Output = Result<Self, ExtractError>> + Send {
        let value = cx.head().parts().extensions.get::<T>().cloned();
        async move { value.map(Host).ok_or(ExtractError::HostMissing { param: HOST, type_name: type_name::<T>() }) }
    }
}

/// A type a host value is read or supplied under, for the check an embedding declaring
/// `host_extensions: false` runs in `prepare`: compared by `id`, named by `name`.
#[derive(Clone, Copy, Debug)]
pub(crate) struct HostType {
    pub(crate) id: TypeId,
    pub(crate) name: &'static str,
}

impl HostType {
    pub(crate) fn of<T: 'static>() -> Self {
        HostType { id: TypeId::of::<T>(), name: type_name::<T>() }
    }
}
