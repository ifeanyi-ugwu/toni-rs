use std::future::Future;
use std::ops::Deref;

use serde::de::DeserializeOwned;
use ulo_transport::{ExtractError, FieldViolation, FromCall, Validate};

use crate::cx::HttpCx;
use crate::transport::Http;

/// The matched route's path parameters as `T`: a struct by field name (serde renames included), a
/// tuple by position, a scalar or newtype from the one parameter.
///
/// Checked against its route when the server prepares: `T`'s `Deserialize` impl runs against a
/// deserializer that records what it asks for, so a field the route does not name, a tuple of the
/// wrong length, or a scalar on a route with other than one parameter is a startup error rather
/// than the first request's. A map, a `#[serde(flatten)]` struct and a hand-written impl calling
/// `deserialize_any` ask for no names and are not checked.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct Path<T>(pub T);

impl<T> Deref for Path<T> {
    type Target = T;

    fn deref(&self) -> &T {
        &self.0
    }
}

impl<T: DeserializeOwned + Send + 'static> FromCall<Http> for Path<T> {
    fn from_call(cx: &HttpCx) -> impl Future<Output = Result<Self, ExtractError>> + Send {
        let _ = cx;
        async { todo!("deserialize the matched route's parameters; a missing one is `Missing`, a bad one `Malformed` naming it") }
    }
}

impl<T: Validate> Validate for Path<T> {
    fn validate(&self) -> Result<(), Vec<FieldViolation>> {
        self.0.validate()
    }
}

/// The startup check of one `Path<T>` parameter against its route, which a handler attribute
/// records on the handler value through `ulo_http::__private::PathProbe`.
#[derive(Clone, Copy)]
pub struct PathCheck {
    pub(crate) check: fn(&[&str]) -> Result<(), String>,
    pub(crate) type_name: &'static str,
}

impl PathCheck {
    /// The check of `T`: its `Deserialize` impl against a recording deserializer, the route's
    /// parameter names given.
    pub fn of<T: DeserializeOwned>() -> PathCheck {
        PathCheck { check: check_path::<T>, type_name: std::any::type_name::<T>() }
    }
}

/// Runs `T`'s `Deserialize` impl against a deserializer that records the names, the tuple length
/// or the single value it asks for, and compares them with `params`, the route's parameter names.
pub(crate) fn check_path<T: DeserializeOwned>(params: &[&str]) -> Result<(), String> {
    let _ = params;
    todo!("the recording deserializer: struct → field names ⊆ params and every param named; tuple → len; scalar/newtype → exactly one; map/any → unchecked")
}
