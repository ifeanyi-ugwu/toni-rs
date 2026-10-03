use std::future::Future;
use std::ops::Deref;

use serde::de::DeserializeOwned;
use ulo_transport::{ExtractError, FieldViolation, FromCall, Validate};

use crate::cx::HttpCx;
use crate::transport::Http;

/// The query string as `T`, decoded as `application/x-www-form-urlencoded`. A missing query is an
/// empty one, so a `T` whose fields are all optional or defaulted always extracts.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct Query<T>(pub T);

impl<T> Deref for Query<T> {
    type Target = T;

    fn deref(&self) -> &T {
        &self.0
    }
}

impl<T: DeserializeOwned + Send + 'static> FromCall<Http> for Query<T> {
    fn from_call(cx: &HttpCx) -> impl Future<Output = Result<Self, ExtractError>> + Send {
        let _ = cx;
        async { todo!("`serde_urlencoded` over the URI's query; failure is `Malformed { param: \"query\" }`, redacted through the app") }
    }
}

impl<T: Validate> Validate for Query<T> {
    fn validate(&self) -> Result<(), Vec<FieldViolation>> {
        self.0.validate()
    }
}
