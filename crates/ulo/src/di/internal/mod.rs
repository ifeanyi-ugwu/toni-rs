//! The DI engine: what builds the module graph and answers a resolution.
//!
//! Private to `di`, which is the vocabulary an application and an integration crate write against.
//! Nothing here is nameable from outside the crate, and `ModuleRef` is the one type that leaves —
//! re-exported by `di` as the handle a caller resolves against.

mod container;
pub(crate) use self::container::{Container, ModuleLifecycle};

mod instance_loader;
pub(crate) use self::instance_loader::InstanceLoader;
mod module;
mod multi_collection_provider;

mod dependency_graph;
pub(crate) use self::dependency_graph::{DependencyGraph, find_dependency_cycle};

mod module_ref;
pub use self::module_ref::ModuleRef;

mod module_ref_provider;

pub(crate) mod builtin_module;
pub(crate) mod scanner;

/// One resolver per transport, each turning what a dispatch target declares into what the
/// dispatcher serves. Four things doing one job, in one place.

#[cfg(test)]
mod module_identity_tests;
