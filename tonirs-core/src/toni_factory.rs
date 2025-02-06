use std::cell::RefCell;
use std::{error::Error, rc::Rc};

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

    pub fn create<'a>(
        &self,
        module: ModuleDefinition,
        http_adapter: impl HttpAdapter
    ) -> Result<impl HttpAdapter, Box<dyn Error>> 
    {
        // println!("FIELDS_STRUCT_CONTROLLER: {:?}", FIELDS_STRUCT_CONTROLLER);
        let container = Rc::new(RefCell::new(ToniContainer::new()));
        
        self.initialize(module, container.clone())?;
        
        let mut app = ToniApplication::new(http_adapter, container);
        app.init();
        let http_adapter2 = app.get_http_adapter();
        
        Ok(http_adapter2)
    }

    fn initialize(
        &self,
        module: ModuleDefinition,
        container: Rc<RefCell<ToniContainer>>
    ) -> Result<(), Box<dyn Error>> {
        let mut scanner = ToniDependenciesScanner::new(container.clone());
        scanner.scan(module)?;

        ToniInstanceLoader::new(container.clone())
            .create_instances_of_dependencies();

        Ok(())
    }
}