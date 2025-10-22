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
        http_adapter: impl HttpAdapter,
    ) -> ToniApplication<impl HttpAdapter> {
        let container = Rc::new(RefCell::new(ToniContainer::new()));

        match self.initialize(module, container.clone()) {
            Ok(_) => (),
            Err(e) => {
                eprintln!("Falha crítica na inicialização do módulo: {}", e);
                std::process::exit(1);
            }
        };

        let mut app = ToniApplication::new(http_adapter, container);
        match app.init() {
            Ok(_) => (),
            Err(e) => {
                eprintln!("Falha na inicialização da aplicação: {}", e);
                std::process::exit(1);
            }
        }

        app
    }

    fn initialize(
        &self,
        module: ModuleDefinition,
        container: Rc<RefCell<ToniContainer>>,
    ) -> Result<()> {
        let mut scanner = ToniDependenciesScanner::new(container.clone());
        scanner.scan(module)?;

        ToniInstanceLoader::new(container.clone()).create_instances_of_dependencies()?;

        Ok(())
    }
}
