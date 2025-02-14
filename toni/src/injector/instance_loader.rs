use anyhow::{Result, anyhow};
use rustc_hash::FxHashMap;
use std::{
    cell::{RefCell, RefMut},
    rc::Rc,
    sync::Arc,
};

use super::{DependencyGraph, ToniContainer};
use crate::traits_helpers::{ControllerTrait, ProviderTrait};

pub struct ToniInstanceLoader {
    container: Rc<RefCell<ToniContainer>>,
}

impl ToniInstanceLoader {
    pub fn new(container: Rc<RefCell<ToniContainer>>) -> Self {
        Self { container }
    }

    pub fn create_instances_of_dependencies(&self) -> Result<()> {
        let modules_token = self.container.borrow().get_ordered_modules_token();

        for module_token in modules_token {
            self.create_module_instances(module_token)?;
        }
        Ok(())
    }

    fn create_module_instances(&self, module_token: String) -> Result<()> {
        self.create_instances_of_providers(module_token.clone())?;
        self.create_instances_of_controllers(module_token.clone())?;
        Ok(())
    }

    fn create_instances_of_providers(&self, module_token: String) -> Result<()> {
        let dependency_graph = DependencyGraph::new(self.container.clone(), module_token.clone());
        let ordered_providers_token = dependency_graph.get_ordered_providers_token()?;
        let provider_instances = {
            let container = self.container.borrow();
            let mut instances = FxHashMap::default();

            for provider_token in ordered_providers_token {
                let provider_manager = container
                    .get_provider_by_token(&module_token, &provider_token)?
                    .ok_or_else(|| anyhow!("Provider not found: {}", provider_token))?;

                let dependencies = provider_manager.get_dependencies();
                let resolved_dependencies =
                    self.resolve_dependencies(&module_token, dependencies, Some(&instances))?;

                let provider_instances = provider_manager.get_all_providers(&resolved_dependencies);
                instances.extend(provider_instances);
            }
            instances
        };
        self.add_providers_instances(&module_token, provider_instances)?;
        Ok(())
    }

    fn add_providers_instances(
        &self,
        module_token: &String,
        providers_instances: FxHashMap<String, Arc<Box<dyn ProviderTrait>>>,
    ) -> Result<()> {
        let mut container = self.container.borrow_mut();
        let mut providers_tokens = Vec::new();
        for (provider_instance_token, provider_instance) in providers_instances {
            let token_manager = provider_instance.get_token_manager().clone();
            container.add_provider_instance(module_token, provider_instance)?;
            providers_tokens.push((token_manager, provider_instance_token));
        }

        self.resolve_exports(module_token, providers_tokens, container)?;
        Ok(())
    }

    fn resolve_exports(
        &self,
        module_token: &String,
        providers_tokens: Vec<(String, String)>,
        container: RefMut<'_, ToniContainer>,
    ) -> Result<()> {
        let exports = container.get_exports_tokens_vec(module_token)?;
        self.add_export_instances_tokens(module_token, providers_tokens, exports, container)?;
        Ok(())
    }

    fn add_export_instances_tokens(
        &self,
        module_token: &String,
        providers_tokens: Vec<(String, String)>,
        exports: Vec<String>,
        mut container: RefMut<'_, ToniContainer>,
    ) -> Result<()> {
        for (provider_manager_token, provider_instance_token) in providers_tokens {
            if exports.contains(&provider_manager_token) {
                container.add_export_instance(module_token, provider_instance_token)?;
            }
        }
        Ok(())
    }

    fn create_instances_of_controllers(&self, module_token: String) -> Result<()> {
        let controllers_instances = {
            let container = self.container.borrow();
            let mut instances = FxHashMap::default();
            let controllers_manager = container.get_controllers_manager(&module_token)?;

            for controller_manager in controllers_manager.values() {
                let dependencies = controller_manager.get_dependencies();
                let resolved_dependencies =
                    self.resolve_dependencies(&module_token, dependencies, None)?;
                let controllers_instances =
                    controller_manager.get_all_controllers(&resolved_dependencies);
                instances.extend(controllers_instances);
            }
            instances
        };
        self.add_controllers_instances(module_token, controllers_instances)?;
        Ok(())
    }

    fn add_controllers_instances(
        &self,
        module_token: String,
        controllers_instances: FxHashMap<String, Arc<Box<dyn ControllerTrait>>>,
    ) -> Result<()> {
        let mut container_mut = self.container.borrow_mut();
        for (_controller_instance_token, controller_instance) in controllers_instances {
            container_mut.add_controller_instance(&module_token, controller_instance)?;
        }
        Ok(())
    }

    fn resolve_dependencies(
        &self,
        module_token: &String,
        dependencies: Vec<String>,
        providers_instances: Option<&FxHashMap<String, Arc<Box<dyn ProviderTrait>>>>,
    ) -> Result<FxHashMap<String, Arc<Box<dyn ProviderTrait>>>> {
        let container = self.container.borrow();
        let mut resolved_dependencies = FxHashMap::default();

        for dependency in dependencies {
            let instances = match providers_instances {
                Some(providers_instances) => providers_instances,
                None => container.get_providers_instance(module_token)?,
            };
            if let Some(instance) = instances.get(&dependency) {
                resolved_dependencies.insert(dependency, instance.clone());
            } else if let Some(exported_instance) =
                self.resolve_from_imported_modules(module_token, &dependency)?
            {
                resolved_dependencies.insert(dependency, exported_instance.clone());
            } else {
                return Err(anyhow!(
                    "Dependency not found: {} in module {}",
                    dependency,
                    module_token
                ));
            }
        }

        Ok(resolved_dependencies)
    }

    fn resolve_from_imported_modules(
        &self,
        module_token: &String,
        dependency: &String,
    ) -> Result<Option<Arc<Box<dyn ProviderTrait>>>> {
        let container = self.container.borrow();
        let imported_modules = container.get_imported_modules(module_token)?;
        for imported_module in imported_modules {
            let exported_instances_tokens =
                container.get_exports_instances_tokens(imported_module)?;
            if exported_instances_tokens.contains(dependency) {
                if let Ok(Some(exported_instance) )=
                    container.get_provider_instance_by_token(imported_module, dependency)
                {
                    return Ok(Some(exported_instance.clone()));
                }
            }
        }

        Ok(None)
    }
}
