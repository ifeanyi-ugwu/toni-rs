//! The route-pattern grammar, checked at compile time on the attribute's literal. The same
//! grammar as `ulo_http::router::pattern`, which parses it again at `prepare`; the two are kept in
//! step by hand, since this crate cannot depend on the crate that re-exports it.

/// Why `text` is not a route pattern, or `Ok`: a leading `/`; segments of static text, `{name}`
/// with an identifier-like name, or a final `{*name}`; no brace elsewhere; no name twice.
pub(crate) fn check(text: &str) -> Result<(), &'static str> {
    let _ = text;
    todo!("the grammar above")
}
