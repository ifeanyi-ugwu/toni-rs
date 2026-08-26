//! Typed DI views over the request's [extension bag](crate::context::Extensions).
//!
//! The bag reaches enhancers and handlers on its own. These bring it to the rest
//! of the injection tree — a service two calls below the controller declares what
//! it needs instead of having it threaded down through every signature.
//!
//! ```rust,ignore
//! #[injectable(scope = "request")]
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
//! Register one line per payload type, alongside any other provider:
//!
//! ```rust,ignore
//! #[module(providers: [Extension::<CurrentUser>, AuditLog])]
//! impl AppModule {}
//! ```
//!
//! # Scope
//!
//! These are request-scoped, so they cannot be injected into singletons — a
//! singleton holding one request's values would serve them to every later
//! request. The container refuses it at startup. Request scope is an HTTP
//! concept in toni: WebSocket gateways and RPC controllers are built once at
//! startup, so their handlers read the bag off the client or the context
//! instead.

use std::any::Any;
use std::marker::PhantomData;
use std::sync::Arc;

use crate::FxHashMap;
use crate::async_trait;
use crate::context::Extensions;
use crate::provider_scope::ProviderScope;
use crate::traits_helpers::{Injectable, Provider, ProviderContext, ProviderFactory};

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
    fn over(bag: Extensions) -> Self {
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

    #[doc(hidden)]
    pub fn __toni_provider_factory() -> ExtensionFactory<T> {
        ExtensionFactory::new()
    }
}

impl<T: Clone + Send + Sync + 'static> Extension<T> {
    /// A clone of the attached value, if a stage before this one attached it.
    pub fn get(&self) -> Option<T> {
        self.bag.get::<T>()
    }
}

#[async_trait]
impl<T: Send + Sync + 'static> Provider for Extension<T> {
    fn get_token(&self) -> String {
        crate::di::token_of::<Extension<T>>()
    }

    async fn execute(
        &self,
        _params: Vec<Box<dyn Any + Send>>,
        ctx: ProviderContext,
    ) -> Box<dyn Any + Send> {
        let Some(bag) = ctx.extensions() else {
            panic!(
                "Extension<{}> is request-scoped and cannot be resolved outside an execution",
                std::any::type_name::<T>()
            );
        };
        Box::new(Extension::<T>::over(bag))
    }

    fn get_scope(&self) -> ProviderScope {
        ProviderScope::Request
    }
}

/// The bag itself is injectable too, for code that reads several payload types
/// and would rather not declare a view for each. It needs no registration —
/// unlike [`Extension<T>`] there is only one of it, so the framework registers
/// it globally.
#[async_trait]
impl Provider for Extensions {
    fn get_token(&self) -> String {
        crate::di::token_of::<Extensions>()
    }

    async fn execute(
        &self,
        _params: Vec<Box<dyn Any + Send>>,
        ctx: ProviderContext,
    ) -> Box<dyn Any + Send> {
        let Some(bag) = ctx.extensions() else {
            panic!("Extensions is request-scoped and cannot be resolved outside an execution");
        };
        Box::new(bag)
    }

    fn get_scope(&self) -> ProviderScope {
        ProviderScope::Request
    }
}

pub struct ExtensionsFactory;

#[async_trait]
impl ProviderFactory for ExtensionsFactory {
    fn get_token(&self) -> String {
        crate::di::token_of::<Extensions>()
    }

    async fn build(&self, _deps: FxHashMap<String, Injectable>) -> Injectable {
        Injectable::new(
            Arc::new(Box::new(Extensions::new()) as Box<dyn Provider>),
            vec![],
        )
    }
}

/// Registers an [`Extension<T>`] with the container.
///
/// Written as `Extension::<T>` in a module's `providers` list, which resolves
/// to this. One entry per payload type: each `T` is its own DI token, so the
/// container cannot conjure them from a single registration.
pub struct ExtensionFactory<T> {
    _marker: PhantomData<fn() -> T>,
}

impl<T> ExtensionFactory<T> {
    pub fn new() -> Self {
        Self {
            _marker: PhantomData,
        }
    }
}

impl<T> Default for ExtensionFactory<T> {
    fn default() -> Self {
        Self::new()
    }
}

#[async_trait]
impl<T: Send + Sync + 'static> ProviderFactory for ExtensionFactory<T> {
    fn get_token(&self) -> String {
        crate::di::token_of::<Extension<T>>()
    }

    async fn build(&self, _deps: FxHashMap<String, Injectable>) -> Injectable {
        let provider = Extension::<T>::over(Extensions::new());
        Injectable::new(Arc::new(Box::new(provider) as Box<dyn Provider>), vec![])
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

    #[test]
    fn the_registration_token_names_the_payload_type() {
        let factory = Extension::<User>::__toni_provider_factory();
        assert!(factory.get_token().contains("Extension"));
        assert!(factory.get_token().contains("User"));
    }
}
