//! The `HttpServiceFactory` over the app: a scope at a path, or the default service, building the
//! app's `Request` from actix's with the prefix stripped and the payload pumped.

use std::borrow::Cow;

use actix_web::dev::{AppService, HttpServiceFactory};

use crate::Handle;

/// The service an actix `App` mounts, from [`scope`].
#[derive(Clone)]
pub struct ActixScope {
    pub(crate) path: Cow<'static, str>,
    pub(crate) handle: Handle,
}

/// The app at `path`, `App::service(ulo_http_actix::scope("/api", &embedded))`; `""` mounts it as
/// the default service.
pub fn scope(path: impl Into<Cow<'static, str>>, handle: &Handle) -> ActixScope {
    ActixScope { path: path.into(), handle: handle.clone() }
}

impl HttpServiceFactory for ActixScope {
    fn register(self, config: &mut AppService) {
        let _ = (config, &self.path, &self.handle);
        todo!()
    }
}
