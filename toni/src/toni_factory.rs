use std::cell::RefCell;
use std::rc::Rc;
use std::sync::Arc;

use anyhow::Result;

use crate::middleware::Middleware;
use crate::module_helpers::module_enum::ModuleDefinition;
use crate::toni_application::ToniApplication;
use crate::{
    http_adapter::HttpAdapter,
    injector::{ToniContainer, ToniInstanceLoader},
    scanner::ToniDependenciesScanner,
};

#[derive(Default)]
pub struct ToniFactory {
    global_middleware: Vec<Arc<dyn Middleware>>,
}

impl ToniFactory {
    #[inline]
    pub fn new() -> Self {
        Self {
            global_middleware: Vec::new(),
        }
    }

    pub fn use_global_middleware(&mut self, middleware: Arc<dyn Middleware>) -> &mut Self {
        self.global_middleware.push(middleware);
        self
    }

    pub async fn create(
        &self,
        module: ModuleDefinition,
        http_adapter: impl HttpAdapter,
    ) -> ToniApplication<impl HttpAdapter> {
        let container = Rc::new(RefCell::new(ToniContainer::new()));

        match self.initialize(module, container.clone()).await {
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

    async fn initialize(
        &self,
        module: ModuleDefinition,
        container: Rc<RefCell<ToniContainer>>,
    ) -> Result<()> {
        let mut scanner = ToniDependenciesScanner::new(container.clone());
        scanner.scan(module)?;

        // Register global middleware
        {
            let mut container_mut = container.borrow_mut();
            if let Some(middleware_manager) = container_mut.get_middleware_manager_mut() {
                for middleware in &self.global_middleware {
                    middleware_manager.add_global(middleware.clone());
                }
            }
        }

        scanner.scan_middleware()?;

        ToniInstanceLoader::new(container.clone()).create_instances_of_dependencies().await?;

        Ok(())
    }
}
