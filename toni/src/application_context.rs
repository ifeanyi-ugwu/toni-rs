//! Standalone application context for non-HTTP scenarios
//!
//! Use this for CLI tools, CRON jobs, background workers, and other
//! scenarios where you need dependency injection without an HTTP server.

use std::{any::Any, cell::RefCell, rc::Rc, sync::Arc};

use anyhow::Result;

use crate::{
    injector::{IntoToken, ModuleRef, ToniContainer},
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
    /// Matches the module whose identity is `token_of::<M>()` exactly, or — for
    /// a module type that folds a config fingerprint into its identity, like the
    /// GraphQL modules — the single module whose identity extends that token
    /// with a fingerprint. Two fingerprinted instances of the same type are
    /// ambiguous: the error names both, and
    /// [`get_module_by_name`](Self::get_module_by_name) reaches one when their
    /// display names differ.
    ///
    /// The handle resolves providers in that module's scope, the way an
    /// injected [`ModuleRef`] does from inside it.
    pub async fn get_module<M: 'static>(&self) -> Result<ModuleRef> {
        let wanted = crate::di::token_of::<M>();
        let id = self.module_id_for(&wanted)?;
        self.module_ref_for(&id).await
    }

    /// The module handle for the module whose display name is `name`.
    ///
    /// The reach for modules whose identity is not a type: a `DynamicModule` is
    /// keyed by its base name plus a config fingerprint, so
    /// [`get_module`](Self::get_module) cannot address it. Ambiguous when the
    /// same maker was imported twice with different config — the two keep
    /// distinct identities under one name, and the error names both.
    pub async fn get_module_by_name(&self, name: &str) -> Result<ModuleRef> {
        let matches: Vec<String> = {
            let container = self.container.borrow();
            container
                .get_modules_token()
                .into_iter()
                .filter(|id| {
                    container
                        .get_module_by_token(id)
                        .map(|m| m.get_metadata().get_name() == name)
                        .unwrap_or(false)
                })
                .collect()
        };

        match matches.as_slice() {
            [id] => self.module_ref_for(id).await,
            [] => Err(anyhow::anyhow!("No module is named '{name}'")),
            many => Err(anyhow::anyhow!(
                "Module name '{name}' is ambiguous: {many:?} share it. \
                 The same module imported with different config keeps distinct identities."
            )),
        }
    }

    /// The identity of the one module matching `wanted`: exact, or the single
    /// fingerprinted extension (`wanted#…`).
    fn module_id_for(&self, wanted: &str) -> Result<String> {
        let container = self.container.borrow();
        let ids = container.get_modules_token();

        if ids.iter().any(|id| id == wanted) {
            return Ok(wanted.to_string());
        }

        let prefix = format!("{wanted}#");
        let fingerprinted: Vec<&String> = ids.iter().filter(|id| id.starts_with(&prefix)).collect();
        match fingerprinted.as_slice() {
            [id] => Ok((*id).clone()),
            [] => Err(anyhow::anyhow!(
                "No module has identity '{wanted}'. The module is not imported, \
                 or its identity is not its type — a DynamicModule is reached with \
                 `get_module_by_name`."
            )),
            many => Err(anyhow::anyhow!(
                "Module type '{wanted}' is ambiguous: {many:?} share it. \
                 `get_module_by_name` reaches one when their names differ."
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
