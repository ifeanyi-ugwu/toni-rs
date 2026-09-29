use crate::dispatch::transport::{EnhancerSet, Http};
use std::{collections::hash_map::Drain, sync::Arc};

use rustc_hash::{FxHashMap, FxHashSet};

use crate::http::RoutePipeline;

use super::FxIndexMap;

use crate::{
    di::ModuleMetadata,
    dispatch::{Controller, ControllerFactory},
    http::Route,
    spi::{Provider, ProviderFactory},
};
pub struct Module {
    controllers: FxIndexMap<String, Arc<dyn ControllerFactory>>,
    providers: FxIndexMap<String, Arc<dyn ProviderFactory>>,
    imports: FxHashSet<String>,
    exports: FxHashSet<String>,
    /// One per route, the dispatch units the router registers with the adapter.
    controllers_instances: FxHashMap<String, Arc<RoutePipeline>>,
    /// One per controller struct, kept for lifecycle hooks (fired once each).
    controller_objects: Vec<Arc<dyn Controller>>,
    providers_instances: FxIndexMap<String, Arc<Box<dyn Provider>>>,
    exports_instances: FxHashSet<String>,
    metadata: Arc<dyn ModuleMetadata>,
}

impl Module {
    pub fn new(metadata: Box<dyn ModuleMetadata>) -> Self {
        let metadata: Arc<dyn ModuleMetadata> = Arc::from(metadata);
        Self {
            controllers: FxIndexMap::default(),
            providers: FxIndexMap::default(),
            imports: FxHashSet::default(),
            exports: FxHashSet::default(),
            controllers_instances: FxHashMap::default(),
            controller_objects: Vec::new(),
            providers_instances: FxIndexMap::default(),
            exports_instances: FxHashSet::default(),
            metadata,
        }
    }
}
impl Module {
    pub fn add_controller(&mut self, controller: Box<dyn ControllerFactory>) {
        self.controllers
            .insert(controller.token(), Arc::from(controller));
    }

    pub fn add_provider(&mut self, provider: Box<dyn ProviderFactory>) {
        self.providers.insert(provider.token(), Arc::from(provider));
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
    pub(crate) fn add_route_instance(
        &mut self,
        controller_token: &str,
        route: Arc<dyn Route>,
        enhancers: EnhancerSet<Http>,
    ) {
        let key = format!(
            "{}::{} {}",
            controller_token,
            route.method().as_str(),
            route.path()
        );
        let instance_wrapper = RoutePipeline::new(route, enhancers);
        self.controllers_instances
            .insert(key, Arc::new(instance_wrapper));
    }

    pub fn add_provider_instance(&mut self, provider: Arc<Box<dyn Provider>>) {
        self.providers_instances.insert(provider.token(), provider);
    }
    pub fn add_export_instance(&mut self, provider_token: String) {
        self.exports_instances.insert(provider_token);
    }

    pub fn provider_factories(&self) -> &FxIndexMap<String, Arc<dyn ProviderFactory>> {
        &self.providers
    }

    pub fn provider_instances(&self) -> &FxIndexMap<String, Arc<Box<dyn Provider>>> {
        &self.providers_instances
    }

    /// The factory for `provider_token`, as a handle rather than a borrow.
    ///
    /// Cloned out so a caller can await [`ProviderFactory::build`] without holding the
    /// container lock across it.
    pub fn get_provider_by_token(
        &self,
        provider_token: &String,
    ) -> Option<Arc<dyn ProviderFactory>> {
        self.providers.get(provider_token).map(Arc::clone)
    }

    pub fn get_provider_instance_by_token(
        &self,
        provider_token: &String,
    ) -> Option<&Arc<Box<dyn Provider>>> {
        self.providers_instances.get(provider_token)
    }

    pub fn controller_factories(&self) -> &FxIndexMap<String, Arc<dyn ControllerFactory>> {
        &self.controllers
    }

    pub(crate) fn drain_controllers_instances(&mut self) -> Drain<'_, String, Arc<RoutePipeline>> {
        self.controllers_instances.drain()
    }

    pub fn imported_modules(&self) -> &FxHashSet<String> {
        &self.imports
    }

    pub fn exported_instance_tokens(&self) -> &FxHashSet<String> {
        &self.exports_instances
    }

    pub fn exported_tokens(&self) -> &FxHashSet<String> {
        &self.exports
    }

    /// The module's metadata, as a handle rather than a borrow.
    ///
    /// Cloned out so a caller can await one of its lifecycle hooks without holding the
    /// container lock across the await.
    pub fn metadata(&self) -> Arc<dyn ModuleMetadata> {
        Arc::clone(&self.metadata)
    }

    pub fn _get_controller_by_token(
        &self,
        controller_token: &String,
    ) -> Option<&dyn ControllerFactory> {
        self.controllers
            .get(controller_token)
            .map(|controller| controller.as_ref())
    }

    /// The controller instances, one per struct, for lifecycle-hook dispatch.
    pub fn controller_objects(&self) -> &[Arc<dyn Controller>] {
        &self.controller_objects
    }
}
