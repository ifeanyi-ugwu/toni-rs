use syn::LitStr;

/// Reject Express-style `:param` route segments at compile time.
///
/// The parameter syntax is `{param}`, on every adapter. A `:` is a legal
/// literal path character mid-segment (`/users/{id}:activate`), so only a
/// segment-leading colon is rejected.
pub fn validate_route_path(lit: &LitStr) -> syn::Result<()> {
    for segment in lit.value().split('/') {
        if let Some(name) = segment.strip_prefix(':') {
            return Err(syn::Error::new(
                lit.span(),
                format!(
                    "route path segment `{segment}` uses `:param` syntax; \
                     toni's parameter syntax is `{{{name}}}`"
                ),
            ));
        }
    }
    Ok(())
}
