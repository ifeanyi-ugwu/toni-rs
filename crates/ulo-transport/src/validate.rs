//! Declarative, opt-in validation through the parameter type (transports DESIGN §2.2).

use std::any::type_name;
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
///
/// `Invalid`'s `param` is `P`'s type name without its path or generic arguments, `"Json"` for
/// `Valid<Json<NewUser>>`; each violation names the field that failed.
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
        async move {
            let value = match P::from_call(cx).await {
                Ok(value) => value,
                Err(err) => return Err(err),
            };
            match value.validate() {
                Ok(()) => Ok(Valid(value)),
                Err(violations) => Err(ExtractError::Invalid { param: outer_name(type_name::<P>()), violations }),
            }
        }
    }
}

/// The outermost type's last path segment, borrowed from `full` so it stays `'static`:
/// `ulo_http::extract::Json<app::NewUser>` gives `Json`. A tuple, a reference or any other name
/// that is not a path is kept whole.
fn outer_name(full: &'static str) -> &'static str {
    let head = &full[..full.find('<').unwrap_or(full.len())];
    let is_path = !head.is_empty() && head.chars().all(|c| c.is_alphanumeric() || c == '_' || c == ':');
    if !is_path {
        return full;
    }
    match head.rfind("::") {
        Some(at) => &head[at + "::".len()..],
        None => head,
    }
}

/// Rules written for the `validator` crate, read as [`FieldViolation`]s. The bridge is this one
/// function: a type validated by `validator` implements [`Validate`](crate::Validate) by calling
/// it.
///
/// ```ignore
/// impl ulo_transport::Validate for NewUser {
///     fn validate(&self) -> Result<(), Vec<FieldViolation>> {
///         validator::Validate::validate(self).map_err(|e| validator_bridge::violations(&e))
///     }
/// }
/// ```
#[cfg(feature = "validator")]
pub mod validator_bridge {
    use validator::{ValidationError, ValidationErrors, ValidationErrorsKind};

    use crate::details::FieldViolation;

    /// Every field error in `errors`, nested paths joined with `.` and list items indexed as
    /// `items[0]`, as one violation each, ordered by field. The description is the rule's
    /// message where one is set, otherwise its code and parameters; the rejected value, which
    /// `validator` records as the `value` parameter, is left out, since it may be a secret.
    pub fn violations(errors: &ValidationErrors) -> Vec<FieldViolation> {
        let mut out = Vec::new();
        collect(errors, "", &mut out);
        out.sort_by(|a, b| a.field.cmp(&b.field));
        out
    }

    fn collect(errors: &ValidationErrors, prefix: &str, out: &mut Vec<FieldViolation>) {
        for (name, kind) in errors.errors() {
            let path = if prefix.is_empty() { name.to_string() } else { format!("{prefix}.{name}") };
            match kind {
                ValidationErrorsKind::Field(failures) => {
                    out.extend(failures.iter().map(|failure| FieldViolation::new(path.clone(), describe(failure))));
                }
                ValidationErrorsKind::Struct(nested) => collect(nested, &path, out),
                ValidationErrorsKind::List(items) => {
                    for (index, nested) in items {
                        collect(nested, &format!("{path}[{index}]"), out);
                    }
                }
            }
        }
    }

    fn describe(failure: &ValidationError) -> String {
        if let Some(message) = &failure.message {
            return message.to_string();
        }
        let mut params: Vec<_> = failure.params.iter().filter(|(name, _)| **name != "value").collect();
        params.sort_by(|a, b| a.0.cmp(b.0));
        let listed: Vec<String> = params.iter().map(|(name, value)| format!("{name} = {value}")).collect();
        if listed.is_empty() {
            format!("failed the `{}` rule", failure.code)
        } else {
            format!("failed the `{}` rule ({})", failure.code, listed.join(", "))
        }
    }
}
