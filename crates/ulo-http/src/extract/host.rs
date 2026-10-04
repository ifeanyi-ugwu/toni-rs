use std::any::type_name;
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
/// form when the app listens. Guards and services read a host value as `Ext<T>` once a
/// pre-dispatch `adopt::<T>()` entry has copied it into the execution.
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
