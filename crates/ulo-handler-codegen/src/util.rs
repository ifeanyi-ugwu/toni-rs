//! Helpers every macro built on this crate uses, `ulo-macros` included.

use syn::spanned::Spanned;
use syn::{Expr, ExprClosure, ReturnType};

/// A factory's parameters are injection points, read by type, so each needs its type written.
/// Checked to report the error on the parameter rather than on a factory bound the closure fails
/// as a whole.
pub fn check_factory_params(closure: &syn::ExprClosure) -> syn::Result<()> {
    for input in &closure.inputs {
        if !matches!(input, syn::Pat::Type(_)) {
            return Err(syn::Error::new_spanned(input, "a factory parameter needs its type written, as in `cfg: Dep<DbConfig>`"));
        }
    }
    Ok(())
}

/// Every error in `errors` as one, so a single expansion reports all of them; `None` when empty.
pub fn combine(errors: Vec<syn::Error>) -> Option<syn::Error> {
    errors.into_iter().reduce(|mut all, e| {
        all.combine(e);
        all
    })
}

/// `|u: Ext<CurrentUser>| RoleGuard::require(u)` becomes `|u: Ext<CurrentUser>| async move {
/// RoleGuard::require(u) }`. An explicit return type moves onto a binding inside the block,
/// since an `async` block cannot carry one. An enhancer attribute's `with` entry, `#[module]`'s
/// `into` lists and a gateway's `connect_guards` and `session_with` share it, so a closure is
/// written the same way in each.
pub fn wrap_async(closure: &ExprClosure) -> ExprClosure {
    if closure.asyncness.is_some() || matches!(*closure.body, Expr::Async(_)) {
        return closure.clone();
    }
    let mut wrapped = closure.clone();
    let body = &closure.body;
    let new_body: Expr = match &closure.output {
        ReturnType::Default => syn::parse_quote_spanned! {body.span()=> async move { #body } },
        ReturnType::Type(_, ty) => syn::parse_quote_spanned! {body.span()=>
            async move {
                let __ulo_built: #ty = #body;
                __ulo_built
            }
        },
    };
    wrapped.output = ReturnType::Default;
    wrapped.body = Box::new(new_body);
    wrapped
}
