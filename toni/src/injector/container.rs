use std::{collections::hash_map::Drain, sync::Arc};

use anyhow::{Result, anyhow};
use rustc_hash::{FxHashMap, FxHashSet};

use crate::{
    middleware::MiddlewareManager,
    rpc::RpcControllerSource,
    structs_helpers::EnhancerMetadata,
    traits_helpers::{
        Controller, ControllerFactory, ErrorObserver, GrpcErrorHandlerArc, GrpcGuardEntry,
        GrpcInterceptorEntry, HttpErrorHandlerArc, HttpGuardEntry, HttpInterceptorEntry,
        ModuleMetadata, Provider, ProviderFactory, ProviderRole, RpcErrorHandlerArc, RpcGuardEntry,
        RpcInterceptorEntry, WsErrorHandlerArc, WsGuardEntry, WsInterceptorEntry,
    },
    websocket::GatewayTrait,
};

use super::{InstanceWrapper, RoleRegistry, module::Module};

pub struct ToniContainer {
    modules: FxHashMap<String, Module>,
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
    /// Global enhancers - applied to every HTTP route's pipeline.
    global_http_guards: Vec<HttpGuardEntry>,
    global_http_interceptors: Vec<HttpInterceptorEntry>,
    global_http_error_handlers: Vec<HttpErrorHandlerArc>,
    /// Global enhancers - applied to every RPC controller's pipeline.
    global_rpc_guards: Vec<RpcGuardEntry>,
    global_rpc_interceptors: Vec<RpcInterceptorEntry>,
    global_rpc_error_handlers: Vec<RpcErrorHandlerArc>,
    /// Global enhancers - applied to every WS gateway's pipeline.
    global_ws_guards: Vec<WsGuardEntry>,
    global_ws_interceptors: Vec<WsInterceptorEntry>,
    global_ws_error_handlers: Vec<WsErrorHandlerArc>,
    /// Global enhancers - applied to every gRPC service's pipeline.
    global_grpc_guards: Vec<GrpcGuardEntry>,
    global_grpc_interceptors: Vec<GrpcInterceptorEntry>,
    global_grpc_error_handlers: Vec<GrpcErrorHandlerArc>,
    /// Universal error observers — fire on any framework-generated error
    /// across every transport.
    global_error_observers: Vec<Arc<dyn ErrorObserver>>,
    /// APP_* token providers - providers registered with special tokens (module_token, provider_token)
    /// These will be resolved to global enhancers after DI container is built
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

impl Default for ToniContainer {
    fn default() -> Self {
        Self::new()
    }
}

impl ToniContainer {
    pub fn new() -> Self {
        Self {
            modules: FxHashMap::default(),
            middleware_manager: Some(MiddlewareManager::new()),
            global_providers: FxHashMap::default(),
            global_provider_sources: FxHashMap::default(),
            global_provider_tokens: FxHashSet::default(),
            global_http_guards: Vec::new(),
            global_http_interceptors: Vec::new(),
            global_http_error_handlers: Vec::new(),
            global_rpc_guards: Vec::new(),
            global_rpc_interceptors: Vec::new(),
            global_rpc_error_handlers: Vec::new(),
            global_ws_guards: Vec::new(),
            global_ws_interceptors: Vec::new(),
            global_ws_error_handlers: Vec::new(),
            global_grpc_guards: Vec::new(),
            global_grpc_interceptors: Vec::new(),
            global_grpc_error_handlers: Vec::new(),
            global_error_observers: Vec::new(),
            app_guard_providers: Vec::new(),
            app_interceptor_providers: Vec::new(),
            multi_providers: FxHashMap::default(),
            multi_collection_providers: FxHashMap::default(),
            role_registry: RoleRegistry::new(),
        }
    }

    pub fn add_global_http_guard(&mut self, guard: HttpGuardEntry) {
        self.global_http_guards.push(guard);
    }

    pub fn add_global_http_interceptor(&mut self, interceptor: HttpInterceptorEntry) {
        self.global_http_interceptors.push(interceptor);
    }

    pub fn add_global_http_error_handler(&mut self, handler: HttpErrorHandlerArc) {
        self.global_http_error_handlers.push(handler);
    }

    pub fn add_global_rpc_guard(&mut self, guard: RpcGuardEntry) {
        self.global_rpc_guards.push(guard);
    }

    pub fn add_global_rpc_interceptor(&mut self, interceptor: RpcInterceptorEntry) {
        self.global_rpc_interceptors.push(interceptor);
    }

    pub fn add_global_rpc_error_handler(&mut self, handler: RpcErrorHandlerArc) {
        self.global_rpc_error_handlers.push(handler);
    }

    pub fn get_global_rpc_guards(&self) -> Vec<RpcGuardEntry> {
        self.global_rpc_guards.clone()
    }

    pub fn get_global_rpc_interceptors(&self) -> Vec<RpcInterceptorEntry> {
        self.global_rpc_interceptors.clone()
    }

    pub fn get_global_rpc_error_handlers(&self) -> Vec<RpcErrorHandlerArc> {
        self.global_rpc_error_handlers.clone()
    }

    pub fn add_global_ws_guard(&mut self, guard: WsGuardEntry) {
        self.global_ws_guards.push(guard);
    }

    pub fn add_global_ws_interceptor(&mut self, interceptor: WsInterceptorEntry) {
        self.global_ws_interceptors.push(interceptor);
    }

    pub fn add_global_ws_error_handler(&mut self, handler: WsErrorHandlerArc) {
        self.global_ws_error_handlers.push(handler);
    }

    pub fn get_global_ws_guards(&self) -> Vec<WsGuardEntry> {
        self.global_ws_guards.clone()
    }

    pub fn get_global_ws_interceptors(&self) -> Vec<WsInterceptorEntry> {
        self.global_ws_interceptors.clone()
    }

    pub fn get_global_ws_error_handlers(&self) -> Vec<WsErrorHandlerArc> {
        self.global_ws_error_handlers.clone()
    }

    pub fn add_global_grpc_guard(&mut self, guard: GrpcGuardEntry) {
        self.global_grpc_guards.push(guard);
    }

    pub fn get_global_grpc_guards(&self) -> Vec<GrpcGuardEntry> {
        self.global_grpc_guards.clone()
    }

    pub fn add_global_grpc_interceptor(&mut self, interceptor: GrpcInterceptorEntry) {
        self.global_grpc_interceptors.push(interceptor);
    }

    pub fn get_global_grpc_interceptors(&self) -> Vec<GrpcInterceptorEntry> {
        self.global_grpc_interceptors.clone()
    }

    pub fn add_global_grpc_error_handler(&mut self, handler: GrpcErrorHandlerArc) {
        self.global_grpc_error_handlers.push(handler);
    }

    pub fn get_global_grpc_error_handlers(&self) -> Vec<GrpcErrorHandlerArc> {
        self.global_grpc_error_handlers.clone()
    }

    pub fn add_global_error_observer(&mut self, observer: Arc<dyn ErrorObserver>) {
        self.global_error_observers.push(observer);
    }

    pub fn get_global_error_observers(&self) -> Vec<Arc<dyn ErrorObserver>> {
        self.global_error_observers.clone()
    }

    pub fn get_global_enhancers(&self) -> EnhancerMetadata {
        EnhancerMetadata {
            guards: self.global_http_guards.clone(),
            interceptors: self.global_http_interceptors.clone(),
            error_handlers: self.global_http_error_handlers.clone(),
        }
    }

    pub fn add_module(&mut self, module_metadata: Box<dyn ModuleMetadata>) -> Result<()> {
        let token: String = module_metadata.get_id();
        // The token is the full identity (type name for static modules, base + config fingerprint
        // for dynamic ones). A repeat is the same module reached through a second import path — a
        // diamond — so dedup instead of overwriting. Distinct modules that resolve to the same
        // exported provider token are caught later, at global-provider registration.
        if self.modules.contains_key(&token) {
            return Ok(());
        }
        let name: String = module_metadata.get_name();
        let module = Module::new(&token, &name, module_metadata);
        self.modules.insert(token, module);
        Ok(())
    }

    pub fn add_import(
        &mut self,
        module_ref_token: &String,
        imported_module_token: String,
    ) -> Result<()> {
        let module_ref = self
            .modules
            .get_mut(module_ref_token)
            .ok_or_else(|| anyhow!("Module not found"))?;
        module_ref.add_import(imported_module_token);
        Ok(())
    }

    pub fn add_controller(
        &mut self,
        module_ref_token: &String,
        controller: Box<dyn ControllerFactory>,
    ) -> Result<()> {
        let module_ref = self
            .modules
            .get_mut(module_ref_token)
            .ok_or_else(|| anyhow!("Module not found"))?;
        module_ref.add_controller(controller);
        Ok(())
    }

    pub fn add_provider(
        &mut self,
        module_ref_token: &String,
        provider: Box<dyn ProviderFactory>,
    ) -> Result<()> {
        let module_ref = self
            .modules
            .get_mut(module_ref_token)
            .ok_or_else(|| anyhow!("Module not found"))?;
        module_ref.add_provider(provider);
        Ok(())
    }

    pub fn add_provider_instance(
        &mut self,
        module_ref_token: &String,
        provider_instance: Arc<Box<dyn Provider>>,
        roles: Vec<ProviderRole>,
    ) -> Result<()> {
        let token = provider_instance.get_token();

        for role in roles {
            match role {
                ProviderRole::HttpGuard(g) => {
                    self.role_registry.http_guards.insert(token.clone(), g);
                }
                ProviderRole::HttpInterceptor(i) => {
                    self.role_registry
                        .http_interceptors
                        .insert(token.clone(), i);
                }
                ProviderRole::HttpErrorHandler(eh) => {
                    self.role_registry
                        .http_error_handlers
                        .insert(token.clone(), eh);
                }
                ProviderRole::RpcGuard(g) => {
                    self.role_registry.rpc_guards.insert(token.clone(), g);
                }
                ProviderRole::RpcInterceptor(i) => {
                    self.role_registry.rpc_interceptors.insert(token.clone(), i);
                }
                ProviderRole::RpcErrorHandler(eh) => {
                    self.role_registry
                        .rpc_error_handlers
                        .insert(token.clone(), eh);
                }
                ProviderRole::WsGuard(g) => {
                    self.role_registry.ws_guards.insert(token.clone(), g);
                }
                ProviderRole::WsInterceptor(i) => {
                    self.role_registry.ws_interceptors.insert(token.clone(), i);
                }
                ProviderRole::WsErrorHandler(eh) => {
                    self.role_registry
                        .ws_error_handlers
                        .insert(token.clone(), eh);
                }
                ProviderRole::GrpcGuard(g) => {
                    self.role_registry.grpc_guards.insert(token.clone(), g);
                }
                ProviderRole::GrpcInterceptor(i) => {
                    self.role_registry
                        .grpc_interceptors
                        .insert(token.clone(), i);
                }
                ProviderRole::GrpcErrorHandler(eh) => {
                    self.role_registry
                        .grpc_error_handlers
                        .insert(token.clone(), eh);
                }
                ProviderRole::Middleware(m) => {
                    self.role_registry.middleware.insert(token.clone(), m);
                }
                ProviderRole::Gateway(gw) => {
                    let path = gw.get_path();
                    self.role_registry.gateways.insert(path, gw);
                }
            }
        }

        let module_ref = self
            .modules
            .get_mut(module_ref_token)
            .ok_or_else(|| anyhow!("Module not found"))?;
        module_ref.add_provider_instance(provider_instance);
        Ok(())
    }

    /// Every provider instance a module's lifecycle hooks must reach. Controllers are held
    /// separately and iterated beside these.
    pub fn get_lifecycle_instances(
        &self,
        module_ref_token: &String,
    ) -> Result<Vec<&Arc<Box<dyn Provider>>>> {
        let module_ref = self
            .modules
            .get(module_ref_token)
            .ok_or_else(|| anyhow!("Module not found"))?;
        Ok(module_ref.get_providers_instances().values().collect())
    }

    /// Register an RPC controller's source under its own token. Called from the controller path,
    /// which is where every dispatch target is declared.
    pub(crate) fn add_rpc_controller_source(
        &mut self,
        source: Arc<dyn crate::rpc::RpcControllerSource>,
    ) {
        self.role_registry
            .rpc_controllers
            .insert(source.get_token(), source);
    }

    /// Register a gRPC service's source under its own token.
    pub(crate) fn add_grpc_service_source(
        &mut self,
        source: Arc<dyn crate::adapter::GrpcServiceSource>,
    ) {
        self.role_registry
            .grpc_services
            .insert(source.token(), source);
    }

    pub(crate) fn get_role_registry(&self) -> &RoleRegistry {
        &self.role_registry
    }

    pub(crate) fn get_provider_roles(
        &self,
        token: &str,
    ) -> Vec<crate::traits_helpers::ProviderRole> {
        self.role_registry.get_roles_for_token(token)
    }

    pub fn get_gateways(&self) -> &FxHashMap<String, Arc<Box<dyn GatewayTrait>>> {
        &self.role_registry.gateways
    }

    pub fn get_rpc_controllers(&self) -> &FxHashMap<String, Arc<dyn RpcControllerSource>> {
        &self.role_registry.rpc_controllers
    }

    pub fn get_grpc_services(
        &self,
    ) -> &FxHashMap<String, Arc<dyn crate::adapter::GrpcServiceSource>> {
        &self.role_registry.grpc_services
    }

    /// Resolve middleware tokens for one module against the role registry.
    ///
    /// Called from the instance loader after all providers are instantiated so
    /// that the registry is fully populated before middleware is resolved.
    pub fn resolve_module_middleware(&mut self, module_token: &str) -> Result<()> {
        if let Some(manager) = self.middleware_manager.as_mut() {
            manager.resolve_middleware_tokens(module_token, &self.role_registry.middleware)?;
        }
        Ok(())
    }

    pub fn add_controller_object(
        &mut self,
        module_ref_token: &String,
        controller: Arc<dyn Controller>,
    ) -> Result<()> {
        let module_ref = self
            .modules
            .get_mut(module_ref_token)
            .ok_or_else(|| anyhow!("Module not found"))?;
        module_ref.add_controller_object(controller);
        Ok(())
    }

    pub fn add_route_instance(
        &mut self,
        module_ref_token: &String,
        controller_token: &str,
        route: Arc<dyn crate::traits_helpers::Route>,
        enhancer_metadata: EnhancerMetadata,
    ) -> Result<()> {
        let global_enhancers = self.get_global_enhancers();
        let error_observers = self.get_global_error_observers();
        let module_ref = self
            .modules
            .get_mut(module_ref_token)
            .ok_or_else(|| anyhow!("Module not found"))?;
        module_ref.add_route_instance(
            controller_token,
            route,
            enhancer_metadata,
            global_enhancers,
            error_observers,
        );
        Ok(())
    }

    pub fn add_export(&mut self, module_ref_token: &String, provider_token: String) -> Result<()> {
        let module_ref = self
            .modules
            .get_mut(module_ref_token)
            .ok_or_else(|| anyhow!("Module not found"))?;
        module_ref.add_export(provider_token);
        Ok(())
    }

    pub fn add_export_instance(
        &mut self,
        module_ref_token: &String,
        provider_token: String,
    ) -> Result<()> {
        let module_ref = self
            .modules
            .get_mut(module_ref_token)
            .ok_or_else(|| anyhow!("Module not found"))?;
        module_ref.add_export_instance(provider_token);
        Ok(())
    }

    pub fn get_providers_factory(
        &self,
        module_ref_token: &String,
    ) -> Result<&FxHashMap<String, Box<dyn ProviderFactory>>> {
        let module_ref = self
            .modules
            .get(module_ref_token)
            .ok_or_else(|| anyhow!("Module not found"))?;
        Ok(module_ref.get_providers_factory())
    }

    pub fn get_controllers_factory(
        &self,
        module_ref_token: &String,
    ) -> Result<&FxHashMap<String, Box<dyn ControllerFactory>>> {
        let module_ref = self
            .modules
            .get(module_ref_token)
            .ok_or_else(|| anyhow!("Module not found"))?;
        Ok(module_ref.get_controllers_factory())
    }

    pub fn get_providers_instance(
        &self,
        module_ref_token: &String,
    ) -> Result<&FxHashMap<String, Arc<Box<dyn Provider>>>> {
        let module_ref = self
            .modules
            .get(module_ref_token)
            .ok_or_else(|| anyhow!("Module not found"))?;
        Ok(module_ref.get_providers_instances())
    }

    pub fn get_provider_instance_by_token(
        &self,
        module_ref_token: &String,
        provider_token: &String,
    ) -> Result<Option<&Arc<Box<dyn Provider>>>> {
        let module_ref = self
            .modules
            .get(module_ref_token)
            .ok_or_else(|| anyhow!("Module not found"))?;
        Ok(module_ref.get_provider_instance_by_token(provider_token))
    }

    pub fn get_provider_by_token(
        &self,
        module_ref_token: &String,
        provider_token: &String,
    ) -> Result<Option<&dyn ProviderFactory>> {
        let module_ref = self
            .modules
            .get(module_ref_token)
            .ok_or_else(|| anyhow!("Module not found"))?;
        Ok(module_ref.get_provider_by_token(provider_token))
    }

    pub fn get_controllers_instance(
        &mut self,
        module_ref_token: &String,
    ) -> Result<Drain<'_, String, Arc<InstanceWrapper>>> {
        let module_ref = self
            .modules
            .get_mut(module_ref_token)
            .ok_or_else(|| anyhow!("Module not found"))?;
        Ok(module_ref.drain_controllers_instances())
    }

    pub fn get_imported_modules(&self, module_ref_token: &String) -> Result<&FxHashSet<String>> {
        let module_ref = self
            .modules
            .get(module_ref_token)
            .ok_or_else(|| anyhow!("Module not found"))?;
        Ok(module_ref.get_imported_modules())
    }

    pub fn get_exports_instances_tokens(
        &self,
        module_ref_token: &String,
    ) -> Result<&FxHashSet<String>> {
        let module_ref = self
            .modules
            .get(module_ref_token)
            .ok_or_else(|| anyhow!("Module not found: {:?}", module_ref_token))?;
        Ok(module_ref.get_exports_instances_tokens())
    }

    pub fn get_exports_tokens_vec(&self, module_ref_token: &String) -> Result<Vec<String>> {
        let module_ref = self
            .modules
            .get(module_ref_token)
            .ok_or_else(|| anyhow!("Module not found: {:?}", module_ref_token))?;
        Ok(module_ref.get_exports_tokens().iter().cloned().collect())
    }

    pub fn get_modules_token(&self) -> Vec<String> {
        self.modules.keys().cloned().collect::<Vec<String>>()
    }

    pub fn get_ordered_modules_token(&self) -> Vec<String> {
        let mut ordered_modules: Vec<String> = Vec::new();
        let mut visited: FxHashMap<String, bool> = FxHashMap::default();

        // Standard topological sort based on explicit imports
        while ordered_modules.len() < self.modules.len() {
            let mut ready_modules: Vec<String> = Vec::new();

            for (token, module) in self.modules.iter() {
                if visited.contains_key(token) {
                    continue;
                }

                let imported_modules = module.get_imported_modules();
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

    pub fn get_module_by_token(&self, module_ref_token: &String) -> Option<&Module> {
        self.modules.get(module_ref_token)
    }

    /// Register all exported providers from a global module into the global registry
    pub fn register_global_providers(&mut self, module_token: &String) -> Result<()> {
        let (is_global, module_name, exports_tokens) = {
            let module = self
                .modules
                .get(module_token)
                .ok_or_else(|| anyhow!("Module not found: {}", module_token))?;
            (
                module.get_metadata().is_global(),
                module.get_metadata().get_name(),
                module.get_exports_instances_tokens().clone(),
            )
        };

        // Only register if module is marked as global
        if !is_global {
            return Ok(());
        }

        // Register all exported providers as globally accessible
        for export_token in exports_tokens.iter() {
            // Two distinct global modules exporting the same token is the shape of the
            // "two connections, no names" mistake: both provide `DatabaseConnection`, and one
            // would silently shadow the other at injection. Refuse it and point at the fix. The
            // owner is keyed by identity, not display name — two dynamic modules with different
            // config share a name but are genuinely different modules.
            if let Some((owner_token, owner_name)) = self.global_provider_sources.get(export_token)
            {
                if owner_token != module_token {
                    return Err(anyhow!(
                        "provider '{export_token}' is exported globally by two modules \
                         ('{owner_name}' and '{module_name}'). One would silently shadow the other. \
                         If these are separate instances of the same integration, register each \
                         under a distinct name — integrations expose a named constructor for this \
                         (e.g. `for_root_named`) — and inject it with `#[inject(\"<name>\")]`."
                    ));
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

    /// Get a provider from the global registry
    pub fn get_global_provider(&self, token: &String) -> Option<Arc<Box<dyn Provider>>> {
        self.global_providers.get(token).cloned()
    }

    /// Register a provider token as globally available (during scan phase)
    pub fn register_global_provider_token(&mut self, token: String) {
        self.global_provider_tokens.insert(token);
    }

    /// Check if a provider token is registered as globally available
    pub fn is_global_provider_token(&self, token: &String) -> bool {
        self.global_provider_tokens.contains(token)
    }

    // pub fn register_controller_enhancers(
    //     &mut self,
    //     module_ref_token: &String,
    //     controller_token: &String,
    //     controller_enhancers: &Vec<Box<dyn ControllerEnhancer>>,
    // ) -> Result<()> {
    //     let module_ref = self
    //         .modules
    //         .get_mut(module_ref_token)
    //         .ok_or_else(|| anyhow!("Module not found"))?;
    //     module_ref.register_controller_enhancers(controller_enhancers);
    //     Ok(())
    // }

    pub fn get_middleware_manager(&self) -> Option<&MiddlewareManager> {
        self.middleware_manager.as_ref()
    }

    pub fn get_middleware_manager_mut(&mut self) -> Option<&mut MiddlewareManager> {
        self.middleware_manager.as_mut()
    }

    /// Register a provider with APP_GUARD token (during scan phase)
    pub fn register_app_guard_provider(&mut self, module_token: String, provider_token: String) {
        self.app_guard_providers
            .push((module_token, provider_token));
    }

    /// Register a provider with APP_INTERCEPTOR token (during scan phase)
    pub fn register_app_interceptor_provider(
        &mut self,
        module_token: String,
        provider_token: String,
    ) {
        self.app_interceptor_providers
            .push((module_token, provider_token));
    }

    /// Get all APP_GUARD providers (after instances are created)
    pub fn get_app_guard_providers(&self) -> &[(String, String)] {
        &self.app_guard_providers
    }

    /// Get all APP_INTERCEPTOR providers (after instances are created)
    pub fn get_app_interceptor_providers(&self) -> &[(String, String)] {
        &self.app_interceptor_providers
    }

    /// Register one multi-provider contribution during the scan phase.
    pub fn register_multi_provider(
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

    pub fn get_multi_providers(&self) -> &FxHashMap<String, Vec<(String, String)>> {
        &self.multi_providers
    }

    pub fn add_multi_collection_provider(
        &mut self,
        base_token: String,
        instance: Arc<Box<dyn Provider>>,
    ) {
        self.multi_collection_providers.insert(base_token, instance);
    }

    pub fn get_multi_collection_provider(
        &self,
        base_token: &str,
    ) -> Option<Arc<Box<dyn Provider>>> {
        self.multi_collection_providers.get(base_token).cloned()
    }
}
