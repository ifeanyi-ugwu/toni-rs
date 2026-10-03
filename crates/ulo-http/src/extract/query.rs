use std::future::Future;
use std::ops::Deref;

use serde::de::DeserializeOwned;
use ulo_transport::{ExtractError, FieldViolation, FromCall, Validate};

use crate::cx::HttpCx;
use crate::extract::de::{Source, from_pairs, parse_form};
use crate::transport::Http;

/// The query string as `T`, decoded as `application/x-www-form-urlencoded`. A missing query is an
/// empty one, so a `T` whose fields are all optional or defaulted always extracts.
///
/// A required field the query lacks is `ExtractError::Missing` naming it, so `Option<Query<T>>` is
/// `None` then; a value that does not parse is `Malformed` naming it.
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
        async move {
            let decoded = parse_form(cx.uri().query().unwrap_or("").as_bytes());
            let pairs: Vec<(&str, &str)> = decoded.iter().map(|(name, value)| (name.as_str(), value.as_str())).collect();
            from_pairs::<T>(&pairs, Source::Form).map(Query).map_err(|error| error.into_extract("query", cx))
        }
    }
}

impl<T: Validate> Validate for Query<T> {
    fn validate(&self) -> Result<(), Vec<FieldViolation>> {
        self.0.validate()
    }
}
