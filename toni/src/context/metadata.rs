//! What a handler declares about itself, for an enhancer to read back.
//!
//! Written by `#[set_metadata(...)]` at expansion and shared unchanged by every execution, where
//! [`Extensions`](super::Extensions) holds what one execution puts there and discards after.
//!
//! # Example
//!
//! ```
//! use toni::context::Metadata;
//!
//! #[derive(Clone)]
//! struct Roles(Vec<&'static str>);
//!
//! let mut metadata = Metadata::new();
//! metadata.insert(Roles(vec!["admin", "moderator"]));
//!
//! // Later, in a guard:
//! if let Some(Roles(required)) = metadata.get::<Roles>() {
//!     // Check user has required roles
//! }
//! ```

use crate::type_map::TypeMap;

/// A handler's declared configuration — roles, a rate-limit tier, a feature flag.
///
/// Built once at registration from the `#[set_metadata]` entries on the handler and on its impl
/// block, and read through [`HandlerContext::metadata`](super::HandlerContext::metadata) on every
/// transport. A type nothing declared reads back as absent rather than as an error, which is what
/// lets one guard serve annotated and unannotated handlers alike.
#[derive(Clone, Default)]
pub struct Metadata {
    data: TypeMap,
}

impl Metadata {
    pub fn new() -> Self {
        Self::default()
    }

    /// Record one declaration, keeping any already recorded for `T`.
    ///
    /// The macro inserts the impl block's entries before the handler's, so the order kept here is
    /// least-specific first.
    pub fn insert<T: Clone + Send + Sync + 'static>(&mut self, val: T) {
        match self.data.get_mut::<Vec<T>>() {
            Some(all) => all.push(val),
            None => {
                self.data.insert(vec![val]);
            }
        }
    }

    /// The declaration that wins for `T` — the most specific one, which is the handler's where it
    /// declared `T` and the impl block's otherwise.
    pub fn get<T: Clone + Send + Sync + 'static>(&self) -> Option<&T> {
        self.data.get::<Vec<T>>().and_then(|all| all.last())
    }

    /// Every declaration of `T`, least-specific first.
    ///
    /// Reach for this where declarations accumulate rather than replace — roles a handler adds to
    /// its controller's, tags that collect. Nothing is combined here: what it means to combine two
    /// `T`s is known where `T` is defined, not here.
    ///
    /// ```rust,ignore
    /// let required: Vec<&str> = metadata
    ///     .get_all::<Roles>()
    ///     .iter()
    ///     .flat_map(|r| r.0.iter().copied())
    ///     .collect();
    /// ```
    pub fn get_all<T: Clone + Send + Sync + 'static>(&self) -> &[T] {
        self.data
            .get::<Vec<T>>()
            .map(|all| all.as_slice())
            .unwrap_or(&[])
    }

    pub fn is_empty(&self) -> bool {
        self.data.is_empty()
    }
}

impl std::fmt::Debug for Metadata {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("Metadata").finish()
    }
}
