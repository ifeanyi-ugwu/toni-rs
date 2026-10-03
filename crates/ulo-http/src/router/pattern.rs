//! Route patterns (transports DESIGN §3.2): `{name}` segments plus an optional trailing
//! `{*rest}`. A trailing slash is insignificant: the pattern and the request path both drop it,
//! except for `/`.
//!
//! `ulo-http-macros` checks a literal against the same grammar at compile time, so a malformed
//! literal such as `/u/{id` is a span error; the two parsers are kept in step by hand.

use std::fmt;
use std::sync::Arc;

use crate::cx::PathParams;

/// A parsed route pattern.
#[derive(Clone, Debug)]
pub(crate) struct Pattern {
    /// Normalized: no trailing slash except for `/`.
    pub(crate) raw: Arc<str>,
    pub(crate) segments: Vec<Segment>,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) enum Segment {
    Static(String),
    Param(Arc<str>),
    /// `{*rest}`, last only: the remaining path, one or more segments.
    Rest(Arc<str>),
}

/// Why a pattern does not parse.
#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) struct PatternError {
    pub(crate) pattern: String,
    pub(crate) reason: &'static str,
}

impl Pattern {
    pub(crate) fn parse(text: &str) -> Result<Pattern, PatternError> {
        let _ = text;
        todo!("split on `/`; `{name}` a parameter, `{*name}` the rest (last only); braces elsewhere, empty or repeated names refused")
    }

    /// `prefix` and `route` joined with exactly one `/` between them, as `.at(prefix)` applies a
    /// controller's prefix.
    pub(crate) fn join(prefix: &str, route: &str) -> String {
        let _ = (prefix, route);
        todo!("trim the slashes at the seam, keep a leading `/`")
    }

    /// The same method on two patterns that are equal after normalization, or that differ only in
    /// a parameter's name at one position, is a duplicate route.
    pub(crate) fn conflicts(&self, other: &Pattern) -> bool {
        let _ = other;
        todo!("segment-wise: statics equal, parameters at the same positions regardless of name")
    }

    /// The parameters of `path`, percent-decoded, when it matches.
    pub(crate) fn matches(&self, path: &str) -> Option<PathParams> {
        let _ = path;
        todo!("segment-wise match after dropping a trailing slash")
    }

    pub(crate) fn param_names(&self) -> impl Iterator<Item = &str> + '_ {
        self.segments.iter().filter_map(|segment| match segment {
            Segment::Param(name) | Segment::Rest(name) => Some(&**name),
            Segment::Static(_) => None,
        })
    }
}

impl fmt::Display for Pattern {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(&self.raw)
    }
}

/// A pre-dispatch scope, `/admin/*`: matched against route patterns when the server prepares, so
/// each route's scoped stack is built once. `*` as the last segment matches one or more segments;
/// a scope without one matches its pattern exactly.
#[derive(Clone, Debug)]
pub(crate) struct ScopePattern {
    pub(crate) raw: Arc<str>,
}

impl ScopePattern {
    pub(crate) fn parse(text: &str) -> Result<ScopePattern, PatternError> {
        let _ = text;
        todo!("a route pattern whose last segment may be `*`")
    }

    /// Whether this scope covers the route `pattern`.
    pub(crate) fn covers(&self, pattern: &Pattern) -> bool {
        let _ = pattern;
        todo!("prefix match on segments, a parameter in the scope matching a parameter in the route")
    }

    /// Whether this scope covers a request path that matched no route: unscoped entries run on
    /// misses, scoped ones never, so this serves only `exclude` on an unscoped entry.
    pub(crate) fn covers_path(&self, path: &str) -> bool {
        let _ = path;
        todo!("segment-wise match of a concrete path")
    }
}
