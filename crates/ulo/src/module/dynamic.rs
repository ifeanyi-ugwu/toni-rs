use std::hash::Hash;
use std::sync::Mutex;

use crate::module::def::ModuleDef;
use crate::module::{Module, ModuleIdentity};

type Register = Box<dyn FnOnce(&mut ModuleDef<'_>) + Send>;

/// A module built at runtime by a function, for integration crates. Its identity is the owner
/// type plus the configuration value, so these are still distinct Rust-typed identities.
///
/// ```ignore
/// pub fn redis_module(cfg: RedisConfig) -> DynamicModule {
///     DynamicModule::new::<RedisIntegration>(cfg.clone(), move |m| {
///         m.singleton(move || { let cfg = cfg.clone(); async move { RedisPool::connect(&cfg).await } });
///         m.export::<RedisPool>();
///     })
/// }
/// ```
///
/// The registration closure runs once: identities are deduplicated before `register` is called,
/// so a second import of an equal identity never reaches it.
pub struct DynamicModule {
    identity: ModuleIdentity,
    register: Mutex<Option<Register>>,
}

impl DynamicModule {
    pub fn new<O: 'static, C: Eq + Hash + Clone + Send + Sync + 'static>(
        config: C,
        register: impl FnOnce(&mut ModuleDef<'_>) + Send + 'static,
    ) -> Self {
        DynamicModule {
            identity: ModuleIdentity::of_owner::<O, C>(&config),
            register: Mutex::new(Some(Box::new(register))),
        }
    }

    /// The name diagnostics print in place of the owner type's name.
    pub fn label(self, name: &'static str) -> Self {
        DynamicModule { identity: self.identity.label(name), ..self }
    }
}

impl Module for DynamicModule {
    fn identity(&self) -> ModuleIdentity {
        self.identity.clone()
    }

    fn register(&self, m: &mut ModuleDef<'_>) {
        todo!()
    }
}
