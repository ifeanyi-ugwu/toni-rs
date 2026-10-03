//! Declarative, opt-in validation through the parameter type (transports DESIGN §2.2).

use std::future::Future;
use std::ops::Deref;

use ulo::{Dependencies, Transport};

use crate::details::FieldViolation;
use crate::extract::{ExtractError, FromCall};

/// A value checked against its declared rules after it is extracted. `#[derive(Validate)]` writes
/// it from `#[validate(length(min = 1, max = 64))]`, `#[validate(email)]`,
/// `#[validate(range(min = 13))]` on the fields.
///
/// A transport implements it for its wrapping extractors by delegating to the value inside, as
/// `ulo-http` does for `Json<T>`, `Form<T>`, `Query<T>` and `Path<T>`, so `Valid<Json<NewUser>>`
/// checks the `NewUser`.
pub trait Validate {
    fn validate(&self) -> Result<(), Vec<FieldViolation>>;
}

/// `P`, extracted, then validated: violations fail the call as [`ExtractError::Invalid`], which
/// classifies as `Unprocessable`. Forwards `P`'s `CONSUMES_BODY` and dependencies, as
/// `Option<P>` does.
pub struct Valid<P>(pub P);

impl<P> Valid<P> {
    pub fn into_inner(self) -> P {
        self.0
    }
}

impl<P> Deref for Valid<P> {
    type Target = P;

    fn deref(&self) -> &P {
        &self.0
    }
}

impl<T: Transport, P: FromCall<T> + Validate> FromCall<T> for Valid<P> {
    const CONSUMES_BODY: bool = P::CONSUMES_BODY;

    fn dependencies(d: &mut Dependencies) {
        P::dependencies(d);
    }

    fn from_call(cx: &T::Cx) -> impl Future<Output = Result<Self, ExtractError>> + Send {
        let _ = cx;
        async { todo!("`P::from_call`, then `validate`; violations → `ExtractError::Invalid` with `param` naming `P`") }
    }
}

/// Rules written for the `validator` crate, read as [`FieldViolation`]s.
#[cfg(feature = "validator")]
pub mod validator_bridge {
    use crate::details::FieldViolation;

    /// Every field error in `errors`, nested paths joined with `.`, as one violation each.
    pub fn violations(errors: &validator::ValidationErrors) -> Vec<FieldViolation> {
        let _ = errors;
        todo!("flatten `errors` into field violations")
    }
}
