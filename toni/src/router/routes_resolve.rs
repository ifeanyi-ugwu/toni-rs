use std::{cell::RefCell, rc::Rc};

use anyhow::Result;

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
        let mut container = self.container.borrow_mut();
        let controllers = container.get_controllers_instance(&module_token)?;
        for (_, controller) in controllers {
            http_adapter.add_route(&controller.get_path(), controller.get_method(), controller);
        }
        Ok(())
    }
}
