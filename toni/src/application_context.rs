//! Standalone application context for non-HTTP scenarios
//!
//! Use this for CLI tools, CRON jobs, background workers, and other
//! scenarios where you need dependency injection without an HTTP server.

use std::{any::Any, cell::RefCell, rc::Rc, sync::Arc};

use anyhow::Result;

use crate::{
    injector::{IntoToken, ModuleRef, ToniContainer},
    module_helpers::ModuleIdentity,
    traits_helpers::{Provider, ProviderContext},
};

/// Full DI container without an HTTP server
pub struct ToniApplicationContext {
    container: Rc<RefCell<ToniContainer>>,
}

impl ToniApplicationContext {
    pub(crate) fn new(container: Rc<RefCell<ToniContainer>>) -> Self {
        Self { container }
    }

    /// The provider registered under `token`, from whichever module holds it.
    ///
    /// The instance is cloned out so the container borrow ends here rather than
    /// spanning the `execute` that follows.
    fn provider_in_any_module(&self, token: &str) -> Result<Arc<Box<dyn Provider>>> {
        let container = self.container.borrow();
        let token = token.to_string();

        container
            .get_modules_token()
            .iter()
            .find_map(|module_token| {
                container
                    .get_provider_instance_by_token(module_token, &token)
                    .ok()
                    .flatten()
                    .cloned()
            })
            .ok_or_else(|| anyhow::anyhow!("Provider '{}' not found in any module", token))
    }

    /// The provider registered under `token` in one named module.
    fn provider_in_module(
        &self,
        module_token: &str,
        token: &str,
    ) -> Result<Arc<Box<dyn Provider>>> {
        let container = self.container.borrow();

        container
            .get_provider_instance_by_token(&module_token.to_string(), &token.to_string())?
            .cloned()
            .ok_or_else(|| {
                anyhow::anyhow!(
                    "Provider '{}' not found in module '{}'",
                    token,
                    module_token
                )
            })
    }

    /// Returns an instance of `T` from the DI container, searching across all modules
    pub async fn get<T: 'static>(&self) -> Result<T> {
        let token = crate::di::token_of::<T>();
        let provider = self.provider_in_any_module(&token)?;
        ProviderContext::None.ensure_can_build(provider.get_scope(), &token)?;

        downcast(
            provider.execute(vec![], ProviderContext::None).await,
            &token,
        )
    }

    /// Returns an instance of `T` from a specific module's scope in the DI container
    pub async fn get_from<T: 'static>(&self, module_token: &str) -> Result<T> {
        let token = crate::di::token_of::<T>();
        let provider = self.provider_in_module(module_token, &token)?;
        ProviderContext::None.ensure_can_build(provider.get_scope(), &token)?;

        downcast(
            provider.execute(vec![], ProviderContext::None).await,
            &token,
        )
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
    pub async fn get_module<M: 'static>(&self) -> Result<ModuleRef> {
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
    pub async fn get_module_by_id(&self, id: &str) -> Result<ModuleRef> {
        let exact = self
            .container
            .borrow()
            .get_modules_token()
            .into_iter()
            .find(|key| key == id);
        let key = match exact {
            Some(key) => key,
            None => self.module_key_for_base(id)?,
        };
        self.module_ref_for(&key).await
    }

    /// The key of the one module whose identity base is `base`.
    fn module_key_for_base(&self, base: &str) -> Result<String> {
        let container = self.container.borrow();
        let keys = container.get_modules_token();

        let matches: Vec<&String> = keys
            .iter()
            .filter(|key| ModuleIdentity::parse(key).base() == base)
            .collect();
        match matches.as_slice() {
            [key] => Ok((*key).clone()),
            [] => Err(anyhow::anyhow!(
                "No module has identity '{base}'. The module is not imported, \
                 or its identity base differs — a DynamicModule's base is the \
                 name its builder was given."
            )),
            many => Err(anyhow::anyhow!(
                "Module identity '{base}' is ambiguous: {many:?} share the \
                 base. Pass one full key to `get_module_by_id`."
            )),
        }
    }

    async fn module_ref_for(&self, module_id: &str) -> Result<ModuleRef> {
        let token = crate::di::token_of::<ModuleRef>();
        let provider = self.provider_in_module(module_id, &token)?;
        downcast(
            provider.execute(vec![], ProviderContext::None).await,
            &token,
        )
    }

    /// Returns an instance from the DI container by token rather than type; use when providers are registered with a custom token
    pub async fn get_by_token<T: 'static>(&self, token: impl IntoToken<T>) -> Result<T> {
        let token = token.into_token();
        let provider = self.provider_in_any_module(&token)?;
        ProviderContext::None.ensure_can_build(provider.get_scope(), &token)?;

        downcast(
            provider.execute(vec![], ProviderContext::None).await,
            &token,
        )
    }

    /// Returns an instance by token from a specific module's scope in the DI container
    pub async fn get_from_by_token<T: 'static>(
        &self,
        module_token: &str,
        token: impl IntoToken<T>,
    ) -> Result<T> {
        let token = token.into_token();
        let provider = self.provider_in_module(module_token, &token)?;
        ProviderContext::None.ensure_can_build(provider.get_scope(), &token)?;

        downcast(
            provider.execute(vec![], ProviderContext::None).await,
            &token,
        )
    }

    /// Resolves a provider `T` in an execution.
    ///
    /// What [`get`](Self::get) cannot reach: a request-scoped provider is built into
    /// the execution's cache, so it needs one. Everything resolved in the same
    /// execution shares that cache — a request-scoped type is built once and handed
    /// to each of them, the way a handler and its guards see one instance.
    ///
    /// The execution can be any transport's context, or
    /// [`ProviderContext::standalone`] where the work arrived over nothing: a CLI
    /// command, a job, a test.
    ///
    /// # Example
    /// ```rust,ignore
    /// let execution = ProviderContext::standalone();
    /// let repo = ctx.resolve::<Repo>(&execution).await?;
    /// let audit = ctx.resolve::<AuditLog>(&execution).await?;
    ///
    /// // An HTTP execution, when the work is genuinely a request:
    /// let execution: ProviderContext = HttpContext::from_parts(parts).into();
    /// let service = ctx.resolve::<RequestService>(&execution).await?;
    /// ```
    pub async fn resolve<T: 'static>(&self, execution: &ProviderContext) -> Result<T> {
        let token = crate::di::token_of::<T>();
        let provider = self.provider_in_any_module(&token)?;
        execution.ensure_can_build(provider.get_scope(), &token)?;

        downcast(provider.execute(vec![], execution.clone()).await, &token)
    }

    /// Resolves a provider by token in an execution.
    pub async fn resolve_by_token<T: 'static>(
        &self,
        token: impl IntoToken<T>,
        execution: &ProviderContext,
    ) -> Result<T> {
        let token = token.into_token();
        let provider = self.provider_in_any_module(&token)?;
        execution.ensure_can_build(provider.get_scope(), &token)?;

        downcast(provider.execute(vec![], execution.clone()).await, &token)
    }

    pub async fn close(&mut self) {
        self.call_module_destroy_hooks().await;
        self.call_before_shutdown_hooks(None).await;
        self.call_shutdown_hooks(None).await;
    }

    pub(crate) async fn call_before_shutdown_hooks(&self, signal: Option<String>) {
        let container = self.container.borrow();
        let modules = container.get_modules_token();

        for module_token in modules.clone() {
            if let Some(module_ref) = container.get_module_by_token(&module_token) {
                module_ref
                    .get_metadata()
                    .before_application_shutdown(signal.clone())
                    .await;
            }
        }

        for module_token in modules {
            if let Ok(providers) = container.get_lifecycle_instances(&module_token) {
                for provider in providers {
                    if provider.get_scope() == crate::ProviderScope::Request {
                        continue;
                    }
                    provider.before_application_shutdown(signal.clone()).await;
                }
            }
            if let Some(module) = container.get_module_by_token(&module_token) {
                for controller in module.get_controller_objects() {
                    controller.before_application_shutdown(signal.clone()).await;
                }
            }
        }
    }

    pub(crate) async fn call_module_destroy_hooks(&self) {
        let container = self.container.borrow();
        let modules = container.get_modules_token();

        for module_token in modules.clone() {
            if let Some(module_ref) = container.get_module_by_token(&module_token) {
                module_ref.get_metadata().on_module_destroy().await;
            }
        }

        for module_token in modules {
            if let Ok(providers) = container.get_lifecycle_instances(&module_token) {
                for provider in providers {
                    if provider.get_scope() == crate::ProviderScope::Request {
                        continue;
                    }
                    provider.on_module_destroy().await;
                }
            }
            if let Some(module) = container.get_module_by_token(&module_token) {
                for controller in module.get_controller_objects() {
                    controller.on_module_destroy().await;
                }
            }
        }
    }

    pub(crate) async fn call_shutdown_hooks(&self, signal: Option<String>) {
        let container = self.container.borrow();
        let modules = container.get_modules_token();

        for module_token in modules.clone() {
            if let Some(module_ref) = container.get_module_by_token(&module_token) {
                module_ref
                    .get_metadata()
                    .on_application_shutdown(signal.clone())
                    .await;
            }
        }

        for module_token in modules {
            if let Ok(providers) = container.get_lifecycle_instances(&module_token) {
                for provider in providers {
                    if provider.get_scope() == crate::ProviderScope::Request {
                        continue;
                    }
                    provider.on_application_shutdown(signal.clone()).await;
                }
            }
            if let Some(module) = container.get_module_by_token(&module_token) {
                for controller in module.get_controller_objects() {
                    controller.on_application_shutdown(signal.clone()).await;
                }
            }
        }
    }
}

fn downcast<T: 'static>(instance: Box<dyn Any + Send>, token: &str) -> Result<T> {
    instance
        .downcast::<T>()
        .map(|boxed| *boxed)
        .map_err(|_| anyhow::anyhow!("Failed to downcast provider '{}' to requested type", token))
}
