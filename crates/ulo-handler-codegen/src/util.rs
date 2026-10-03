//! Helpers every macro built on this crate uses, `ulo-macros` included.

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
