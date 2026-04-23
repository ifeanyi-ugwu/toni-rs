use anyhow::Result;
use std::{cell::RefCell, rc::Rc, sync::Arc};

use crate::{
    http_adapter::ErasedHttpAdapter,
    injector::ToniContainer,
    middleware::MiddlewareChain,
};

use super::{
    RequestDispatcher,
    framework_router::FrameworkRouterBuilder,
};

pub struct RoutesResolver {
    pub(crate) container: Rc<RefCell<ToniContainer>>,
}

impl RoutesResolver {
    pub fn new(container: Rc<RefCell<ToniContainer>>) -> Self {
        Self { container }
    }

    pub fn resolve(&mut self, http_adapter: &mut dyn ErasedHttpAdapter) -> Result<()> {
        let modules_token = self.container.borrow().get_modules_token();
        let mut builder = FrameworkRouterBuilder::new();

        for module_token in modules_token {
            self.register_routes(module_token, &mut builder)?;
        }

        let router = builder.build();

        // Build the global middleware chain separately — it wraps the entire
        // router at dispatch time, so it runs pre-routing on every request.
        let global_chain = {
            let container = self.container.borrow();
            let mut chain = MiddlewareChain::new();
            if let Some(mm) = container.get_middleware_manager() {
                for mw in mm.get_global_middleware() {
                    chain.use_middleware(mw.clone());
                }
            }
            chain
        };

        let dispatcher = Arc::new(RequestDispatcher::new(router, global_chain));
        http_adapter.set_dispatcher(dispatcher);

        Ok(())
    }

    fn register_routes(
        &mut self,
        module_token: String,
        builder: &mut FrameworkRouterBuilder,
    ) -> Result<()> {
        let controllers_vec: Vec<_> = {
            let mut container = self.container.borrow_mut();
            let controllers = container.get_controllers_instance(&module_token)?;
            controllers.collect()
        };

        for (_, mut wrapper) in controllers_vec {
            let route_path = wrapper.get_path();
            let route_method = wrapper.get_method();

            // Route-scoped middleware only — global middleware lives in the dispatcher.
            let route_middleware = {
                let container = self.container.borrow();
                if let Some(mm) = container.get_middleware_manager() {
                    mm.get_middleware_for_route(
                        &module_token,
                        &route_path,
                        route_method.as_str(),
                    )
                } else {
                    Vec::new()
                }
            };

            tracing::debug!(
                method = %route_method.as_str(),
                path = %route_path,
                middleware = route_middleware.len(),
                "route registered"
            );

            if let Some(w) = Arc::get_mut(&mut wrapper) {
                w.set_middleware(route_middleware);
            }

            builder.insert(route_method, &route_path, wrapper);
        }

        Ok(())
    }
}
