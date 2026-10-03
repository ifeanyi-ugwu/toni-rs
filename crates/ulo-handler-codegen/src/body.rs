//! The single body consumer, checked at compile time and naming both parameters (transports
//! DESIGN §2.2).
//!
//! For each pair of parameters one assertion, n(n−1)/2 for n parameters, none skipped by
//! spelling: `CONSUMES_BODY` is a property of the type, so `type Body<T> = Json<T>` consumes
//! whatever it is called.
//!
//! ```text
//! assert!(
//!     !(<Json<NewUser> as Param<Http, _>>::CONSUMES_BODY && <Form<Login> as Param<Http, _>>::CONSUMES_BODY),
//!     "`user` and `login` both consume the body; a handler reads the body once"
//! );
//! ```
//!
//! The assertions read `Param<T, _>`, whose marker inference covers a `FromContainer` parameter
//! as well as a `FromCall` one; `<UserType as FromCall<T>>` would not compile for a container
//! type. They sit inside the `__ULO_CHECKS_<name>` constant, which `#[routes]` evaluates.

use proc_macro2::TokenStream;

use crate::params::Param;
use crate::paths::Paths;

/// The initializer of `__ULO_CHECKS_<name>`: a block of every pairwise assertion, each spanned on
/// the pair's second parameter; `()` for fewer than two parameters.
pub fn consumer_checks(params: &[Param], paths: &Paths) -> TokenStream {
    let _ = (params, paths);
    todo!("one `assert!` per pair (a, b), a before b, spanned at b.span, message \"`{a}` and `{b}` both consume the body; a handler reads the body once\"")
}
