use std::{collections::hash_map::Drain, sync::Arc};

use crate::error::SetupResult;
use rustc_hash::{FxHashMap, FxHashSet};

use crate::{
    di::ModuleMetadata,
    dispatch::transport::{EnhancerSet, Grpc, Http, Rpc, Ws},
    dispatch::{Controller, ControllerFactory},
    http::middleware::MiddlewareManager,
    spi::{Provider, ProviderFactory, ProviderRole},
    ws::Gateway,
};

use crate::dispatch::registry::RoleRegistry;

use super::FxIndexMap;
use super::module::Module;
use crate::di::ProviderScope;
use crate::http::RoutePipeline;

/// What one module contributes to a lifecycle phase, detached from the container.
///
/// Produced by [`Container::module_lifecycle`]; see there for why the handles are cloned out.
pub(crate) struct ModuleLifecycle {
    pub(crate) metadata: Arc<dyn ModuleMetadata>,
    pub(crate) providers: Vec<Arc<Box<dyn Provider>>>,
    pub(crate) controllers: Vec<Arc<dyn Controller>>,
}

pub(crate) struct Container {
    modules: FxIndexMap<String, Module>,
    /// Modules in the order the loader finished building their providers. The loader takes
    /// imports before importers and retries a module waiting on a provider not yet built in a later
    /// pass, so a module finishes after every module whose providers it injects. The lifecycle hooks
    /// walk it, and shutdown walks it back.
    construction_order: Vec<String>,
    middleware_manager: Option<MiddlewareManager>,
    /// Global provider registry - providers from modules marked as global
    global_providers: FxHashMap<String, Arc<Box<dyn Provider>>>,
    /// Owner of each global token: `(module identity, display name)`. Keyed by export token. Lets
    /// a second, distinct module claiming the same token fail loudly with both names instead of
    /// silently shadowing. Two dynamic modules with different config share a display name but not an
    /// identity, so the comparison keys on the identity, not the name.
    global_provider_sources: FxHashMap<String, (String, String)>,
    /// Global provider tokens - registered during scan phase (before instance creation)
    global_provider_tokens: FxHashSet<String>,
    /// What each transport runs on every one of its dispatch targets.
    pub(crate) global_http: EnhancerSet<Http>,
    pub(crate) global_rpc: EnhancerSet<Rpc>,
    pub(crate) global_ws: EnhancerSet<Ws>,
    pub(crate) global_grpc: EnhancerSet<Grpc>,
    /// Contributions to the HTTP guard and interceptor collections, as (module, provider) tokens,
    /// resolved to HTTP's global enhancers once every provider is built.
    app_guard_providers: Vec<(String, String)>,
    app_interceptor_providers: Vec<(String, String)>,
    /// Multi-provider registry: base_token -> Vec<(module_token, provider_token)>.
    /// Populated during the scan phase; the instance loader uses this to collect contributions
    /// into a MultiCollectionProvider after all individual providers are built.
    multi_providers: FxHashMap<String, Vec<(String, String)>>,
    /// Fully-collected multi-provider instances, keyed by base token.
    /// Built by the instance loader after Phase 1 and resolved like regular providers.
    multi_collection_providers: FxHashMap<String, Arc<Box<dyn Provider>>>,
    /// Per-role registries populated by `ProviderFactory::extract_roles` at instance creation.
    role_registry: RoleRegistry,
}

impl Default for Container {
    fn default() -> Self {
        Self::new()
    }
}

impl Container {
    pub(crate) fn new() -> Self {
        Self {
            modules: FxIndexMap::default(),
            construction_order: Vec::new(),
            middleware_manager: Some(MiddlewareManager::new()),
            global_http: EnhancerSet::default(),
            global_rpc: EnhancerSet::default(),
            global_ws: EnhancerSet::default(),
            global_grpc: EnhancerSet::default(),
            global_providers: FxHashMap::default(),
            global_provider_sources: FxHashMap::default(),
            global_provider_tokens: FxHashSet::default(),
            app_guard_providers: Vec::new(),
            app_interceptor_providers: Vec::new(),
            multi_providers: FxHashMap::default(),
            multi_collection_providers: FxHashMap::default(),
            role_registry: RoleRegistry::new(),
        }
    }

    pub(crate) fn add_module(&mut self, module_metadata: Box<dyn ModuleMetadata>) -> SetupResult {
        let token: String = module_metadata.identity().key();
        // The token is the full identity key (type name for static modules, base + config
        // fingerprint for dynamic ones). The scanner skips a module it has already scanned; a
        // repeat from any other caller is deduped rather than overwritten. Distinct modules that
        // resolve to the same exported provider token are caught later, at global-provider
        // registration.
        if self.modules.contains_key(&token) {
            return Ok(());
        }
        let module = Module::new(module_metadata);
        self.modules.insert(token, module);
        Ok(())
    }

    pub(crate) fn add_import(
        &mut self,
        module_ref_token: &String,
        imported_module_token: String,
    ) -> SetupResult {
        let module_ref = self
            .modules
            .get_mut(module_ref_token)
            .ok_or_else(|| "Module not found".to_string())?;
        module_ref.add_import(imported_module_token);
        Ok(())
    }

    pub(crate) fn add_controller(
        &mut self,
        module_ref_token: &String,
        controller: Box<dyn ControllerFactory>,
    ) -> SetupResult {
        let module_ref = self
            .modules
            .get_mut(module_ref_token)
            .ok_or_else(|| "Module not found".to_string())?;
        module_ref.add_controller(controller);
        Ok(())
    }

    pub(crate) fn add_provider(
        &mut self,
        module_ref_token: &String,
        provider: Box<dyn ProviderFactory>,
    ) -> SetupResult {
        let module_ref = self
            .modules
            .get_mut(module_ref_token)
            .ok_or_else(|| "Module not found".to_string())?;
        module_ref.add_provider(provider);
        Ok(())
    }

    pub(crate) fn add_provider_instance(
        &mut self,
        module_ref_token: &String,
        provider_instance: Arc<Box<dyn Provider>>,
        roles: Vec<ProviderRole>,
    ) -> SetupResult {
        let token = provider_instance.token();

        for role in roles {
            match role {
                ProviderRole::HttpGuard(g) => {
                    self.role_registry.http.guards.insert(token.clone(), g);
                }
                ProviderRole::HttpInterceptor(i) => {
                    self.role_registry
                        .http
                        .interceptors
                        .insert(token.clone(), i);
                }
                ProviderRole::HttpErrorHandler(eh) => {
                    self.role_registry
                        .http
                        .error_handlers
                        .insert(token.clone(), eh);
                }
                ProviderRole::RpcGuard(g) => {
                    self.role_registry.rpc.guards.insert(token.clone(), g);
                }
                ProviderRole::RpcInterceptor(i) => {
                    self.role_registry.rpc.interceptors.insert(token.clone(), i);
                }
                ProviderRole::RpcErrorHandler(eh) => {
                    self.role_registry
                        .rpc
                        .error_handlers
                        .insert(token.clone(), eh);
                }
                ProviderRole::WsGuard(g) => {
                    self.role_registry.ws.guards.insert(token.clone(), g);
                }
                ProviderRole::WsInterceptor(i) => {
                    self.role_registry.ws.interceptors.insert(token.clone(), i);
                }
                ProviderRole::WsErrorHandler(eh) => {
                    self.role_registry
                        .ws
                        .error_handlers
                        .insert(token.clone(), eh);
                }
                ProviderRole::GrpcGuard(g) => {
                    self.role_registry.grpc.guards.insert(token.clone(), g);
                }
                ProviderRole::GrpcInterceptor(i) => {
                    self.role_registry
                        .grpc
                        .interceptors
                        .insert(token.clone(), i);
                }
                ProviderRole::GrpcErrorHandler(eh) => {
                    self.role_registry
                        .grpc
                        .error_handlers
                        .insert(token.clone(), eh);
                }
                ProviderRole::Middleware(m) => {
                    self.role_registry.middleware.insert(token.clone(), m);
                }
                ProviderRole::Gateway(gw) => {
                    let path = gw.path();
                    self.role_registry.gateways.insert(path, gw);
                }
            }
        }

        let module_ref = self
            .modules
            .get_mut(module_ref_token)
            .ok_or_else(|| "Module not found".to_string())?;
        module_ref.add_provider_instance(provider_instance);
        Ok(())
    }

    /// One module's hook-carrying handles, cloned out of the container: providers in the order the
    /// loader built them, controllers in the order they are declared.
    ///
    /// Every lifecycle hook is awaited and the container sits behind a lock, so a caller
    /// takes the handles and releases the lock rather than holding it across each await.
    ///
    /// Execution-scoped providers are left out: they are built into an execution, and no
    /// lifecycle phase is one.
    pub(crate) fn module_lifecycle(&self, module_ref_token: &str) -> Option<ModuleLifecycle> {
        let module = self.modules.get(module_ref_token)?;
        Some(ModuleLifecycle {
            metadata: module.metadata(),
            providers: module
                .provider_instances()
                .values()
                .filter(|provider| provider.scope() != ProviderScope::Execution)
                .cloned()
                .collect(),
            controllers: module.controller_objects().to_vec(),
        })
    }

    /// Register an RPC controller's resolved wrapper under its token. Called from the controller
    /// path at create, with enhancer tokens already resolved against the role registry.
    ///
    /// A pattern another controller already declares is refused, naming both. Left in, dispatch
    /// would reach one of the two by the registry's hash order, and `NatsAdapter`, which subscribes
    /// once per entry in the pattern list, would run every call twice.
    pub(crate) fn add_rpc_controller(
        &mut self,
        token: String,
        wrapper: Arc<crate::rpc::RpcControllerWrapper>,
    ) -> SetupResult {
        // An earlier registration under this same token is not compared: it is the token's
        // collision, not the pattern's.
        for pattern in wrapper.patterns() {
            let declared_by =
                self.role_registry
                    .rpc_controllers
                    .iter()
                    .find(|(other, registered)| {
                        **other != token && registered.patterns().contains(&pattern)
                    });
            if let Some((other, _)) = declared_by {
                return Err(format!(
                    "RPC pattern `{pattern}` is declared by two controllers, `{other}` and \
                     `{token}`; a pattern has one handler"
                )
                .into());
            }
        }
        self.role_registry.rpc_controllers.insert(token, wrapper);
        Ok(())
    }

    /// Register a gRPC service and its resolved enhancer bundle under the service's token.
    pub(crate) fn add_grpc_service(
        &mut self,
        token: String,
        service: (
            Arc<dyn crate::grpc::GrpcServiceSource>,
            Arc<crate::grpc::ResolvedGrpcEnhancers>,
        ),
    ) {
        self.role_registry.grpc_services.insert(token, service);
    }

    pub(crate) fn role_registry(&self) -> &RoleRegistry {
        &self.role_registry
    }

    pub(crate) fn provider_roles(&self, token: &str) -> Vec<crate::spi::ProviderRole> {
        self.role_registry.get_roles_for_token(token)
    }

    pub(crate) fn gateways(&self) -> &FxHashMap<String, Arc<Box<dyn Gateway>>> {
        &self.role_registry.gateways
    }

    pub(crate) fn rpc_controllers(
        &self,
    ) -> &FxHashMap<String, Arc<crate::rpc::RpcControllerWrapper>> {
        &self.role_registry.rpc_controllers
    }

    pub(crate) fn grpc_services(
        &self,
    ) -> &FxHashMap<
        String,
        (
            Arc<dyn crate::grpc::GrpcServiceSource>,
            Arc<crate::grpc::ResolvedGrpcEnhancers>,
        ),
    > {
        &self.role_registry.grpc_services
    }

    /// Resolve middleware tokens for one module against the role registry.
    ///
    /// Called from the instance loader after all providers are instantiated so
    /// that the registry is fully populated before middleware is resolved.
    pub(crate) fn resolve_module_middleware(&mut self, module_token: &str) -> SetupResult {
        if let Some(manager) = self.middleware_manager.as_mut() {
            manager.resolve_middleware_tokens(module_token, &self.role_registry.middleware)?;
        }
        Ok(())
    }

    pub(crate) fn add_controller_object(
        &mut self,
        module_ref_token: &String,
        controller: Arc<dyn Controller>,
    ) -> SetupResult {
        let module_ref = self
            .modules
            .get_mut(module_ref_token)
            .ok_or_else(|| "Module not found".to_string())?;
        module_ref.add_controller_object(controller);
        Ok(())
    }

    pub(crate) fn add_route_instance(
        &mut self,
        module_ref_token: &String,
        controller_token: &str,
        route: Arc<dyn crate::http::Route>,
        enhancer_metadata: EnhancerSet<Http>,
    ) -> SetupResult {
        let module_ref = self
            .modules
            .get_mut(module_ref_token)
            .ok_or_else(|| "Module not found".to_string())?;
        module_ref.add_route_instance(controller_token, route, enhancer_metadata);
        Ok(())
    }

    pub(crate) fn add_export(
        &mut self,
        module_ref_token: &String,
        provider_token: String,
    ) -> SetupResult {
        let module_ref = self
            .modules
            .get_mut(module_ref_token)
            .ok_or_else(|| "Module not found".to_string())?;
        module_ref.add_export(provider_token);
        Ok(())
    }

    pub(crate) fn add_export_instance(
        &mut self,
        module_ref_token: &String,
        provider_token: String,
    ) -> SetupResult {
        let module_ref = self
            .modules
            .get_mut(module_ref_token)
            .ok_or_else(|| "Module not found".to_string())?;
        module_ref.add_export_instance(provider_token);
        Ok(())
    }

    pub(crate) fn provider_factories(
        &self,
        module_ref_token: &String,
    ) -> SetupResult<&FxIndexMap<String, Arc<dyn ProviderFactory>>> {
        let module_ref = self
            .modules
            .get(module_ref_token)
            .ok_or_else(|| "Module not found".to_string())?;
        Ok(module_ref.provider_factories())
    }

    pub(crate) fn controller_factories(
        &self,
        module_ref_token: &String,
    ) -> SetupResult<&FxIndexMap<String, Arc<dyn ControllerFactory>>> {
        let module_ref = self
            .modules
            .get(module_ref_token)
            .ok_or_else(|| "Module not found".to_string())?;
        Ok(module_ref.controller_factories())
    }

    pub(crate) fn get_provider_instances(
        &self,
        module_ref_token: &String,
    ) -> SetupResult<&FxIndexMap<String, Arc<Box<dyn Provider>>>> {
        let module_ref = self
            .modules
            .get(module_ref_token)
            .ok_or_else(|| "Module not found".to_string())?;
        Ok(module_ref.provider_instances())
    }

    pub(crate) fn get_provider_instance_by_token(
        &self,
        module_ref_token: &String,
        provider_token: &String,
    ) -> SetupResult<Option<&Arc<Box<dyn Provider>>>> {
        let module_ref = self
            .modules
            .get(module_ref_token)
            .ok_or_else(|| "Module not found".to_string())?;
        Ok(module_ref.get_provider_instance_by_token(provider_token))
    }

    pub(crate) fn get_provider_by_token(
        &self,
        module_ref_token: &String,
        provider_token: &String,
    ) -> SetupResult<Option<Arc<dyn ProviderFactory>>> {
        let module_ref = self
            .modules
            .get(module_ref_token)
            .ok_or_else(|| "Module not found".to_string())?;
        Ok(module_ref.get_provider_by_token(provider_token))
    }

    pub(crate) fn get_controller_instances(
        &mut self,
        module_ref_token: &String,
    ) -> SetupResult<Drain<'_, String, Arc<RoutePipeline>>> {
        let module_ref = self
            .modules
            .get_mut(module_ref_token)
            .ok_or_else(|| "Module not found".to_string())?;
        Ok(module_ref.drain_controllers_instances())
    }

    pub(crate) fn imported_modules(
        &self,
        module_ref_token: &String,
    ) -> SetupResult<&FxHashSet<String>> {
        let module_ref = self
            .modules
            .get(module_ref_token)
            .ok_or_else(|| "Module not found".to_string())?;
        Ok(module_ref.imported_modules())
    }

    pub(crate) fn exported_instance_tokens(
        &self,
        module_ref_token: &String,
    ) -> SetupResult<&FxHashSet<String>> {
        let module_ref = self
            .modules
            .get(module_ref_token)
            .ok_or_else(|| format!("Module not found: {:?}", module_ref_token))?;
        Ok(module_ref.exported_instance_tokens())
    }

    pub(crate) fn exported_tokens_of(&self, module_ref_token: &String) -> SetupResult<Vec<String>> {
        let module_ref = self
            .modules
            .get(module_ref_token)
            .ok_or_else(|| format!("Module not found: {:?}", module_ref_token))?;
        Ok(module_ref.exported_tokens().iter().cloned().collect())
    }

    pub(crate) fn module_tokens(&self) -> Vec<String> {
        self.modules.keys().cloned().collect::<Vec<String>>()
    }

    /// Record that the loader has built every provider of `module_token`.
    pub(crate) fn record_construction(&mut self, module_token: &str) {
        self.construction_order.push(module_token.to_string());
    }

    /// Modules in construction order, which the lifecycle hooks follow (ADR-0057).
    pub(crate) fn modules_in_construction_order(&self) -> Vec<String> {
        self.construction_order.clone()
    }

    pub(crate) fn ordered_module_tokens(&self) -> Vec<String> {
        let mut ordered_modules: Vec<String> = Vec::new();
        let mut visited: FxHashMap<String, bool> = FxHashMap::default();

        // Standard topological sort based on explicit imports
        while ordered_modules.len() < self.modules.len() {
            let mut ready_modules: Vec<String> = Vec::new();

            for (token, module) in self.modules.iter() {
                if visited.contains_key(token) {
                    continue;
                }

                let imported_modules = module.imported_modules();
                let all_imports_processed = imported_modules
                    .iter()
                    .all(|import_token| visited.contains_key(import_token));

                if all_imports_processed {
                    ready_modules.push(token.clone());
                }
            }

            if ready_modules.is_empty() {
                // The remaining modules form an import cycle. An import edge is a
                // visibility relationship, not a construction dependency, so a cycle
                // here is not inherently fatal — dropping these modules would silently
                // omit their providers. Append them in a deterministic order and let
                // the Phase-1 deferred-retry loop resolve instantiation order (or, if
                // their providers genuinely cross-depend, surface a precise cycle error).
                let mut remaining: Vec<String> = self
                    .modules
                    .keys()
                    .filter(|token| !visited.contains_key(*token))
                    .cloned()
                    .collect();
                remaining.sort();
                for token in remaining {
                    ordered_modules.push(token.clone());
                    visited.insert(token, true);
                }
                break;
            }

            for token in ready_modules {
                ordered_modules.push(token.clone());
                visited.insert(token.clone(), true);
            }
        }

        ordered_modules
    }

    pub(crate) fn get_module_by_token(&self, module_ref_token: &String) -> Option<&Module> {
        self.modules.get(module_ref_token)
    }

    /// Register all exported providers from a global module into the global registry
    pub(crate) fn register_global_providers(&mut self, module_token: &String) -> SetupResult {
        let (is_global, module_name, exports_tokens) = {
            let module = self
                .modules
                .get(module_token)
                .ok_or_else(|| format!("Module not found: {}", module_token))?;
            (
                module.metadata().is_global(),
                module.metadata().identity().key(),
                module.exported_instance_tokens().clone(),
            )
        };

        // Only register if module is marked as global
        if !is_global {
            return Ok(());
        }

        // Register all exported providers as globally accessible
        for export_token in exports_tokens.iter() {
            // Two distinct global modules exporting the same token is the shape of the
            // "two connections, no markers" mistake: both provide `DatabaseConnection`, and one
            // would silently shadow the other at injection. Refuse it and point at the fix. The
            // owner is keyed by identity, not display name — two dynamic modules with different
            // config share a name but are genuinely different modules.
            if let Some((owner_token, owner_name)) = self.global_provider_sources.get(export_token)
            {
                if owner_token != module_token {
                    return Err(format!(
                        "provider '{export_token}' is exported globally by two modules \
                         ('{owner_name}' and '{module_name}'). One would silently shadow the other. \
                         If these are separate instances of the same integration, register each \
                         under a marker of its own — integrations expose a keyed constructor for \
                         this (e.g. `for_root_keyed::<K>`), `K` declared with `key!` — and inject \
                         it with `#[inject(K)]`."
                    ).into());
                }
            }
            if let Ok(Some(instance)) =
                self.get_provider_instance_by_token(module_token, export_token)
            {
                self.global_providers
                    .insert(export_token.clone(), instance.clone());
                self.global_provider_sources.insert(
                    export_token.clone(),
                    (module_token.clone(), module_name.clone()),
                );
            }
        }

        Ok(())
    }

    /// The global registry: every token a global module exports, with its instance.
    pub(crate) fn global_providers(&self) -> &FxHashMap<String, Arc<Box<dyn Provider>>> {
        &self.global_providers
    }

    /// Get a provider from the global registry
    pub(crate) fn get_global_provider(&self, token: &String) -> Option<Arc<Box<dyn Provider>>> {
        self.global_providers.get(token).cloned()
    }

    /// Register a provider token as globally available (during scan phase)
    pub(crate) fn register_global_provider_token(&mut self, token: String) {
        self.global_provider_tokens.insert(token);
    }

    /// Check if a provider token is registered as globally available
    pub(crate) fn is_global_provider_token(&self, token: &String) -> bool {
        self.global_provider_tokens.contains(token)
    }

    // pub fn register_controller_enhancers(
    //     &mut self,
    //     module_ref_token: &String,
    //     controller_token: &String,
    //     controller_enhancers: &Vec<Box<dyn ControllerEnhancer>>,
    // ) -> SetupResult {
    //     let module_ref = self
    //         .modules
    //         .get_mut(module_ref_token)
    //         .ok_or_else(|| "Module not found".to_string())?;
    //     module_ref.register_controller_enhancers(controller_enhancers);
    //     Ok(())
    // }

    pub(crate) fn middleware_manager(&self) -> Option<&MiddlewareManager> {
        self.middleware_manager.as_ref()
    }

    pub(crate) fn middleware_manager_mut(&mut self) -> Option<&mut MiddlewareManager> {
        self.middleware_manager.as_mut()
    }

    /// Records a contribution to the HTTP guard collection, HTTP's global guards.
    pub(crate) fn register_app_guard_provider(
        &mut self,
        module_token: String,
        provider_token: String,
    ) {
        self.app_guard_providers
            .push((module_token, provider_token));
    }

    /// Records a contribution to the HTTP interceptor collection, HTTP's global interceptors.
    pub(crate) fn register_app_interceptor_provider(
        &mut self,
        module_token: String,
        provider_token: String,
    ) {
        self.app_interceptor_providers
            .push((module_token, provider_token));
    }

    /// The contributions to the HTTP guard collection, in scan order.
    pub(crate) fn app_guard_providers(&self) -> &[(String, String)] {
        &self.app_guard_providers
    }

    /// The contributions to the HTTP interceptor collection, in scan order.
    pub(crate) fn app_interceptor_providers(&self) -> &[(String, String)] {
        &self.app_interceptor_providers
    }

    /// Register one multi-provider contribution during the scan phase.
    pub(crate) fn register_multi_provider(
        &mut self,
        base_token: String,
        module_token: String,
        provider_token: String,
    ) {
        self.multi_providers
            .entry(base_token)
            .or_default()
            .push((module_token, provider_token));
    }

    pub(crate) fn multi_providers(&self) -> &FxHashMap<String, Vec<(String, String)>> {
        &self.multi_providers
    }

    pub(crate) fn add_multi_collection_provider(
        &mut self,
        base_token: String,
        instance: Arc<Box<dyn Provider>>,
    ) {
        self.multi_collection_providers.insert(base_token, instance);
    }

    pub(crate) fn get_multi_collection_provider(
        &self,
        base_token: &str,
    ) -> Option<Arc<Box<dyn Provider>>> {
        self.multi_collection_providers.get(base_token).cloned()
    }
}
