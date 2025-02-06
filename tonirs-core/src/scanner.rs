use std::{cell::RefCell, error::Error, rc::Rc};

use crate::{
    injector::ToniContainer,
    module_helpers::module_enum::ModuleDefinition,
    traits_helpers::{ModuleMetadata, Provider},
};

pub struct ToniDependenciesScanner {
    container: Rc<RefCell<ToniContainer>>,
}

impl ToniDependenciesScanner {
    pub fn new(container: Rc<RefCell<ToniContainer>>) -> Self {
        Self { container }
    }
    pub fn scan(&mut self, module: ModuleDefinition) -> Result<(), Box<dyn Error>> {
        self.scan_for_modules_with_imports(module);
        self.scan_modules_for_dependencies()?;
        Ok(())
    }
    fn scan_for_modules_with_imports(
        &mut self,
        module: ModuleDefinition,
    ) -> Result<(), Box<dyn Error>> {
        let mut ctx_registry: Vec<String> = vec![];

        let mut stack: Vec<ModuleDefinition> = vec![module];

        while let Some(current_module_definition) = stack.pop() {
            let default_module = match current_module_definition {
                ModuleDefinition::DefaultModule(default_module) => default_module,
            };

            ctx_registry.push(default_module.get_name());

            let modules_imported: Vec<Box<dyn ModuleMetadata>> = match default_module.imports() {
                Some(modules_imported) => modules_imported,
                None => Vec::new(),
            };

            let mut modules_imported_tokens = vec![];

            for module_imported in modules_imported {
                modules_imported_tokens.push(module_imported.get_id());

                if ctx_registry
                    .iter()
                    .any(|module_imported_id| module_imported_id == &module_imported.get_name())
                {
                    continue;
                }

                stack.push(ModuleDefinition::DefaultModule(module_imported));
            }
            let default_module_id = default_module.get_id();
            self.insert_module(default_module);
            self.insert_imports(default_module_id, modules_imported_tokens);
        }
        Ok(())
    }

    pub fn scan_modules_for_dependencies(&mut self) -> Result<(), Box<dyn Error>> {
        let modules_token = self.container.borrow().get_modules_token();
        for module_token in modules_token {
            self.insert_providers(module_token.clone())?;
            self.insert_controllers(module_token.clone())?;
            self.insert_exports(module_token.clone())?;
        }
        Ok(())
    }

    fn insert_module(&mut self, module: Box<dyn ModuleMetadata>) {
        let mut container = self.container.borrow_mut();
        container.add_module(module);
    }

    pub fn insert_imports(
        &mut self,
        module_token: String,
        imports: Vec<String>,
    ) -> Result<(), Box<dyn Error>> {
        let mut container = self.container.borrow_mut();

        for import in imports {
            container.add_import(&module_token, import);
        }

        Ok(())
    }

    pub fn insert_controllers(&mut self, module_token: String) -> Result<(), Box<dyn Error>> {
        let mut container = self.container.borrow_mut();
        let module_ref = container.get_module_by_token(&module_token);
        let resolved_module_ref = match module_ref {
            Some(module_ref) => module_ref,
            None => return Err("Module not found".into()),
        };

        let controllers = resolved_module_ref.get_metadata().controllers();

        if let Some(controllers) = controllers {
            for controller in controllers {
                container.add_controller(&module_token, controller);
            }
        };

        Ok(())
    }

    pub fn insert_providers(&mut self, module_token: String) -> Result<(), Box<dyn Error>> {
        let mut container = self.container.borrow_mut();
        let module_ref = container.get_module_by_token(&module_token);
        let resolved_module_ref = match module_ref {
            Some(module_ref) => module_ref,
            None => return Err("Module not found".into()),
        };

        let providers =
            resolved_module_ref.get_metadata().providers();

        if let Some(providers) = providers {
            for provider in providers {
                container.add_provider(&module_token, provider);
            }
        };

        Ok(())
    }

    pub fn insert_exports(&mut self, module_token: String) -> Result<(), Box<dyn Error>> {
        let mut container = self.container.borrow_mut();
        let module_ref = container.get_module_by_token(&module_token);
        let resolved_module_ref = match module_ref {
            Some(module_ref) => module_ref,
            None => return Err("Module not found".into()),
        };

        let exports = resolved_module_ref.get_metadata().exports();
        if let Some(exports) = exports {
            for export in exports {
                container.add_export(&module_token, export);
            }
        };

        Ok(())
    }
}
