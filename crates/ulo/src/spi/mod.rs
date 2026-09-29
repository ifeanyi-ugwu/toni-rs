//! What an integration crate implements to plug into the framework.
//!
//! A provider, a controller and a route are things the macros write for an app author and a
//! hand-written integration writes for itself — `ulo-config`, `ulo-db-prisma` and the GraphQL
//! crates all implement these directly. The DI vocabulary an app *uses* is `di`; this is what it
//! takes to *be* one of the things `di` hands out.

mod bind_target;
pub(crate) mod provider;

pub use self::bind_target::BindTarget;
pub use self::provider::{Injectable, Provider, ProviderFactory, ProviderRole};
pub use crate::error::{AdapterResult, BuildResult};
