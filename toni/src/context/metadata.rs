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

    pub fn insert<T: Clone + Send + Sync + 'static>(&mut self, val: T) -> Option<T> {
        self.data.insert(val)
    }

    pub fn get<T: Send + Sync + 'static>(&self) -> Option<&T> {
        self.data.get()
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
