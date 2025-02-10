use super::ToniContainer;
use anyhow::{anyhow, Result};
use rustc_hash::FxHashMap;
use std::{cell::RefCell, rc::Rc};

pub struct DependencyGraph {
    container: Rc<RefCell<ToniContainer>>,
    module_token: String,
    visited: FxHashMap<String, bool>,
    temp_mark: FxHashMap<String, bool>,
    ordered: Vec<String>,
}

impl DependencyGraph {
    pub fn new(container: Rc<RefCell<ToniContainer>>, module_token: String) -> Self {
        Self {
            container,
            module_token,
            visited: FxHashMap::default(),
            temp_mark: FxHashMap::default(),
            ordered: Vec::new(),
        }
    }

    pub fn get_ordered_providers_token(mut self) -> Result<Vec<String>> {
        let providers = {
            let container = self.container.borrow();
            let providers_map = container.get_providers_manager(&self.module_token)?;
            providers_map
                .iter()
                .map(|(token, provider)| (token.clone(), provider.get_dependencies()))
                .collect::<Vec<(String, Vec<String>)>>()
        };
        let clone_providers = providers.clone();
        for (token, dependencies) in providers {
            if !self.visited.contains_key(&token) {
                self.visit_node(token, dependencies, &clone_providers)?;
            }
        }
        Ok(self.ordered)
    }

    fn visit_node(&mut self, token: String, dependencies: Vec<String>, providers: &Vec<(String, Vec<String>)>) -> Result<()> {
        if self.temp_mark.contains_key(&token) {
            return Err(anyhow!("Circular dependency detected for provider: {}", token));
        }

        if self.visited.contains_key(&token) {
            return Ok(());
        }

        self.temp_mark.insert(token.clone(), true);

        for dep_token in dependencies {
            if let Some((dep_token, dependencies)) = providers
                .iter()
                .find(|(token, _dependencies)| dep_token.contains(token))
            {
                self.visit_node(dep_token.clone(), dependencies.clone(), providers)?;
            }
        }

        self.temp_mark.remove(&token);
        self.visited.insert(token.clone(), true);
        self.ordered.push(token);
        Ok(())
    }
}
