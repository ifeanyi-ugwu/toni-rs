//! Standalone application context for non-HTTP scenarios
//!
//! Use this for CLI tools, CRON jobs, background workers, and other
//! scenarios where you need dependency injection without an HTTP server.

use std::{any::Any, cell::RefCell, rc::Rc, sync::Arc};

use anyhow::Result;

use crate::{
    context::HttpContext,
    injector::{IntoToken, ToniContainer},
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
        let token = std::any::type_name::<T>().to_string();
        let provider = self.provider_in_any_module(&token)?;
        refuse_request_scope(&provider, &token)?;

        downcast(
            provider.execute(vec![], ProviderContext::None).await,
            &token,
        )
    }

    /// Returns an instance of `T` from a specific module's scope in the DI container
    pub async fn get_from<T: 'static>(&self, module_token: &str) -> Result<T> {
        let token = std::any::type_name::<T>().to_string();
        let provider = self.provider_in_module(module_token, &token)?;
        refuse_request_scope(&provider, &token)?;

        downcast(
            provider.execute(vec![], ProviderContext::None).await,
            &token,
        )
    }

    /// Returns an instance from the DI container by token rather than type; use when providers are registered with a custom token
    pub async fn get_by_token<T: 'static>(&self, token: impl IntoToken) -> Result<T> {
        let token = token.into_token();
        let provider = self.provider_in_any_module(&token)?;
        refuse_request_scope(&provider, &token)?;

        downcast(
            provider.execute(vec![], ProviderContext::None).await,
            &token,
        )
    }

    /// Returns an instance by token from a specific module's scope in the DI container
    pub async fn get_from_by_token<T: 'static>(
        &self,
        module_token: &str,
        token: impl IntoToken,
    ) -> Result<T> {
        let token = token.into_token();
        let provider = self.provider_in_module(module_token, &token)?;
        refuse_request_scope(&provider, &token)?;

        downcast(
            provider.execute(vec![], ProviderContext::None).await,
            &token,
        )
    }

    /// Resolves a request-scoped or transient provider `T` using a synthetic request context.
    ///
    /// Use this when you need a request-scoped provider outside of an HTTP handler — for
    /// testing, CLI tools, or health checks that need to exercise the full provider tree.
    ///
    /// Each call is its own execution, so two calls build two instances of a
    /// request-scoped type. To place several resolutions in one execution the
    /// way a single request would, build a context and use
    /// [`resolve_in`](Self::resolve_in).
    ///
    /// # Example
    /// ```rust,ignore
    /// let parts = http::Request::builder().body(()).unwrap().into_parts().0;
    /// let service = ctx.resolve::<RequestService>(&parts).await?;
    ///
    /// // Two resolutions, one execution:
    /// let execution = HttpContext::from_parts(parts);
    /// let a = ctx.resolve_in::<ServiceA>(&execution).await?;
    /// let b = ctx.resolve_in::<ServiceB>(&execution).await?;
    /// ```
    pub async fn resolve<T: 'static>(&self, parts: &crate::http_helpers::RequestPart) -> Result<T> {
        let token = std::any::type_name::<T>().to_string();
        let provider = self.provider_in_any_module(&token)?;
        let execution = ProviderContext::Http(HttpContext::from_parts(parts.clone()));

        downcast(provider.execute(vec![], execution).await, &token)
    }

    /// Resolves a request-scoped or transient provider by token using a synthetic request context.
    pub async fn resolve_by_token<T: 'static>(
        &self,
        token: impl IntoToken,
        parts: &crate::http_helpers::RequestPart,
    ) -> Result<T> {
        let token = token.into_token();
        let provider = self.provider_in_any_module(&token)?;
        let execution = ProviderContext::Http(HttpContext::from_parts(parts.clone()));

        downcast(provider.execute(vec![], execution).await, &token)
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

/// A request-scoped provider cannot be built here: there is no execution to build it into.
fn refuse_request_scope(provider: &Arc<Box<dyn Provider>>, token: &str) -> Result<()> {
    if provider.get_scope() == crate::ProviderScope::Request {
        return Err(anyhow::anyhow!(
            "Provider '{}' is request-scoped and cannot be retrieved from ToniApplicationContext. \
             Request-scoped providers are only available within an active HTTP request.",
            token
        ));
    }

    Ok(())
}

fn downcast<T: 'static>(instance: Box<dyn Any + Send>, token: &str) -> Result<T> {
    instance
        .downcast::<T>()
        .map(|boxed| *boxed)
        .map_err(|_| anyhow::anyhow!("Failed to downcast provider '{}' to requested type", token))
}
