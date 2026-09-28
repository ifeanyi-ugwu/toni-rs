//! The application as a DI root, with nothing served.
//!
//! What a CLI tool, a CRON job, a background worker or a test builds: the module graph resolves and
//! the lifecycle hooks run, and no transport is bound. An application that serves is
//! [`UloApplication`](crate::UloApplication) instead.

use parking_lot::RwLock;
use std::{any::Any, sync::Arc};

use crate::error::ResolutionError;

use crate::{
    di::internal::{Container, IntoToken, ModuleLifecycle, ModuleRef},
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

    /// The provider registered under `token`, from whichever module holds it.
    ///
    /// The instance is cloned out so the container borrow ends here rather than
    /// spanning the `resolve` that follows.
    fn provider_in_any_module(
        &self,
        token: &str,
    ) -> Result<Arc<Box<dyn Provider>>, ResolutionError> {
        let container = self.container.read();
        let token = token.to_string();

        container
            .module_tokens()
            .iter()
            .find_map(|module_token| {
                container
                    .get_provider_instance_by_token(module_token, &token)
                    .ok()
                    .flatten()
                    .cloned()
            })
            .ok_or(ResolutionError::ProviderNotFound {
                token,
                module: None,
            })
    }

    /// The provider registered under `token` in one named module.
    fn provider_in_module(
        &self,
        module_token: &str,
        token: &str,
    ) -> Result<Arc<Box<dyn Provider>>, ResolutionError> {
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

    /// Returns an instance of `T` from the DI container, searching across all modules
    pub async fn get<T: 'static>(&self) -> Result<T, ResolutionError> {
        let token = crate::di::token_of::<T>();
        let provider = self.provider_in_any_module(&token)?;
        Execution::None.ensure_can_build(provider.scope(), &token)?;

        downcast(provider.resolve(Execution::None).await, &token)
    }

    /// Returns an instance of `T` from a specific module's scope in the DI container
    pub async fn get_from<T: 'static>(&self, module_token: &str) -> Result<T, ResolutionError> {
        let token = crate::di::token_of::<T>();
        let provider = self.provider_in_module(module_token, &token)?;
        Execution::None.ensure_can_build(provider.scope(), &token)?;

        downcast(provider.resolve(Execution::None).await, &token)
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
        downcast(provider.resolve(Execution::None).await, &token)
    }

    /// Returns the value under the marker `K`, searching across all modules. A slot holding a
    /// trait object is not reached here: it hands out `Arc<K::Value>`, which a field reads through
    /// `#[inject(K)]`.
    pub async fn get_key<K>(&self) -> Result<K::Value, ResolutionError>
    where
        K: Key,
        K::Value: Sized,
    {
        self.get_by_token::<K::Value>(crate::di::token_of::<K>())
            .await
    }

    /// Returns the value under the marker `K` from a specific module's scope. See
    /// [`get_key`](Self::get_key).
    pub async fn get_from_key<K>(&self, module_token: &str) -> Result<K::Value, ResolutionError>
    where
        K: Key,
        K::Value: Sized,
    {
        self.get_from_by_token::<K::Value>(module_token, crate::di::token_of::<K>())
            .await
    }

    /// Returns an instance from the DI container by token rather than type; use when providers are registered with a custom token
    pub async fn get_by_token<T: 'static>(
        &self,
        token: impl IntoToken<T>,
    ) -> Result<T, ResolutionError> {
        let token = token.into_token();
        let provider = self.provider_in_any_module(&token)?;
        Execution::None.ensure_can_build(provider.scope(), &token)?;

        downcast(provider.resolve(Execution::None).await, &token)
    }

    /// Returns an instance by token from a specific module's scope in the DI container
    pub async fn get_from_by_token<T: 'static>(
        &self,
        module_token: &str,
        token: impl IntoToken<T>,
    ) -> Result<T, ResolutionError> {
        let token = token.into_token();
        let provider = self.provider_in_module(module_token, &token)?;
        Execution::None.ensure_can_build(provider.scope(), &token)?;

        downcast(provider.resolve(Execution::None).await, &token)
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
    pub async fn resolve<T: 'static>(&self, execution: &Execution) -> Result<T, ResolutionError> {
        let token = crate::di::token_of::<T>();
        let provider = self.provider_in_any_module(&token)?;
        execution.ensure_can_build(provider.scope(), &token)?;

        downcast(provider.resolve(execution.clone()).await, &token)
    }

    /// Resolves the value under the marker `K` in an execution. See [`resolve`](Self::resolve)
    /// and [`get_key`](Self::get_key).
    pub async fn resolve_key<K>(&self, execution: &Execution) -> Result<K::Value, ResolutionError>
    where
        K: Key,
        K::Value: Sized,
    {
        self.resolve_by_token::<K::Value>(crate::di::token_of::<K>(), execution)
            .await
    }

    /// Resolves a provider by token in an execution.
    pub async fn resolve_by_token<T: 'static>(
        &self,
        token: impl IntoToken<T>,
        execution: &Execution,
    ) -> Result<T, ResolutionError> {
        let token = token.into_token();
        let provider = self.provider_in_any_module(&token)?;
        execution.ensure_can_build(provider.scope(), &token)?;

        downcast(provider.resolve(execution.clone()).await, &token)
    }

    pub async fn close(&mut self) {
        self.call_module_destroy_hooks().await;
        self.call_before_shutdown_hooks(None).await;
        self.call_shutdown_hooks(None).await;
    }

    /// Every module's hook-carrying handles, taken in one pass under the container lock.
    ///
    /// The shutdown hooks below are awaited, so the handles are detached from the container
    /// first rather than held across each await. See [`Container::module_lifecycle`].
    fn module_lifecycles(&self) -> Vec<ModuleLifecycle> {
        let container = self.container.read();
        container
            .module_tokens()
            .iter()
            .filter_map(|token| container.module_lifecycle(token))
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

        for lifecycle in lifecycles {
            for provider in lifecycle.providers {
                provider.before_application_shutdown(signal.clone()).await;
            }
            for controller in lifecycle.controllers {
                controller.before_application_shutdown(signal.clone()).await;
            }
        }
    }

    pub(crate) async fn call_module_destroy_hooks(&self) {
        let lifecycles = self.module_lifecycles();

        for lifecycle in &lifecycles {
            lifecycle.metadata.on_module_destroy().await;
        }

        for lifecycle in lifecycles {
            for provider in lifecycle.providers {
                provider.on_module_destroy().await;
            }
            for controller in lifecycle.controllers {
                controller.on_module_destroy().await;
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

        for lifecycle in lifecycles {
            for provider in lifecycle.providers {
                provider.on_application_shutdown(signal.clone()).await;
            }
            for controller in lifecycle.controllers {
                controller.on_application_shutdown(signal.clone()).await;
            }
        }
    }
}

fn downcast<T: 'static>(instance: Box<dyn Any + Send>, token: &str) -> Result<T, ResolutionError> {
    instance
        .downcast::<T>()
        .map(|boxed| *boxed)
        .map_err(|_| ResolutionError::TypeMismatch {
            token: token.to_string(),
        })
}
