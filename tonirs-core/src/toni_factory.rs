use std::cell::RefCell;
use std::rc::Rc;

use anyhow::Result;

use crate::module_helpers::module_enum::ModuleDefinition;
use crate::toni_application::ToniApplication;
use crate::{
    http_adapter::HttpAdapter,
    injector::{ToniContainer, ToniInstanceLoader},
    scanner::ToniDependenciesScanner,
};

#[derive(Default)]
pub struct ToniFactory;

impl ToniFactory {
    #[inline]
    pub fn new() -> Self {
        Self
    }

    pub fn create(
        &self,
        module: ModuleDefinition,
        http_adapter: impl HttpAdapter
    ) -> Result<ToniApplication<impl HttpAdapter>> 
    {
        let container = Rc::new(RefCell::new(ToniContainer::new()));
        
        self.initialize(module, container.clone())?;
        
        let mut app = ToniApplication::new(http_adapter, container);
        app.init()?;
        
        Ok(app)
    }

    fn initialize(
        &self,
        module: ModuleDefinition,
        container: Rc<RefCell<ToniContainer>>
    ) -> Result<()> {
        let mut scanner = ToniDependenciesScanner::new(container.clone());
        scanner.scan(module)?;

        ToniInstanceLoader::new(container.clone())
            .create_instances_of_dependencies()?;

        Ok(())
    }
}