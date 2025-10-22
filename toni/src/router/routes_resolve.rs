use anyhow::Result;
use std::{cell::RefCell, rc::Rc};

use crate::{http_adapter::HttpAdapter, injector::ToniContainer};

pub struct RoutesResolver {
    container: Rc<RefCell<ToniContainer>>,
}

impl RoutesResolver {
    pub fn new(container: Rc<RefCell<ToniContainer>>) -> Self {
        Self { container }
    }

    pub fn resolve(&mut self, http_adapter: &mut impl HttpAdapter) -> Result<()> {
        let modules_token = self.container.borrow().get_modules_token();

        for module_token in modules_token {
            self.register_routes(module_token, http_adapter)?;
        }
        Ok(())
    }

    fn register_routes(
        &mut self,
        module_token: String,
        http_adapter: &mut impl HttpAdapter,
    ) -> Result<()> {
        let controllers_vec: Vec<_> = {
            let mut container = self.container.borrow_mut();
            let controllers = container.get_controllers_instance(&module_token)?;
            controllers.collect()
        };

        // Process each controller
        for (_, mut controller) in controllers_vec {
            let route_path = controller.get_path();

            let route_middleware = {
                let container = self.container.borrow(); // Immutable borrow
                if let Some(middleware_manager) = container.get_middleware_manager() {
                    middleware_manager.get_middleware_for_route(&module_token, &route_path)
                } else {
                    Vec::new()
                }
            };

            // Apply middleware to controller
            if let Some(wrapper) = std::sync::Arc::get_mut(&mut controller) {
                wrapper.set_middleware(route_middleware);
            }

            // Register route
            http_adapter.add_route(&route_path, controller.get_method(), controller);
        }

        Ok(())
    }
}
