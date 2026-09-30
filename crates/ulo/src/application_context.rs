//! The application as a DI root, with nothing served.
//!
//! What a CLI tool, a CRON job, a background worker or a test builds: the module graph resolves and
//! the lifecycle hooks run, and no transport is bound. An application that serves is
//! [`UloApplication`](crate::UloApplication) instead.

use parking_lot::RwLock;
use std::sync::Arc;

use crate::__di::take_shared;
use crate::error::ResolutionError;

use crate::{
    di::internal::{Container, ModuleLifecycle, ModuleRef},
    di::module::ModuleIdentity,
    di::{Execution, Key},
    spi::Provider,
};

/// The module graph, resolvable, with no transport bound.
pub struct UloApplicationContext {
    container: Arc<RwLock<Container>>,
}

impl UloApplicationContext {
    pub(crate) fn new(container: Arc<RwLock<Container>>) -> Self {
        Self { container }
    }

    /// The provider registered under `token` in the one module holding it. Two holders answer
    /// `AmbiguousModule` with both keys rather than one instance.
    ///
    /// The instance is cloned out so the container borrow ends here rather than
    /// spanning the `resolve` that follows.
    fn provider_in_any_module(&self, token: &str) -> Result<Arc<dyn Provider>, ResolutionError> {
        let container = self.container.read();
        let token = token.to_string();

        // Every module holding the token: two would make the answer depend on which module the
        // search reached first (ADR-0057), so they are handed back instead.
        let mut holders: Vec<(String, Arc<dyn Provider>)> = Vec::new();
        for module_token in container.module_tokens() {
            if let Ok(Some(instance)) =
                container.get_provider_instance_by_token(&module_token, &token)
            {
                holders.push((module_token, instance.clone()));
            }
        }
        match holders.len() {
            0 => Err(ResolutionError::ProviderNotFound {
                token,
                module: None,
            }),
            1 => Ok(holders.remove(0).1),
            _ => Err(ResolutionError::AmbiguousModule {
                base: token,
                candidates: holders.into_iter().map(|(module, _)| module).collect(),
            }),
        }
    }

    /// The provider registered under `token` in one named module.
    fn provider_in_module(
        &self,
        module_token: &str,
        token: &str,
    ) -> Result<Arc<dyn Provider>, ResolutionError> {
        let container = self.container.read();

        container
            .get_provider_instance_by_token(&module_token.to_string(), &token.to_string())
            .map_err(|_| ResolutionError::ModuleNotFound {
                id: module_token.to_string(),
            })?
            .cloned()
            .ok_or_else(|| ResolutionError::ProviderNotFound {
                token: token.to_string(),
                module: Some(module_token.to_string()),
            })
    }

    /// Returns `T` from the DI container, searching across all modules, as an `Arc<T>` field reads
    /// it. A type two modules hold answers [`ResolutionError::AmbiguousModule`] with their keys,
    /// each of which [`get_module_by_id`](Self::get_module_by_id) resolves.
    pub async fn get<T: 'static>(&self) -> Result<Arc<T>, ResolutionError> {
        let token = crate::di::token_of::<T>();
        let provider = self.provider_in_any_module(&token)?;

        take_shared(provider.resolve(Execution::None).await?, &token)
    }

    /// Returns `T` from a specific module's scope in the DI container. See [`get`](Self::get).
    pub async fn get_from<T: 'static>(
        &self,
        module_token: &str,
    ) -> Result<Arc<T>, ResolutionError> {
        let token = crate::di::token_of::<T>();
        let provider = self.provider_in_module(module_token, &token)?;

        take_shared(provider.resolve(Execution::None).await?, &token)
    }

    /// The module handle for `M`, found by its identity.
    ///
    /// Matches the module whose identity base is `token_of::<M>()`,
    /// fingerprinted or not. Two fingerprinted instances of one type are
    /// ambiguous: the error lists their full keys, and
    /// [`get_module_by_id`](Self::get_module_by_id) takes one.
    ///
    /// The handle resolves providers in that module's scope, the way an
    /// injected [`ModuleRef`] does from inside it.
    pub async fn get_module<M: 'static>(&self) -> Result<ModuleRef, ResolutionError> {
        let base = crate::di::token_of::<M>();
        let key = self.module_key_for_base(&base)?;
        self.module_ref_for(&key).await
    }

    /// The module handle for the module whose identity key or base is `id`.
    ///
    /// A full key (`base#<16 hex digits>`, as the ambiguity errors print)
    /// matches exactly. A bare base — a `DynamicModule`'s builder-given name,
    /// or a type path — matches whichever module carries it, and is ambiguous
    /// when two configs of one maker share it.
    pub async fn get_module_by_id(&self, id: &str) -> Result<ModuleRef, ResolutionError> {
        let exact = self
            .container
            .read()
            .module_tokens()
            .into_iter()
            .find(|key| key == id);
        let key = match exact {
            Some(key) => key,
            None => self.module_key_for_base(id)?,
        };
        self.module_ref_for(&key).await
    }

    /// The key of the one module whose identity base is `base`.
    fn module_key_for_base(&self, base: &str) -> Result<String, ResolutionError> {
        let container = self.container.read();
        let keys = container.module_tokens();

        let matches: Vec<&String> = keys
            .iter()
            .filter(|key| ModuleIdentity::parse(key).base() == base)
            .collect();
        match matches.as_slice() {
            [key] => Ok((*key).clone()),
            [] => Err(ResolutionError::ModuleNotFound {
                id: base.to_string(),
            }),
            many => Err(ResolutionError::AmbiguousModule {
                base: base.to_string(),
                candidates: many.iter().map(|key| (*key).clone()).collect(),
            }),
        }
    }

    async fn module_ref_for(&self, module_id: &str) -> Result<ModuleRef, ResolutionError> {
        let token = crate::di::token_of::<ModuleRef>();
        let provider = self.provider_in_module(module_id, &token)?;
        crate::__di::take_value(provider.resolve(Execution::None).await?, &token)
    }

    /// Returns the value under the marker `K`, searching across all modules. A slot holding a
    /// trait object is not reached here: it hands out `Arc<K::Value>`, which a field reads through
    /// `#[inject(K)]`.
    pub async fn get_key<K>(&self) -> Result<Arc<K::Value>, ResolutionError>
    where
        K: Key,
        K::Value: Sized,
    {
        self.get_under::<K::Value>(crate::di::token_of::<K>()).await
    }

    /// Returns the value under the marker `K` from a specific module's scope. See
    /// [`get_key`](Self::get_key).
    pub async fn get_from_key<K>(
        &self,
        module_token: &str,
    ) -> Result<Arc<K::Value>, ResolutionError>
    where
        K: Key,
        K::Value: Sized,
    {
        self.get_from_under::<K::Value>(module_token, crate::di::token_of::<K>())
            .await
    }

    /// The value registered under `token`, searching across all modules.
    async fn get_under<T: 'static>(&self, token: String) -> Result<Arc<T>, ResolutionError> {
        let provider = self.provider_in_any_module(&token)?;

        take_shared(provider.resolve(Execution::None).await?, &token)
    }

    /// The value registered under `token` in one module.
    async fn get_from_under<T: 'static>(
        &self,
        module_token: &str,
        token: String,
    ) -> Result<Arc<T>, ResolutionError> {
        let provider = self.provider_in_module(module_token, &token)?;

        take_shared(provider.resolve(Execution::None).await?, &token)
    }

    /// Resolves a provider `T` in an execution.
    ///
    /// What [`get`](Self::get) cannot reach: an execution-scoped provider is built into
    /// the execution's cache, so it needs one. Everything resolved in the same
    /// execution shares that cache — an execution-scoped type is built once and handed
    /// to each of them, the way a handler and its guards see one instance.
    ///
    /// The execution can be any transport's context, or
    /// [`Execution::standalone`] where the work arrived over nothing: a CLI
    /// command, a job, a test.
    ///
    /// # Example
    /// ```rust,ignore
    /// let execution = Execution::standalone();
    /// let repo = ctx.resolve::<Repo>(&execution).await?;
    /// let audit = ctx.resolve::<AuditLog>(&execution).await?;
    ///
    /// // An HTTP execution, when the work is genuinely a request:
    /// let execution: Execution = HttpContext::from_parts(parts).into();
    /// let service = ctx.resolve::<RequestService>(&execution).await?;
    /// ```
    pub async fn resolve<T: 'static>(
        &self,
        execution: &Execution,
    ) -> Result<Arc<T>, ResolutionError> {
        let token = crate::di::token_of::<T>();
        let provider = self.provider_in_any_module(&token)?;

        take_shared(provider.resolve(execution.clone()).await?, &token)
    }

    /// Resolves the value under the marker `K` in an execution. See [`resolve`](Self::resolve)
    /// and [`get_key`](Self::get_key).
    pub async fn resolve_key<K>(
        &self,
        execution: &Execution,
    ) -> Result<Arc<K::Value>, ResolutionError>
    where
        K: Key,
        K::Value: Sized,
    {
        self.resolve_under::<K::Value>(crate::di::token_of::<K>(), execution)
            .await
    }

    /// The value registered under `token`, resolved in an execution.
    async fn resolve_under<T: 'static>(
        &self,
        token: String,
        execution: &Execution,
    ) -> Result<Arc<T>, ResolutionError> {
        let provider = self.provider_in_any_module(&token)?;

        take_shared(provider.resolve(execution.clone()).await?, &token)
    }

    pub async fn close(&mut self) {
        self.call_module_destroy_hooks().await;
        self.call_before_shutdown_hooks(None).await;
        self.call_shutdown_hooks(None).await;
    }

    /// Every module's hook-carrying handles in the reverse of construction order, providers and
    /// controllers reversed within each, taken in one pass under the container lock: shutdown
    /// tears down a provider before what it injects (ADR-0057). A module's own hooks keep their
    /// phase ahead of every provider's, as at startup.
    ///
    /// The shutdown hooks below are awaited, so the handles are detached from the container
    /// first rather than held across each await. See [`Container::module_lifecycle`].
    fn module_lifecycles(&self) -> Vec<ModuleLifecycle> {
        let container = self.container.read();
        container
            .modules_in_construction_order()
            .iter()
            .rev()
            .filter_map(|token| container.module_lifecycle(token))
            .map(|mut lifecycle| {
                lifecycle.providers.reverse();
                lifecycle.controllers.reverse();
                lifecycle
            })
            .collect()
    }

    pub(crate) async fn call_before_shutdown_hooks(&self, signal: Option<String>) {
        let lifecycles = self.module_lifecycles();

        for lifecycle in &lifecycles {
            lifecycle
                .metadata
                .before_application_shutdown(signal.clone())
                .await;
        }

        // Controllers were built after every provider, so they stop before any.
        for lifecycle in &lifecycles {
            for controller in &lifecycle.controllers {
                controller.before_application_shutdown(signal.clone()).await;
            }
        }
        for lifecycle in &lifecycles {
            for provider in &lifecycle.providers {
                provider.before_application_shutdown(signal.clone()).await;
            }
        }
    }

    pub(crate) async fn call_module_destroy_hooks(&self) {
        let lifecycles = self.module_lifecycles();

        for lifecycle in &lifecycles {
            lifecycle.metadata.on_module_destroy().await;
        }

        // Controllers were built after every provider, so they stop before any.
        for lifecycle in &lifecycles {
            for controller in &lifecycle.controllers {
                controller.on_module_destroy().await;
            }
        }
        for lifecycle in &lifecycles {
            for provider in &lifecycle.providers {
                provider.on_module_destroy().await;
            }
        }
    }

    pub(crate) async fn call_shutdown_hooks(&self, signal: Option<String>) {
        let lifecycles = self.module_lifecycles();

        for lifecycle in &lifecycles {
            lifecycle
                .metadata
                .on_application_shutdown(signal.clone())
                .await;
        }

        // Controllers were built after every provider, so they stop before any.
        for lifecycle in &lifecycles {
            for controller in &lifecycle.controllers {
                controller.on_application_shutdown(signal.clone()).await;
            }
        }
        for lifecycle in &lifecycles {
            for provider in &lifecycle.providers {
                provider.on_application_shutdown(signal.clone()).await;
            }
        }
    }
}
