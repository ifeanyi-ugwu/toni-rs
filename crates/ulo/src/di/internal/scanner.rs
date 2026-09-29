use parking_lot::RwLock;
use rustc_hash::FxHashMap;
use std::sync::Arc;

use crate::error::SetupResult;
use crate::error::StartupError;

use crate::{
    di::internal::Container,
    di::{MiddlewareConsumer, ModuleMetadata},
};

pub(crate) struct DependencyScanner {
    container: Arc<RwLock<Container>>,
}

impl DependencyScanner {
    pub(crate) fn new(container: Arc<RwLock<Container>>) -> Self {
        Self { container }
    }
    pub(crate) fn scan(&mut self, module: Box<dyn ModuleMetadata>) -> SetupResult {
        self.scan_for_modules_with_imports(module)?;
        self.scan_modules_for_dependencies()?;
        Ok(())
    }
    fn scan_for_modules_with_imports(&mut self, module: Box<dyn ModuleMetadata>) -> SetupResult {
        let mut ctx_registry: Vec<String> = vec![];

        let mut stack: Vec<Box<dyn ModuleMetadata>> = vec![module];

        while let Some(default_module) = stack.pop() {
            let module_id = default_module.identity().key();
            if ctx_registry.iter().any(|seen| seen == &module_id) {
                continue;
            }
            tracing::debug!(module = %module_id, "scanning module");
            // Dedup on the full key: two dynamic modules with different config share a base
            // but not a fingerprint, and both must survive to the clash check.
            ctx_registry.push(module_id.clone());

            let modules_imported = default_module.imports().unwrap_or_default();

            let mut modules_imported_tokens = vec![];

            for module_imported in modules_imported {
                let imported_id = module_imported.identity().key();
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

    pub(crate) fn scan_modules_for_dependencies(&mut self) -> SetupResult {
        let modules_token = self.container.read().module_tokens();
        for module_token in modules_token {
            self.insert_providers(module_token.clone())?;
            self.insert_controllers(module_token.clone())?;
            self.insert_exports(module_token.clone())?;
        }
        self.refuse_a_key_bound_both_ways()
    }

    /// A key holds one binding or a collection declared with `into`, never both (ADR-0057): a
    /// collection field finds a single binding it can see before the collection. Collections are
    /// registered by base key across modules, so the check runs once every module's providers are
    /// in.
    fn refuse_a_key_bound_both_ways(&self) -> SetupResult {
        let container = self.container.read();
        for (base, contributions) in container.multi_providers() {
            let Some((collected_in, _)) = contributions.first() else {
                continue;
            };
            for module_token in container.module_tokens() {
                let bound = container
                    .get_module_by_token(&module_token)
                    .is_some_and(|module| module.provider_factories().contains_key(base));
                if bound {
                    return Err(format!(
                        "`{base}` is bound in module `{module_token}` and collected with `into` \
                         in module `{collected_in}`; a key holds one binding or a collection, not \
                         both"
                    )
                    .into());
                }
            }
        }
        Ok(())
    }

    fn insert_module(&mut self, module: Box<dyn ModuleMetadata>) -> SetupResult {
        let mut container = self.container.write();
        container.add_module(module)
    }

    pub(crate) fn insert_imports(
        &mut self,
        module_token: String,
        imports: Vec<String>,
    ) -> SetupResult {
        let mut container = self.container.write();

        for import in imports {
            container.add_import(&module_token, import)?;
        }

        Ok(())
    }

    pub(crate) fn insert_controllers(&mut self, module_token: String) -> SetupResult {
        let mut container = self.container.write();
        let module_ref = container.get_module_by_token(&module_token);
        let resolved_module_ref = match module_ref {
            Some(module_ref) => module_ref,
            None => return Err("Module not found".to_string().into()),
        };

        let controllers = resolved_module_ref.metadata().controllers();

        if let Some(controllers) = controllers {
            let count = controllers.len();
            for controller in controllers {
                container.add_controller(&module_token, controller)?;
            }
            tracing::debug!(module = %module_token, count, "controllers registered");
        };

        Ok(())
    }

    pub(crate) fn insert_providers(&mut self, module_token: String) -> SetupResult {
        let mut container = self.container.write();
        let module_ref = container.get_module_by_token(&module_token);
        let resolved_module_ref = match module_ref {
            Some(module_ref) => module_ref,
            None => return Err("Module not found".to_string().into()),
        };

        let providers = resolved_module_ref.metadata().providers();

        if let Some(providers) = providers {
            let count = providers.len();
            let mut app_guards: usize = 0;
            let mut app_interceptors: usize = 0;
            // Each binding by its 1-based position in the module's provider list, so a second one
            // under a key is refused naming both (ADR-0057). Contributions carry tokens of their
            // own, so only single bindings can meet here.
            let mut bound: FxHashMap<String, usize> = FxHashMap::default();
            for (position, provider) in providers.into_iter().enumerate() {
                let provider_token = provider.token();
                if let Some(first) = bound.insert(provider_token.clone(), position + 1) {
                    return Err(format!(
                        "`{provider_token}` is bound twice in module `{module_token}`, by its \
                         provider entries {first} and {}; a key holds one binding, and `into` \
                         collects several",
                        position + 1
                    )
                    .into());
                }

                // Detect multi-provider contributions and record them by base token. The unnamed
                // collection of an HTTP guard or interceptor type is that transport's global set.
                if let Some(base_token) = provider.multi_base_token() {
                    match crate::di::global_collection(&base_token)? {
                        Some(crate::di::GlobalCollection::HttpGuards) => {
                            app_guards += 1;
                            container.register_app_guard_provider(
                                module_token.clone(),
                                provider_token.clone(),
                            );
                        }
                        Some(crate::di::GlobalCollection::HttpInterceptors) => {
                            app_interceptors += 1;
                            container.register_app_interceptor_provider(
                                module_token.clone(),
                                provider_token.clone(),
                            );
                        }
                        None => {}
                    }
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

    pub(crate) fn insert_exports(&mut self, module_token: String) -> SetupResult {
        let mut container = self.container.write();
        let module_ref = container.get_module_by_token(&module_token);
        let resolved_module_ref = match module_ref {
            Some(module_ref) => module_ref,
            None => return Err("Module not found".to_string().into()),
        };

        let is_global = resolved_module_ref.metadata().is_global();
        let exports = resolved_module_ref.metadata().exports();

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

    pub(crate) fn scan_middleware(&mut self) -> SetupResult {
        let modules_token = self.container.read().module_tokens();
        for module_token in modules_token {
            self.register_module_middleware(&module_token)?;
        }
        Ok(())
    }

    fn register_module_middleware(&mut self, module_token: &str) -> SetupResult {
        let middleware_configs = {
            let container = self.container.read();

            let module_ref = container
                .get_module_by_token(&module_token.to_string())
                .ok_or_else(|| format!("Module not found: {}", module_token))?;

            let metadata = module_ref.metadata();

            let mut consumer = MiddlewareConsumer::new();
            metadata.configure_middleware(&mut consumer);
            consumer.build()
        };

        let mut container_mut = self.container.write();

        let middleware_manager = container_mut
            .middleware_manager_mut()
            .ok_or_else(|| "Middleware manager not initialized".to_string())?;

        for config in middleware_configs {
            middleware_manager.add_for_module(module_token.to_string(), config);
        }

        Ok(())
    }

    pub(crate) async fn call_lifecycle_hooks(&mut self) -> Result<(), StartupError> {
        let modules_token = self.container.read().module_tokens();

        for module_token in &modules_token {
            self.call_module_init_hook(module_token).await?;
        }

        self.call_provider_init_hooks(&modules_token).await?;

        Ok(())
    }

    /// Runs after `call_lifecycle_hooks` (OnModuleInit) but before the application starts listening.
    pub(crate) async fn call_bootstrap_hooks(&mut self) -> Result<(), StartupError> {
        let modules_token = self.container.read().module_tokens();

        for module_token in &modules_token {
            self.call_module_bootstrap_hook(module_token).await?;
        }

        self.call_provider_bootstrap_hooks(&modules_token).await?;

        Ok(())
    }

    async fn call_module_bootstrap_hook(&mut self, module_token: &str) -> Result<(), StartupError> {
        let metadata = {
            let container = self.container.read();
            container
                .get_module_by_token(&module_token.to_string())
                .ok_or_else(|| {
                    StartupError::Setup(format!("Module not found: {module_token}").into())
                })?
                .metadata()
        };

        tracing::debug!(module = %module_token, hook = "on_application_bootstrap", "lifecycle hook");
        metadata
            .on_application_bootstrap()
            .await
            .map_err(|source| StartupError::HookFailed {
                module: module_token.to_string(),
                hook: "on_application_bootstrap",
                source,
            })
    }

    async fn call_provider_bootstrap_hooks(
        &self,
        modules_token: &[String],
    ) -> Result<(), StartupError> {
        for module_token in modules_token {
            let lifecycle = self.container.read().module_lifecycle(module_token);
            let Some(lifecycle) = lifecycle else {
                continue;
            };

            for provider in lifecycle.providers {
                tracing::debug!(module = %module_token, provider = %provider.token(), hook = "on_application_bootstrap", "lifecycle hook");
                provider
                    .on_application_bootstrap()
                    .await
                    .map_err(|source| StartupError::HookFailed {
                        module: module_token.clone(),
                        hook: "on_application_bootstrap",
                        source,
                    })?;
            }

            for controller in lifecycle.controllers {
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
        Ok(())
    }

    async fn call_module_init_hook(&mut self, module_token: &str) -> Result<(), StartupError> {
        let metadata = {
            let container = self.container.read();
            container
                .get_module_by_token(&module_token.to_string())
                .ok_or_else(|| {
                    StartupError::Setup(format!("Module not found: {module_token}").into())
                })?
                .metadata()
        };

        tracing::debug!(module = %module_token, hook = "on_module_init", "lifecycle hook");
        metadata
            .on_module_init()
            .await
            .map_err(|source| StartupError::HookFailed {
                module: module_token.to_string(),
                hook: "on_module_init",
                source,
            })
    }

    async fn call_provider_init_hooks(&self, modules_token: &[String]) -> Result<(), StartupError> {
        for module_token in modules_token {
            let lifecycle = self.container.read().module_lifecycle(module_token);
            let Some(lifecycle) = lifecycle else {
                continue;
            };

            for provider in lifecycle.providers {
                tracing::debug!(module = %module_token, provider = %provider.token(), hook = "on_module_init", "lifecycle hook");
                provider
                    .on_module_init()
                    .await
                    .map_err(|source| StartupError::HookFailed {
                        module: module_token.clone(),
                        hook: "on_module_init",
                        source,
                    })?;
            }

            for controller in lifecycle.controllers {
                controller
                    .on_module_init()
                    .await
                    .map_err(|source| StartupError::HookFailed {
                        module: module_token.clone(),
                        hook: "on_module_init",
                        source,
                    })?;
            }
        }
        Ok(())
    }
}
