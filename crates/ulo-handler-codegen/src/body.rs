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
use quote::{quote, quote_spanned};

use crate::params::Param;
use crate::paths::Paths;

/// The initializer of `__ULO_CHECKS_<name>`: a block of every pairwise assertion, each spanned on
/// the pair's second parameter; `()` for fewer than two parameters.
pub fn consumer_checks(params: &[Param], paths: &Paths) -> TokenStream {
    if params.len() < 2 {
        return quote!(());
    }
    let transport = &paths.transport;
    let marker = &paths.marker;
    let mut assertions = Vec::with_capacity(params.len() * (params.len() - 1) / 2);
    for (position, first) in params.iter().enumerate() {
        for second in &params[position + 1..] {
            let message = format!(
                "`{}` and `{}` both consume the body; a handler reads the body once",
                escape_braces(&first.name),
                escape_braces(&second.name),
            );
            let (first_ty, second_ty) = (&first.ty, &second.ty);
            assertions.push(quote_spanned! {second.span=>
                ::core::assert!(
                    !(<#first_ty as #transport::__private::Param<#marker, _>>::CONSUMES_BODY
                        && <#second_ty as #transport::__private::Param<#marker, _>>::CONSUMES_BODY),
                    #message
                );
            });
        }
    }
    quote!({ #(#assertions)* })
}

/// The message is `assert!`'s format string, and a destructuring pattern's name can carry braces,
/// as in `User { id, .. }`.
fn escape_braces(name: &str) -> String {
    name.replace('{', "{{").replace('}', "}}")
}
