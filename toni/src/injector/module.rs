use std::{collections::hash_map::Drain, sync::Arc};

use rustc_hash::{FxHashMap, FxHashSet};

use super::InstanceWrapper;

use crate::{
    structs_helpers::EnhancerMetadata,
    traits_helpers::{
        Controller, ControllerFactory, ModuleMetadata, Provider, ProviderFactory, Route,
    },
};
pub struct Module {
    _token: String,
    _name: String,
    controllers: FxHashMap<String, Box<dyn ControllerFactory>>,
    providers: FxHashMap<String, Box<dyn ProviderFactory>>,
    imports: FxHashSet<String>,
    exports: FxHashSet<String>,
    /// One per route, the dispatch units the router registers with the adapter.
    controllers_instances: FxHashMap<String, Arc<InstanceWrapper>>,
    /// One per controller struct, kept for lifecycle hooks (fired once each).
    controller_objects: Vec<Arc<dyn Controller>>,
    providers_instances: FxHashMap<String, Arc<Box<dyn Provider>>>,
    /// Instances reached only by dispatch — RPC controllers. Kept for lifecycle hooks the way
    /// `controller_objects` is, and deliberately absent from `providers_instances` so nothing
    /// resolves one as a dependency.
    dispatch_targets: Vec<Arc<Box<dyn Provider>>>,
    exports_instances: FxHashSet<String>,
    metadata: Box<dyn ModuleMetadata>,
}

impl Module {
    pub fn new(token: &str, name: &str, metadata: Box<dyn ModuleMetadata>) -> Self {
        Self {
            _token: token.to_owned(),
            _name: name.to_string(),
            controllers: FxHashMap::default(),
            providers: FxHashMap::default(),
            imports: FxHashSet::default(),
            exports: FxHashSet::default(),
            controllers_instances: FxHashMap::default(),
            controller_objects: Vec::new(),
            providers_instances: FxHashMap::default(),
            dispatch_targets: Vec::new(),
            exports_instances: FxHashSet::default(),
            metadata,
        }
    }
}
impl Module {
    pub fn add_controller(&mut self, controller: Box<dyn ControllerFactory>) {
        self.controllers.insert(controller.get_token(), controller);
    }

    pub fn add_provider(&mut self, provider: Box<dyn ProviderFactory>) {
        self.providers.insert(provider.get_token(), provider);
    }

    pub fn add_import(&mut self, module_token: String) {
        self.imports.insert(module_token);
    }

    pub fn add_export(&mut self, provider_token: String) {
        self.exports.insert(provider_token);
    }

    /// Keep the controller instance for lifecycle-hook dispatch (one per struct).
    pub fn add_controller_object(&mut self, controller: Arc<dyn Controller>) {
        self.controller_objects.push(controller);
    }

    /// Register one route's dispatch unit. Keyed per controller + method + path so
    /// routes from different controllers never collide in the map.
    pub fn add_route_instance(
        &mut self,
        controller_token: &str,
        route: Arc<dyn Route>,
        enhancer_metadata: EnhancerMetadata,
        global_enhancers: EnhancerMetadata,
        error_observers: Vec<Arc<dyn crate::traits_helpers::ErrorObserver>>,
    ) {
        let key = format!(
            "{}::{} {}",
            controller_token,
            route.get_method().as_str(),
            route.get_path()
        );
        let instance_wrapper =
            InstanceWrapper::new(route, enhancer_metadata, global_enhancers, error_observers);
        self.controllers_instances
            .insert(key, Arc::new(instance_wrapper));
    }

    pub fn add_provider_instance(&mut self, provider: Arc<Box<dyn Provider>>) {
        self.providers_instances
            .insert(provider.get_token(), provider);
    }
    /// Keep a dispatch target's instance for lifecycle-hook dispatch, out of dependency resolution.
    pub fn add_dispatch_target(&mut self, provider: Arc<Box<dyn Provider>>) {
        self.dispatch_targets.push(provider);
    }

    pub fn get_dispatch_targets(&self) -> &[Arc<Box<dyn Provider>>] {
        &self.dispatch_targets
    }

    pub fn add_export_instance(&mut self, provider_token: String) {
        self.exports_instances.insert(provider_token);
    }

    pub fn get_providers_factory(&self) -> &FxHashMap<String, Box<dyn ProviderFactory>> {
        &self.providers
    }

    pub fn get_providers_instances(&self) -> &FxHashMap<String, Arc<Box<dyn Provider>>> {
        &self.providers_instances
    }

    pub fn get_provider_by_token(&self, provider_token: &String) -> Option<&dyn ProviderFactory> {
        self.providers
            .get(provider_token)
            .map(|provider| provider.as_ref())
    }

    pub fn get_provider_instance_by_token(
        &self,
        provider_token: &String,
    ) -> Option<&Arc<Box<dyn Provider>>> {
        self.providers_instances.get(provider_token)
    }

    pub fn get_controllers_factory(&self) -> &FxHashMap<String, Box<dyn ControllerFactory>> {
        &self.controllers
    }

    pub fn drain_controllers_instances(&mut self) -> Drain<'_, String, Arc<InstanceWrapper>> {
        self.controllers_instances.drain()
    }

    pub fn get_imported_modules(&self) -> &FxHashSet<String> {
        &self.imports
    }

    pub fn get_exports_instances_tokens(&self) -> &FxHashSet<String> {
        &self.exports_instances
    }

    pub fn get_exports_tokens(&self) -> &FxHashSet<String> {
        &self.exports
    }

    pub fn get_metadata(&self) -> &dyn ModuleMetadata {
        &*self.metadata
    }

    pub fn _get_name(&self) -> &String {
        &self._name
    }

    pub fn _get_token(&self) -> &String {
        &self._token
    }

    pub fn _get_controller_by_token(
        &self,
        controller_token: &String,
    ) -> Option<&dyn ControllerFactory> {
        self.controllers
            .get(controller_token)
            .map(|controller| controller.as_ref())
    }

    pub fn _get_controllers_instances(&self) -> &FxHashMap<String, Arc<InstanceWrapper>> {
        &self.controllers_instances
    }

    pub fn _take_controllers_instances(&mut self) -> FxHashMap<String, Arc<InstanceWrapper>> {
        std::mem::take(&mut self.controllers_instances)
    }

    /// The controller instances, one per struct, for lifecycle-hook dispatch.
    pub fn get_controller_objects(&self) -> &[Arc<dyn Controller>] {
        &self.controller_objects
    }
}
