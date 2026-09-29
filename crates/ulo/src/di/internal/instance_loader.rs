use crate::dispatch::ControllerFactory;
use crate::dispatch::transport::{EnhancerSet, Http};
use crate::error::SetupResult;
use rustc_hash::FxHashMap;
use std::{any::Any, sync::Arc};

use parking_lot::RwLock;

/// Why one module's providers could not all be built on a given pass.
///
/// Instantiation runs in passes because a provider can depend on a global one that a
/// later module in the order contributes. [`Deferred`] is that wait, and it is the
/// expected answer on an early pass rather than a failure: the loader puts the module
/// back on the pending list and tries again once another module has made progress. A
/// pass where nothing succeeds turns the collected reasons into the stall diagnostic.
///
/// [`Deferred`]: LoadError::Deferred
enum LoadError {
    /// A dependency is declared and not yet built. Carries what was waited on.
    Deferred(String),
    /// The module cannot be built on this pass or any later one.
    Failed(Box<dyn std::error::Error + Send + Sync + 'static>),
}

impl From<String> for LoadError {
    fn from(message: String) -> Self {
        Self::Failed(message.into())
    }
}

impl From<Box<dyn std::error::Error + Send + Sync + 'static>> for LoadError {
    fn from(source: Box<dyn std::error::Error + Send + Sync + 'static>) -> Self {
        Self::Failed(source)
    }
}

impl From<LoadError> for Box<dyn std::error::Error + Send + Sync + 'static> {
    fn from(e: LoadError) -> Self {
        match e {
            LoadError::Deferred(reason) => reason.into(),
            LoadError::Failed(source) => source,
        }
    }
}

type LoadResult<T> = std::result::Result<T, LoadError>;

use super::{
    Container, DependencyGraph, FxIndexMap, find_dependency_cycle,
    multi_collection_provider::MultiCollectionProvider,
};
use crate::{
    dispatch::{Controller, Targets},
    http::Route,
    spi::{Injectable, Provider},
};

pub(crate) struct InstanceLoader {
    container: Arc<RwLock<Container>>,
}

impl InstanceLoader {
    pub(crate) fn new(container: Arc<RwLock<Container>>) -> Self {
        Self { container }
    }

    pub(crate) async fn create_instances_of_dependencies(&self) -> SetupResult {
        let modules_order = self.container.read().ordered_module_tokens();

        // PRE-PHASE 1: Register one ModuleRefProvider per module, all sharing the same
        // store Arc. The store is empty now; it gets written after Phase 1 completes.
        let store_arc: Arc<RwLock<super::module_ref::ProviderStore>> =
            Arc::new(RwLock::new(super::module_ref::ProviderStore::default()));
        for module_token in &modules_order {
            let provider: Arc<Box<dyn Provider>> = Arc::new(Box::new(
                super::module_ref_provider::ModuleRefProvider::new(
                    module_token.clone(),
                    store_arc.clone(),
                ),
            ));
            self.container
                .write()
                .add_provider_instance(module_token, provider, vec![])?;
        }

        // PHASE 1: Create provider instances for all modules (with deferred retry logic)
        tracing::debug!(
            total_modules = modules_order.len(),
            "DI phase 1: creating provider instances"
        );
        // Track which modules are pending (deferred due to unready global providers)
        let mut pending_modules: Vec<String> = modules_order.clone();
        let total_modules = pending_modules.len();
        let mut max_iterations = total_modules * 2; // Prevent infinite loops
        // Last deferral reason per module, kept to explain a stall precisely.
        let mut deferred_reasons: FxHashMap<String, String> = FxHashMap::default();

        while !pending_modules.is_empty() && max_iterations > 0 {
            max_iterations -= 1;
            let mut successfully_created = Vec::new();
            let mut deferred_modules = Vec::new();

            for module_token in &pending_modules {
                match self
                    .create_instances_of_providers(module_token.clone())
                    .await
                {
                    Ok(_) => {
                        // Module providers created successfully - register its global providers
                        let mut container = self.container.write();
                        container.register_global_providers(module_token)?;
                        container.record_construction(module_token);
                        successfully_created.push(module_token.clone());
                    }
                    Err(LoadError::Deferred(reason)) => {
                        deferred_reasons.insert(module_token.clone(), reason);
                        deferred_modules.push(module_token.clone());
                        continue;
                    }
                    Err(LoadError::Failed(source)) => {
                        return Err(source);
                    }
                }
            }

            if successfully_created.is_empty() && !pending_modules.is_empty() {
                // No progress this pass: the remaining modules form a dependency cycle
                // that spans modules, or wait on a provider that is never produced.
                let diagnostic =
                    self.diagnose_unresolved_modules(&pending_modules, &deferred_reasons);
                return Err(diagnostic.into());
            }

            // Update pending list to only deferred modules
            pending_modules = deferred_modules;
        }

        if !pending_modules.is_empty() {
            return Err(format!(
                "Module instantiation timed out. Remaining modules: {:?}",
                pending_modules
            )
            .into());
        }

        // PHASE 1.5: Collect multi-provider contributions into Vec collections per base token.
        // Must run after all individual providers are built so as_multi_item() is available.
        tracing::debug!("DI phase 1.5: collecting multi-providers");
        self.collect_multi_providers()?;

        // PHASE 1.6: Populate the shared store now that all providers exist.
        // One write into store_arc; every ModuleRef in the app sees it immediately.
        {
            let container = self.container.read();
            let mut store = store_arc.write();
            store.globals = container.global_providers().clone();
            for module_token in &modules_order {
                if let Ok(instances) = container.get_provider_instances(module_token) {
                    let instances = instances
                        .iter()
                        .map(|(token, instance)| (token.clone(), instance.clone()))
                        .collect();
                    store.modules.insert(module_token.clone(), instances);
                }
                // Every import declaring the export counts, built or not, as it does where the
                // loader resolves an injection.
                let mut imported: super::module_ref::ImportedExports = FxHashMap::default();
                for import in container.imported_modules(module_token)? {
                    for token in container.exported_tokens_of(import)? {
                        let instance = container
                            .get_provider_instance_by_token(import, &token)?
                            .cloned();
                        imported
                            .entry(token)
                            .or_default()
                            .push((import.clone(), instance));
                    }
                }
                store.imports.insert(module_token.clone(), imported);
            }
        }

        // PHASE 2: the HTTP guard and interceptor collections become HTTP's global enhancers,
        // after every provider is built, so they have their dependencies, and before any controller
        // is, so the controllers see them.
        tracing::debug!("DI phase 2: resolving the HTTP global collections");
        self.resolve_app_token_enhancers()?;

        // PHASE 3: Resolve middleware tokens from DI container
        // This happens AFTER DI container is built, allowing middleware to have injected dependencies
        tracing::debug!("DI phase 3: resolving middleware tokens");
        self.resolve_middleware_tokens(&modules_order)?;

        // PHASE 4: Create controller instances now that global enhancers are registered
        tracing::debug!("DI phase 4: creating controller instances");
        for module_token in &modules_order {
            self.create_instances_of_controllers(module_token.clone())
                .await?;
        }

        Ok(())
    }

    /// Collect multi-provider contributions into MultiCollectionProvider instances.
    ///
    /// Iterates all registered multi-provider groups (base_token -> contributions), calls
    /// as_multi_item() on each built contribution, and stores the resulting collection
    /// in the container so it can be resolved like any other provider dependency.
    fn collect_multi_providers(&self) -> SetupResult {
        let multi_map = self.container.read().multi_providers().clone();

        for (base_token, contributions) in multi_map {
            let mut items: Vec<Arc<dyn Any + Send + Sync>> = Vec::new();

            for (module_token, provider_token) in &contributions {
                let container = self.container.read();
                let provider = container
                    .get_provider_instance_by_token(module_token, provider_token)?
                    .ok_or_else(|| {
                        format!(
                            "Multi-provider contribution '{}' not found in module '{}'",
                            provider_token, module_token
                        )
                    })?
                    .clone();

                let item = provider.as_multi_item().ok_or_else(|| {
                    format!(
                        "Provider '{}' is registered as multi but does not implement as_multi_item()",
                        provider_token
                    )
                })?;
                items.push(item);
            }

            let collection: Arc<Box<dyn Provider>> = Arc::new(Box::new(MultiCollectionProvider {
                token: base_token.clone(),
                items,
            }));
            self.container
                .write()
                .add_multi_collection_provider(base_token, collection);
        }

        Ok(())
    }

    /// The contributions to the HTTP guard and interceptor collections, as HTTP's global
    /// enhancers.
    fn resolve_app_token_enhancers(&self) -> SetupResult {
        let container = self.container.read();
        let app_guard_providers = container.app_guard_providers().to_vec();
        let app_interceptor_providers = container.app_interceptor_providers().to_vec();
        drop(container);

        for (_, provider_token) in app_guard_providers {
            let guard = self
                .container
                .read()
                .role_registry()
                .http
                .guards
                .get(&provider_token)
                .cloned()
                .ok_or_else(|| {
                    format!(
                        "Provider '{}' in the HTTP global guard collection does not implement Guard<HttpContext>",
                        provider_token
                    )
                })?;
            self.container.write().global_http.guards.push(guard);
        }

        for (_, provider_token) in app_interceptor_providers {
            let interceptor = self
                .container
                .read()
                .role_registry()
                .http.interceptors
                .get(&provider_token)
                .cloned()
                .ok_or_else(|| {
                    format!(
                        "Provider '{}' in the HTTP global interceptor collection does not implement Interceptor<HttpContext, HttpHandlerResult>",
                        provider_token
                    )
                })?;
            self.container
                .write()
                .global_http
                .interceptors
                .push(interceptor);
        }

        Ok(())
    }

    /// Resolve middleware tokens from the role registry
    fn resolve_middleware_tokens(&self, modules_order: &[String]) -> SetupResult {
        for module_token in modules_order {
            self.container
                .write()
                .resolve_module_middleware(module_token)?;
        }
        Ok(())
    }

    async fn create_instances_of_providers(&self, module_token: String) -> LoadResult<()> {
        let dependency_graph = DependencyGraph::new(self.container.clone(), module_token.clone());
        let ordered_providers_token = dependency_graph.ordered_provider_tokens()?;
        let provider_instances = {
            // In build order, which the module's instance map keeps and the hooks walk.
            let mut instances: FxIndexMap<String, Injectable> = FxIndexMap::default();

            for provider_token in ordered_providers_token {
                // The factory is taken as a handle and the lock released: `build` is
                // awaited, and a provider's constructor may itself resolve from the
                // container.
                let provider_factory = self
                    .container
                    .read()
                    .get_provider_by_token(&module_token, &provider_token)?
                    .ok_or_else(|| format!("Provider not found: {}", provider_token))?;

                let dependencies = provider_factory.dependency_tokens();
                let resolved_dependencies =
                    self.resolve_dependencies(&module_token, dependencies, Some(&instances))?;

                let injectable = provider_factory.build(resolved_dependencies).await;
                tracing::debug!(module = %module_token, provider = %injectable.instance.token(), "provider instantiated");
                let token = injectable.instance.token();
                instances.insert(token, injectable);
            }
            instances
        };
        self.add_providers_instances(&module_token, provider_instances)?;
        Ok(())
    }

    /// Build a provider-level dependency graph across every module in the container.
    ///
    /// Returns `(adjacency, token_module)`: `adjacency` maps a provider token to the
    /// provider tokens it depends on (multi-collection base tokens expanded to their
    /// contributors), and `token_module` records the declaring module of each token for
    /// diagnostics. Runs only on the failure path, so it walks the whole container.
    fn build_provider_dependency_graph(
        &self,
    ) -> (FxHashMap<String, Vec<String>>, FxHashMap<String, String>) {
        let container = self.container.read();
        let multi = container.multi_providers();
        let mut adjacency: FxHashMap<String, Vec<String>> = FxHashMap::default();
        let mut token_module: FxHashMap<String, String> = FxHashMap::default();

        for module_token in container.module_tokens() {
            let Ok(providers) = container.provider_factories(&module_token) else {
                continue;
            };
            for (token, factory) in providers.iter() {
                token_module
                    .entry(token.clone())
                    .or_insert_with(|| module_token.clone());
                let mut deps: Vec<String> = Vec::new();
                for dep in factory.dependency_tokens() {
                    match multi.get(&dep) {
                        // A multi-collection base token resolves to its contributors.
                        Some(contribs) => deps.extend(contribs.iter().map(|(_m, t)| t.clone())),
                        None => deps.push(dep),
                    }
                }
                adjacency.entry(token.clone()).or_default().extend(deps);
            }
        }

        (adjacency, token_module)
    }

    /// Explain why Phase 1 stalled: name the exact provider cycle when one exists,
    /// otherwise report each stuck module's unresolved dependency. The vague
    /// "missing global provider" fallback only applies when no cycle is found —
    /// a genuinely missing dependency already fails earlier, in `resolve_dependencies`.
    fn diagnose_unresolved_modules(
        &self,
        pending_modules: &[String],
        deferred_reasons: &FxHashMap<String, String>,
    ) -> String {
        let (adjacency, token_module) = self.build_provider_dependency_graph();

        if let Some(cycle) = find_dependency_cycle(&adjacency) {
            let chain = cycle
                .iter()
                .map(|token| match token_module.get(token) {
                    Some(module) => format!("{token} (in module {module})"),
                    None => token.clone(),
                })
                .collect::<Vec<_>>()
                .join("\n    -> ");
            return format!(
                "Circular dependency detected between providers:\n    {chain}\n\
                 A provider cannot be built before a provider it depends on. Break the cycle: \
                 extract the shared logic into a third provider both depend on, or inject \
                 `ModuleRef` into one side and resolve the other lazily at call time."
            );
        }

        let mut details = String::new();
        for module in pending_modules {
            match deferred_reasons.get(module) {
                Some(reason) => details.push_str(&format!("\n    - {module}: {reason}")),
                None => details.push_str(&format!("\n    - {module}")),
            }
        }
        format!(
            "Cannot resolve dependencies for modules: {pending_modules:?}. No provider cycle was \
             found, so a required provider is missing or not exported by an imported module:{details}"
        )
    }

    fn add_providers_instances(
        &self,
        module_token: &String,
        providers_instances: FxIndexMap<String, Injectable>,
    ) -> SetupResult {
        let mut container = self.container.write();
        let mut providers_tokens = Vec::new();
        for (provider_instance_token, injectable) in providers_instances {
            let token = injectable.instance.token().clone();
            container.add_provider_instance(module_token, injectable.instance, injectable.roles)?;
            providers_tokens.push((token, provider_instance_token));
        }

        self.resolve_exports(module_token, providers_tokens, container)?;
        Ok(())
    }

    fn resolve_exports(
        &self,
        module_token: &String,
        providers_tokens: Vec<(String, String)>,
        container: parking_lot::RwLockWriteGuard<'_, Container>,
    ) -> SetupResult {
        let exports = container.exported_tokens_of(module_token)?;
        self.add_export_instances_tokens(module_token, providers_tokens, exports, container)?;
        Ok(())
    }

    fn add_export_instances_tokens(
        &self,
        module_token: &String,
        providers_tokens: Vec<(String, String)>,
        exports: Vec<String>,
        mut container: parking_lot::RwLockWriteGuard<'_, Container>,
    ) -> SetupResult {
        for (provider_factory_token, provider_instance_token) in providers_tokens {
            if exports.contains(&provider_factory_token) {
                container.add_export_instance(module_token, provider_instance_token)?;
            }
        }
        Ok(())
    }

    async fn create_instances_of_controllers(&self, module_token: String) -> SetupResult {
        // Taken as handles and the lock released: `build` is awaited, and a controller's
        // constructor may itself resolve from the container.
        let factories: Vec<Arc<dyn ControllerFactory>> = {
            let container = self.container.read();
            container
                .controller_factories(&module_token)?
                .values()
                .map(Arc::clone)
                .collect()
        };

        let mut controllers_instances = Vec::new();
        for controller_factory in factories {
            let dependencies = controller_factory.dependency_tokens();
            let resolved_dependencies = self
                .resolve_dependencies(&module_token, dependencies, None)?
                .into_iter()
                .map(|(k, inj)| (k, inj.instance))
                .collect();
            controllers_instances.push(controller_factory.build(resolved_dependencies).await);
        }
        self.add_controllers_instances(module_token, controllers_instances)?;
        Ok(())
    }

    fn add_controllers_instances(
        &self,
        module_token: String,
        controllers: Vec<Arc<dyn Controller>>,
    ) -> SetupResult {
        // Phase A: expand each controller into its dispatch under an immutable borrow, resolving
        // every transport's enhancer tokens against the role registry — a misdeclared token fails
        // create(), whatever the transport.
        let rpc_resolver =
            crate::dispatch::resolve::RpcControllerResolver::new(self.container.clone());
        let grpc_resolver =
            crate::dispatch::resolve::GrpcServiceResolver::new(self.container.clone());
        type ResolvedController = (Arc<dyn Controller>, ResolvedDispatch);
        let resolved: Vec<ResolvedController> = controllers
            .into_iter()
            .map(|controller| {
                let dispatch = match controller.targets() {
                    Targets::Http(routes) => ResolvedDispatch::Http(
                        routes
                            .into_iter()
                            .map(|route| {
                                let meta = self.resolve_enhancers_from_tokens(&route)?;
                                Ok((route, meta))
                            })
                            .collect::<SetupResult<Vec<_>>>()?,
                    ),
                    Targets::Rpc(source) => {
                        ResolvedDispatch::Rpc(Arc::new(rpc_resolver.wrap_controller(source)?))
                    }
                    Targets::Grpc(source) => {
                        let enhancers = grpc_resolver.resolve_for(source.as_ref())?;
                        ResolvedDispatch::Grpc(source, Arc::new(enhancers))
                    }
                };
                Ok((controller, dispatch))
            })
            .collect::<SetupResult<_>>()?;

        // Phase B: store the controller (for lifecycle) and its dispatch units under a mutable
        // borrow.
        let mut container_mut = self.container.write();
        for (controller, dispatch) in resolved {
            let token = controller.token();
            container_mut.add_controller_object(&module_token, controller)?;
            match dispatch {
                ResolvedDispatch::Http(routes) => {
                    for (route, enhancer_metadata) in routes {
                        container_mut.add_route_instance(
                            &module_token,
                            &token,
                            route,
                            enhancer_metadata,
                        )?;
                    }
                }
                ResolvedDispatch::Rpc(wrapper) => {
                    container_mut.add_rpc_controller(token.clone(), wrapper)
                }
                ResolvedDispatch::Grpc(source, enhancers) => {
                    container_mut.add_grpc_service(token.clone(), (source, enhancers))
                }
            }
        }
        Ok(())
    }

    /// Resolve one route's declared enhancers, with the transport's globals ahead of them.
    ///
    /// A `#[controller]` yields one `Route` per handler method and each registers separately, so
    /// HTTP has one tier where the other three have two.
    fn resolve_enhancers_from_tokens(
        &self,
        route: &Arc<dyn Route>,
    ) -> SetupResult<EnhancerSet<Http>> {
        let declared = route.enhancers();
        let container = self.container.read();
        crate::dispatch::resolve::resolve_target::<Http>(
            &container.role_registry().http,
            &container.global_http,
            crate::dispatch::resolve::Declared {
                guards: declared.guards,
                interceptors: declared.interceptors,
                error_handlers: declared.error_handlers,
            },
        )
    }

    fn resolve_dependencies(
        &self,
        module_token: &String,
        dependencies: Vec<String>,
        providers_instances: Option<&FxIndexMap<String, Injectable>>,
    ) -> LoadResult<FxHashMap<String, Injectable>> {
        let container = self.container.read();
        let mut resolved_dependencies = FxHashMap::default();

        for dependency in dependencies {
            // Step 1: Check local providers (in-progress build map). A dispatch target is not
            // among them — it is declared in `controllers:` and never reaches the provider store —
            // so asking for one falls through to the not-found answer below.
            let built_locally = providers_instances.and_then(|m| m.get(&dependency));
            if let Some(injectable) = built_locally {
                resolved_dependencies.insert(dependency, injectable.clone());
            }
            // Step 1b: Check pre-registered container instances not yet in the build map
            // (e.g. ModuleRefProvider registered before Phase 1)
            else if let Ok(Some(instance)) =
                container.get_provider_instance_by_token(module_token, &dependency)
            {
                let roles = container.provider_roles(&dependency);
                resolved_dependencies.insert(dependency, Injectable::new(instance.clone(), roles));
            }
            // Step 2: Check imported modules
            else if let Some(exported_instance) =
                self.resolve_from_imported_modules(module_token, &dependency)?
            {
                tracing::debug!(module = %module_token, dependency = %dependency, source = "imported_module", "dependency resolved");
                let roles = container.provider_roles(&dependency);
                resolved_dependencies.insert(
                    dependency,
                    Injectable::new(exported_instance.clone(), roles),
                );
            }
            // Step 3: Check if it's a registered global provider token
            else if container.is_global_provider_token(&dependency) {
                if let Some(global_instance) = container.get_global_provider(&dependency) {
                    tracing::debug!(module = %module_token, dependency = %dependency, source = "global", "dependency resolved");
                    let roles = container.provider_roles(&dependency);
                    resolved_dependencies
                        .insert(dependency, Injectable::new(global_instance.clone(), roles));
                } else {
                    return Err(LoadError::Deferred(format!(
                        "global provider '{dependency}' is not instantiated yet"
                    )));
                }
            }
            // Step 3.5: Check cached multi-collection providers (assembled in Phase 1.5)
            else if let Some(multi_instance) =
                container.get_multi_collection_provider(&dependency)
            {
                resolved_dependencies.insert(dependency, Injectable::new(multi_instance, vec![]));
            }
            // Step 3.6: Assemble multi-collection on-demand when contributor and consumer
            // share the same module — contributors are in the in-progress instances map
            // before Phase 1.5 has had a chance to cache the collection.
            else if let Some(contribs) = container.multi_providers().get(&dependency).cloned() {
                let mut items: Vec<std::sync::Arc<dyn std::any::Any + Send + Sync>> = Vec::new();
                for (contrib_module_token, provider_token) in &contribs {
                    let item = providers_instances
                        .and_then(|m| m.get(provider_token))
                        .and_then(|inj| inj.instance.as_multi_item());
                    if let Some(item) = item {
                        items.push(item);
                    } else if let Ok(saved) = container.get_provider_instances(contrib_module_token)
                    {
                        if let Some(item) =
                            saved.get(provider_token).and_then(|p| p.as_multi_item())
                        {
                            items.push(item);
                        }
                    }
                }
                let collection: Arc<Box<dyn Provider>> =
                    Arc::new(Box::new(MultiCollectionProvider {
                        token: dependency.clone(),
                        items,
                    }));
                resolved_dependencies.insert(dependency, Injectable::new(collection, vec![]));
            }
            // Step 4: Not found anywhere
            else {
                return Err(LoadError::Failed(
                    format!("Dependency not found: {dependency} in module {module_token}").into(),
                ));
            }
        }

        Ok(resolved_dependencies)
    }

    fn resolve_from_imported_modules(
        &self,
        module_token: &String,
        dependency: &String,
    ) -> LoadResult<Option<Arc<Box<dyn Provider>>>> {
        let container = self.container.read();

        // Every import exporting the dependency. With two, the instance injected would depend on
        // which import was tried first (ADR-0057), and the injection is refused naming both.
        let mut exporters: Vec<&String> = Vec::new();
        for imported_module in container.imported_modules(module_token)? {
            if container
                .exported_tokens_of(imported_module)?
                .contains(dependency)
            {
                exporters.push(imported_module);
            }
        }
        exporters.sort();
        let imported_module = match exporters.as_slice() {
            [] => return Ok(None),
            [one] => *one,
            [first, second, ..] => {
                return Err(LoadError::Failed(
                    format!(
                        "`{dependency}` is exported into module `{module_token}` by two of its \
                         imports, `{first}` and `{second}`, and a provider or controller there \
                         injects it; \
                         import one of them, or bind one under a marker of its own"
                    )
                    .into(),
                ));
            }
        };

        if container
            .exported_instance_tokens(imported_module)?
            .contains(dependency)
        {
            Ok(container
                .get_provider_instance_by_token(imported_module, dependency)
                .ok()
                .flatten()
                .cloned())
        } else {
            // The import exports the dependency but has not built it yet.
            Err(LoadError::Deferred(format!(
                "imported module '{imported_module}' exports '{dependency}', \
                 whose instance is not created yet"
            )))
        }
    }
}

/// `Targets` with enhancer tokens already resolved — the shape Phase B stores from.
enum ResolvedDispatch {
    Http(Vec<(Arc<dyn Route>, EnhancerSet<Http>)>),
    Rpc(Arc<crate::rpc::RpcControllerWrapper>),
    Grpc(
        Arc<dyn crate::grpc::GrpcServiceSource>,
        Arc<crate::grpc::ResolvedGrpcEnhancers>,
    ),
}
