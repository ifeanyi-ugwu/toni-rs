//! The dependency-injection system: what a module declares, what resolves it, and the execution a
//! resolution happens in.
//!
//! A module is a DI scope. [`ModuleMetadata`] is what one declares, [`ModuleRef`] is the handle
//! that resolves against it, and [`Execution`] is the one a scoped provider is built for — which
//! transport is running, or none. What an integration crate implements to *be* a provider is
//! `spi`.

mod binding;
mod collection;
mod declares;
mod execution;
mod execution_cache;
mod extension;
pub(crate) mod internal;
mod key;
pub(crate) mod module;
mod provide;
mod scope;
mod token;

pub use binding::{Binding, FactoryBinding, PerExecution, Recast, ScopedBinding};
pub use collection::Contribution;
pub(crate) use collection::{GlobalCollection, global_collection};
pub use declares::{DeclaresController, DeclaresProvider};
pub use execution::Execution;
pub use execution_cache::ExecutionCache;
pub use extension::{Extension, ExtensionFactory};
pub use internal::ModuleRef;
pub use key::Key;
pub use module::{
    CheckedModule, DynamicModule, MiddlewareConsumer, ModuleIdentity, ModuleMetadata,
};
pub use provide::{
    AliasDeclaration, Declaration, Declared, FactoryDeclaration, IntoFactory, MakeInExecution,
    Provide, ScopedFactoryDeclaration, Under, ValueDeclaration,
};
pub use scope::ProviderScope;
pub use token::{APP_GUARD, APP_INTERCEPTOR, APP_MIDDLEWARE, IntoToken, Token, token_of};
/// Derives [`Key`](trait@Key) for a type of the crate, naming its own slot.
pub use ulo_macros::Key;

pub use crate::error::{InitResult, ResolutionError};
