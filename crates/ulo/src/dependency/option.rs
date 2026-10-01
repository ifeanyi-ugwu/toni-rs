use crate::dependency::{FromContainer, Requirement};
use crate::error::LookupError;
use crate::resolver::Resolver;
use crate::scope::{AllowedIn, Scope};

/// `None` exactly when `S` would fail with `LookupError::NotFound`: a key no module binds, an
/// extension no guard has written, an input this execution did not seed. Every other error
/// propagates: a construction failure, `ExecutionRequired`, an ambiguous module.
impl<S: FromContainer> FromContainer for Option<S> {
    fn describe(req: &mut Requirement) {
        req.optional::<S>();
    }

    async fn read(r: &Resolver<'_>) -> Result<Self, LookupError> {
        match S::read(r).await {
            Ok(value) => Ok(Some(value)),
            Err(LookupError::NotFound { .. }) => Ok(None),
            Err(other) => Err(other),
        }
    }
}

impl<X: AllowedIn<S>, S: Scope> AllowedIn<S> for Option<X> {}
