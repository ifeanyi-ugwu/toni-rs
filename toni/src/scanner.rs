use std::{cell::RefCell, rc::Rc};

use crate::error::StartupError;
use anyhow::{Result, anyhow};

use crate::{
    injector::ToniContainer,
    traits_helpers::{MiddlewareConsumer, ModuleMetadata},
};

pub struct ToniDependenciesScanner {
    container: Rc<RefCell<ToniContainer>>,
}

impl ToniDependenciesScanner {
    pub fn new(container: Rc<RefCell<ToniContainer>>) -> Self {
        Self { container }
    }
    pub fn scan(&mut self, module: Box<dyn ModuleMetadata>) -> Result<()> {
        self.scan_for_modules_with_imports(module)?;
        self.scan_modules_for_dependencies()?;
        Ok(())
    }
    fn scan_for_modules_with_imports(&mut self, module: Box<dyn ModuleMetadata>) -> Result<()> {
        let mut ctx_registry: Vec<String> = vec![];

        let mut stack: Vec<Box<dyn ModuleMetadata>> = vec![module];

        while let Some(default_module) = stack.pop() {
            let module_id = default_module.get_id();
            tracing::debug!(module = %default_module.get_name(), "scanning module");
            // Dedup on the full identity (get_id), not the display name: two distinct module
            // types can share a short name, and two dynamic modules with different config share
            // a base name but not an identity. Keying on get_id keeps both cases correct.
            ctx_registry.push(module_id.clone());

            let modules_imported = default_module.imports().unwrap_or_default();

            let mut modules_imported_tokens = vec![];

            for module_imported in modules_imported {
                let imported_id = module_imported.get_id();
                modules_imported_tokens.push(imported_id.clone());

                if ctx_registry.iter().any(|seen| seen == &imported_id) {
                    continue;
                }

                stack.push(module_imported);
            }
            self.insert_module(default_module)?;
            self.insert_imports(module_id, modules_imported_tokens)?;
        }

        tracing::debug!(total = ctx_registry.len(), "module graph scan complete");
        Ok(())
    }

    pub fn scan_modules_for_dependencies(&mut self) -> Result<()> {
        let modules_token = self.container.borrow().get_modules_token();
        for module_token in modules_token {
            self.insert_providers(module_token.clone())?;
            self.insert_controllers(module_token.clone())?;
            self.insert_exports(module_token.clone())?;
        }

        Ok(())
    }

    fn insert_module(&mut self, module: Box<dyn ModuleMetadata>) -> Result<()> {
        let mut container = self.container.borrow_mut();
        container.add_module(module)
    }

    pub fn insert_imports(&mut self, module_token: String, imports: Vec<String>) -> Result<()> {
        let mut container = self.container.borrow_mut();

        for import in imports {
            container.add_import(&module_token, import)?;
        }

        Ok(())
    }

    pub fn insert_controllers(&mut self, module_token: String) -> Result<()> {
        let mut container = self.container.borrow_mut();
        let module_ref = container.get_module_by_token(&module_token);
        let resolved_module_ref = match module_ref {
            Some(module_ref) => module_ref,
            None => return Err(anyhow!("Module not found")),
        };

        let controllers = resolved_module_ref.get_metadata().controllers();

        if let Some(controllers) = controllers {
            let count = controllers.len();
            for controller in controllers {
                container.add_controller(&module_token, controller)?;
            }
            tracing::debug!(module = %module_token, count, "controllers registered");
        };

        Ok(())
    }

    pub fn insert_providers(&mut self, module_token: String) -> Result<()> {
        let mut container = self.container.borrow_mut();
        let module_ref = container.get_module_by_token(&module_token);
        let resolved_module_ref = match module_ref {
            Some(module_ref) => module_ref,
            None => return Err(anyhow!("Module not found")),
        };

        let providers = resolved_module_ref.get_metadata().providers();

        if let Some(providers) = providers {
            let count = providers.len();
            let mut app_guards: usize = 0;
            let mut app_interceptors: usize = 0;
            for provider in providers {
                let provider_token = provider.get_token();

                // Detect APP_* token providers and register them separately
                const APP_GUARD_NAME: &str = crate::di::APP_GUARD.name();
                const APP_INTERCEPTOR_NAME: &str = crate::di::APP_INTERCEPTOR.name();
                match provider_token.as_str() {
                    APP_GUARD_NAME => {
                        app_guards += 1;
                        let provider_type_token = provider.get_token();
                        container
                            .register_app_guard_provider(module_token.clone(), provider_type_token);
                    }
                    APP_INTERCEPTOR_NAME => {
                        app_interceptors += 1;
                        let provider_type_token = provider.get_token();
                        container.register_app_interceptor_provider(
                            module_token.clone(),
                            provider_type_token,
                        );
                    }
                    _ => {}
                }

                // Detect multi-provider contributions and record them by base token
                if let Some(base_token) = provider.get_multi_base_token() {
                    container.register_multi_provider(
                        base_token,
                        module_token.clone(),
                        provider_token,
                    );
                }

                container.add_provider(&module_token, provider)?;
            }
            tracing::debug!(
                module = %module_token,
                count,
                app_guards,
                app_interceptors,
                "providers registered"
            );
        };

        Ok(())
    }

    pub fn insert_exports(&mut self, module_token: String) -> Result<()> {
        let mut container = self.container.borrow_mut();
        let module_ref = container.get_module_by_token(&module_token);
        let resolved_module_ref = match module_ref {
            Some(module_ref) => module_ref,
            None => return Err(anyhow!("Module not found")),
        };

        let is_global = resolved_module_ref.get_metadata().is_global();
        let exports = resolved_module_ref.get_metadata().exports();

        if let Some(exports) = exports {
            let count = exports.len();
            tracing::debug!(module = %module_token, count, is_global, "exports registered");
            for export in exports {
                container.add_export(&module_token, export.clone())?;

                // If module is global, register export token as globally available
                if is_global {
                    container.register_global_provider_token(export);
                }
            }
        };

        Ok(())
    }

    pub fn scan_middleware(&mut self) -> Result<()> {
        let modules_token = self.container.borrow().get_modules_token();
        for module_token in modules_token {
            self.register_module_middleware(&module_token)?;
        }
        Ok(())
    }

    fn register_module_middleware(&mut self, module_token: &str) -> Result<()> {
        let middleware_configs = {
            let container = self.container.borrow();

            let module_ref = container
                .get_module_by_token(&module_token.to_string())
                .ok_or_else(|| anyhow!("Module not found: {}", module_token))?;

            let metadata = module_ref.get_metadata();

            let mut consumer = MiddlewareConsumer::new();
            metadata.configure_middleware(&mut consumer);
            consumer.build()
        };

        let mut container_mut = self.container.borrow_mut();

        let middleware_manager = container_mut
            .get_middleware_manager_mut()
            .ok_or_else(|| anyhow!("Middleware manager not initialized"))?;

        for config in middleware_configs {
            middleware_manager.add_for_module(module_token.to_string(), config);
        }

        Ok(())
    }

    pub async fn call_lifecycle_hooks(&mut self) -> Result<(), StartupError> {
        let modules_token = self.container.borrow().get_modules_token();

        for module_token in &modules_token {
            self.call_module_init_hook(module_token).await?;
        }

        self.call_provider_init_hooks(&modules_token).await?;

        Ok(())
    }

    /// Runs after `call_lifecycle_hooks` (OnModuleInit) but before the application starts listening.
    pub async fn call_bootstrap_hooks(&mut self) -> Result<(), StartupError> {
        let modules_token = self.container.borrow().get_modules_token();

        for module_token in &modules_token {
            self.call_module_bootstrap_hook(module_token).await?;
        }

        self.call_provider_bootstrap_hooks(&modules_token).await?;

        Ok(())
    }

    async fn call_module_bootstrap_hook(&mut self, module_token: &str) -> Result<(), StartupError> {
        {
            let container = self.container.borrow();
            let module_ref = container
                .get_module_by_token(&module_token.to_string())
                .ok_or_else(|| {
                    StartupError::from(anyhow::anyhow!("Module not found: {}", module_token))
                })?;

            tracing::debug!(module = %module_token, hook = "on_application_bootstrap", "lifecycle hook");
            module_ref
                .get_metadata()
                .on_application_bootstrap()
                .await
                .map_err(|source| StartupError::HookFailed {
                    module: module_token.to_string(),
                    hook: "on_application_bootstrap",
                    source,
                })?;
        }

        Ok(())
    }

    async fn call_provider_bootstrap_hooks(
        &self,
        modules_token: &[String],
    ) -> Result<(), StartupError> {
        for module_token in modules_token {
            {
                let container = self.container.borrow();
                if let Ok(providers) = container.get_lifecycle_instances(module_token) {
                    for provider in providers {
                        // Skip request-scoped providers — they are built into an
                        // execution, and bootstrap is not one.
                        if provider.get_scope() == crate::ProviderScope::Request {
                            continue;
                        }

                        tracing::debug!(module = %module_token, provider = %provider.get_token(), hook = "on_application_bootstrap", "lifecycle hook");
                        provider
                            .on_application_bootstrap()
                            .await
                            .map_err(|source| StartupError::HookFailed {
                                module: module_token.clone(),
                                hook: "on_application_bootstrap",
                                source,
                            })?;
                    }
                }
            }

            {
                let container = self.container.borrow();
                if let Some(module) = container.get_module_by_token(module_token) {
                    for controller in module.get_controller_objects() {
                        controller
                            .on_application_bootstrap()
                            .await
                            .map_err(|source| StartupError::HookFailed {
                                module: module_token.clone(),
                                hook: "on_application_bootstrap",
                                source,
                            })?;
                    }
                }
            }
        }
        Ok(())
    }

    async fn call_module_init_hook(&mut self, module_token: &str) -> Result<(), StartupError> {
        {
            let container = self.container.borrow();
            let module_ref = container
                .get_module_by_token(&module_token.to_string())
                .ok_or_else(|| {
                    StartupError::from(anyhow::anyhow!("Module not found: {}", module_token))
                })?;

            tracing::debug!(module = %module_token, hook = "on_module_init", "lifecycle hook");
            module_ref
                .get_metadata()
                .on_module_init()
                .await
                .map_err(|source| StartupError::HookFailed {
                    module: module_token.to_string(),
                    hook: "on_module_init",
                    source,
                })?;
        }

        Ok(())
    }

    async fn call_provider_init_hooks(&self, modules_token: &[String]) -> Result<(), StartupError> {
        for module_token in modules_token {
            {
                let container = self.container.borrow();
                if let Ok(providers) = container.get_lifecycle_instances(module_token) {
                    for provider in providers {
                        // Skip request-scoped providers — they are built into an
                        // execution, and module initialisation is not one.
                        if provider.get_scope() == crate::ProviderScope::Request {
                            continue;
                        }

                        tracing::debug!(module = %module_token, provider = %provider.get_token(), hook = "on_module_init", "lifecycle hook");
                        provider.on_module_init().await.map_err(|source| {
                            StartupError::HookFailed {
                                module: module_token.clone(),
                                hook: "on_module_init",
                                source,
                            }
                        })?;
                    }
                }
            }

            {
                let container = self.container.borrow();
                if let Some(module) = container.get_module_by_token(module_token) {
                    for controller in module.get_controller_objects() {
                        controller.on_module_init().await.map_err(|source| {
                            StartupError::HookFailed {
                                module: module_token.clone(),
                                hook: "on_module_init",
                                source,
                            }
                        })?;
                    }
                }
            }
        }
        Ok(())
    }
}
