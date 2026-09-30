//! Typed DI views over the request's [extension bag](crate::context::Extensions).
//!
//! The bag reaches enhancers and handlers on its own. These bring it to the rest
//! of the injection tree — a service two calls below the controller declares what
//! it needs instead of having it threaded down through every signature.
//!
//! ```rust,ignore
//! #[injectable(scope = "execution")]
//! pub struct AuditLog {
//!     #[inject]
//!     user: Extension<CurrentUser>,
//! }
//!
//! impl AuditLog {
//!     pub fn record(&self, action: &str) {
//!         // Whatever the guard put there, however deep this call is.
//!         let who = self.user.get().map(|u| u.id).unwrap_or_default();
//!         tracing::info!(user = %who, action, "audited");
//!     }
//! }
//! ```
//!
//! A view needs no declaration: the container reads it from the execution's bag, the one
//! [`Extensions`](crate::context::Extensions) it answers every module with, and code outside the
//! injection tree reads that bag with `resolve::<Extensions>`.
//!
//! # Scope
//!
//! A view reads the execution's bag, so a singleton cannot hold one: it would
//! serve one execution's values to every later one. `create` refuses it with
//! `ExecutionRequired` naming `Extensions`, the provider a view reads. A
//! `provide!` factory's parameters are not injection sites: a factory takes
//! `Arc<Extensions>` and reads the payload from the bag.

use std::any::Any;
use std::marker::PhantomData;
use std::sync::Arc;

use crate::FxHashMap;
use crate::async_trait;
use crate::context::Extensions;
use crate::di::Execution;
use crate::di::ProviderScope;
use crate::error::{BuildResult, ResolutionError};
use crate::spi::{Provider, ProviderFactory, Registration};
/// An injectable view of one type in the request's extension bag.
///
/// Holds the bag itself, not a copy of the value, so a `set` from a guard is
/// visible to every other holder in the same request — including ones
/// constructed before the write happened.
pub struct Extension<T> {
    bag: Extensions,
    _marker: PhantomData<fn() -> T>,
}

// Manual: `T` is only a key here, so the view clones regardless of whether the
// payload does.
impl<T> Clone for Extension<T> {
    fn clone(&self) -> Self {
        Self {
            bag: self.bag.clone(),
            _marker: PhantomData,
        }
    }
}

impl<T> Extension<T> {
    pub(crate) fn over(bag: Extensions) -> Self {
        Self {
            bag,
            _marker: PhantomData,
        }
    }

    /// The whole bag, for reading types other than `T`.
    pub fn bag(&self) -> &Extensions {
        &self.bag
    }
}

impl<T: Send + Sync + 'static> Extension<T> {
    /// Attach the value for this request, returning what it replaced.
    pub fn set(&self, value: T) -> Option<T> {
        self.bag.insert(value)
    }

    /// Whether a value has been attached yet.
    pub fn is_set(&self) -> bool {
        self.bag.contains::<T>()
    }

    /// Read the value in place — for payloads that are expensive or impossible
    /// to clone.
    pub fn with<R>(&self, f: impl FnOnce(&T) -> R) -> Option<R> {
        self.bag.with(f)
    }

    /// Mutate the value in place. A plain [`get`](Self::get) hands back a copy,
    /// so mutating that changes nothing.
    pub fn with_mut<R>(&self, f: impl FnOnce(&mut T) -> R) -> Option<R> {
        self.bag.with_mut(f)
    }

    /// Remove the value from the bag and return it.
    pub fn take(&self) -> Option<T> {
        self.bag.remove::<T>()
    }
}

impl<T: Clone + Send + Sync + 'static> Extension<T> {
    /// A clone of the attached value, if a stage before this one attached it.
    pub fn get(&self) -> Option<T> {
        self.bag.get::<T>()
    }
}

/// The bag itself is injectable too, for code that reads several payload types. The framework
/// registers it globally, and every injection site of [`Extension<T>`] reads it.
#[async_trait]
impl Provider for Extensions {
    fn token(&self) -> String {
        crate::di::token_of::<Extensions>()
    }

    async fn resolve(&self, ctx: Execution) -> Result<Box<dyn Any + Send>, ResolutionError> {
        let Some(bag) = ctx.extensions() else {
            return Err(ResolutionError::ExecutionRequired {
                token: self.token(),
            });
        };
        Ok(Box::new(bag))
    }

    fn scope(&self) -> ProviderScope {
        ProviderScope::Execution
    }
}

pub(crate) struct ExtensionsFactory;

#[async_trait]
impl ProviderFactory for ExtensionsFactory {
    fn token(&self) -> String {
        crate::di::token_of::<Extensions>()
    }

    async fn build(&self, _deps: FxHashMap<String, Registration>) -> BuildResult<Registration> {
        Ok(Registration::new(
            Arc::new(Extensions::new()) as Arc<dyn Provider>,
            vec![],
        ))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[derive(Clone, PartialEq, Debug)]
    struct User(&'static str);

    struct NotClonable(String);

    fn view<T>() -> (Extensions, Extension<T>) {
        let bag = Extensions::new();
        (bag.clone(), Extension::over(bag))
    }

    #[test]
    fn reads_what_was_written_to_the_underlying_bag() {
        let (bag, ext) = view::<User>();
        bag.insert(User("alice"));

        assert_eq!(ext.get(), Some(User("alice")));
        assert!(ext.is_set());
    }

    /// Two views of one request address one bag, which is what lets a guard's
    /// write reach a service built after it.
    #[test]
    fn writes_through_one_view_are_visible_through_another() {
        let (bag, writer) = view::<User>();
        let reader = Extension::<User>::over(bag);

        writer.set(User("bob"));

        assert_eq!(reader.get(), Some(User("bob")));
    }

    #[test]
    fn is_empty_until_something_writes() {
        let (_bag, ext) = view::<User>();
        assert!(!ext.is_set());
        assert_eq!(ext.get(), None);
    }

    #[test]
    fn with_mut_updates_in_place_where_get_would_hand_back_a_copy() {
        let (_bag, ext) = view::<User>();
        ext.set(User("carol"));

        ext.with_mut(|u| *u = User("dave"));

        assert_eq!(ext.get(), Some(User("dave")));
    }

    #[test]
    fn serves_a_payload_that_is_not_clone() {
        let (_bag, ext) = view::<NotClonable>();
        ext.set(NotClonable("held".into()));

        assert_eq!(ext.with(|v| v.0.clone()), Some("held".to_string()));
        assert_eq!(ext.take().map(|v| v.0), Some("held".to_string()));
        assert!(!ext.is_set());
    }

    #[test]
    fn keys_by_payload_type_so_views_do_not_collide() {
        let bag = Extensions::new();
        let users = Extension::<User>::over(bag.clone());
        let flags = Extension::<u32>::over(bag);

        users.set(User("erin"));
        flags.set(7u32);

        assert_eq!(users.get(), Some(User("erin")));
        assert_eq!(flags.get(), Some(7));
    }
}
