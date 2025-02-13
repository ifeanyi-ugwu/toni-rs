use std::{collections::hash_map::Drain, sync::Arc};

use anyhow::{Result, anyhow};
use rustc_hash::{FxHashMap, FxHashSet};

use crate::traits_helpers::{Controller, ControllerTrait, ModuleMetadata, Provider, ProviderTrait};

use super::module::Module;

pub struct ToniContainer {
    modules: FxHashMap<String, Module>,
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
        }
    }

    pub fn add_module(&mut self, module_metadata: Box<dyn ModuleMetadata>) {
        let token: String = module_metadata.get_id();
        let name: String = module_metadata.get_name();
        let module = Module::new(&token, &name, module_metadata);
        self.modules.insert(token, module);
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
        controller: Box<dyn Controller>,
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
        provider: Box<dyn Provider>,
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
        provider_instance: Arc<Box<dyn ProviderTrait>>,
    ) -> Result<()> {
        let module_ref = self
            .modules
            .get_mut(module_ref_token)
            .ok_or_else(|| anyhow!("Module not found"))?;
        module_ref.add_provider_instance(provider_instance);
        Ok(())
    }

    pub fn add_controller_instance(
        &mut self,
        module_ref_token: &String,
        controller_instance: Arc<Box<dyn ControllerTrait>>,
    ) -> Result<()> {
        let module_ref = self
            .modules
            .get_mut(module_ref_token)
            .ok_or_else(|| anyhow!("Module not found"))?;
        module_ref.add_controller_instance(controller_instance);
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

    pub fn get_providers_manager(
        &self,
        module_ref_token: &String,
    ) -> Result<&FxHashMap<String, Box<dyn Provider>>> {
        let module_ref = self
            .modules
            .get(module_ref_token)
            .ok_or_else(|| anyhow!("Module not found"))?;
        Ok(module_ref.get_providers_manager())
    }

    pub fn get_controllers_manager(
        &self,
        module_ref_token: &String,
    ) -> Result<&FxHashMap<String, Box<dyn Controller>>> {
        let module_ref = self
            .modules
            .get(module_ref_token)
            .ok_or_else(|| anyhow!("Module not found"))?;
        Ok(module_ref.get_controllers_manager())
    }

    pub fn get_providers_instance(
        &self,
        module_ref_token: &String,
    ) -> Result<&FxHashMap<String, Arc<Box<dyn ProviderTrait>>>> {
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
    ) -> Result<Option<&Arc<Box<dyn ProviderTrait>>>> {
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
    ) -> Result<Option<&dyn Provider>> {
        let module_ref = self
            .modules
            .get(module_ref_token)
            .ok_or_else(|| anyhow!("Module not found"))?;
        Ok(module_ref
            .get_provider_by_token(provider_token))
    }

    pub fn get_controllers_instance(
        &mut self,
        module_ref_token: &String,
    ) -> Result<Drain<'_, String, Arc<Box<dyn ControllerTrait>>>> {
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

        for (token, module) in self.modules.iter() {
            if module.get_imported_modules().is_empty() {
                ordered_modules.push(token.clone());
                visited.insert(token.clone(), true);
            }
        }
        while ordered_modules.len() < self.modules.len() {
            for (token, module) in self.modules.iter() {
                if visited.contains_key(token) {
                    continue;
                }

                let imported_modules = module.get_imported_modules();
                let all_imports_processed = imported_modules
                    .iter()
                    .all(|import_token| visited.contains_key(import_token));

                if all_imports_processed {
                    ordered_modules.push(token.clone());
                    visited.insert(token.clone(), true);
                }
            }
        }

        ordered_modules
    }

    pub fn get_module_by_token(&self, module_ref_token: &String) -> Option<&Module> {
        self.modules.get(module_ref_token)
    }
}
