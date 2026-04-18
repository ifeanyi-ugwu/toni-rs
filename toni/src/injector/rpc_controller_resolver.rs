use std::cell::RefCell;
use std::rc::Rc;
use std::sync::Arc;

use anyhow::{Result, anyhow};

use crate::rpc::{RpcControllerTrait, RpcControllerWrapper};
use crate::traits_helpers::{ErrorHandler, Guard, Interceptor, Pipe};

use super::ToniContainer;

pub struct RpcControllerResolver {
    container: Rc<RefCell<ToniContainer>>,
}

impl RpcControllerResolver {
    pub fn new(container: Rc<RefCell<ToniContainer>>) -> Self {
        Self { container }
    }

    pub fn resolve(&self) -> Result<Vec<Arc<RpcControllerWrapper>>> {
        let raw = self.container.borrow().get_rpc_controllers().clone();
        raw.into_values()
            .map(|controller| {
                let wrapper = self.wrap_controller(controller)?;
                Ok(Arc::new(wrapper))
            })
            .collect()
    }

    fn wrap_controller(
        &self,
        controller: Arc<Box<dyn RpcControllerTrait>>,
    ) -> Result<RpcControllerWrapper> {
        let guards = self.resolve_guards(controller.get_guard_tokens())?;
        let interceptors = self.resolve_interceptors(controller.get_interceptor_tokens())?;
        let pipes = self.resolve_pipes(controller.get_pipe_tokens())?;
        let error_handlers = self.resolve_error_handlers(controller.get_error_handler_tokens())?;
        let route_metadata = controller.get_route_metadata();

        Ok(RpcControllerWrapper::new(
            controller,
            guards,
            interceptors,
            pipes,
            error_handlers,
            route_metadata,
        ))
    }

    fn resolve_guards(&self, tokens: Vec<String>) -> Result<Vec<Arc<dyn Guard>>> {
        let mut guards = Vec::new();
        let global_guards = self.container.borrow().get_global_enhancers().guards;
        guards.extend(global_guards);
        for token in tokens {
            guards.push(self.resolve_guard_by_token(&token)?);
        }
        Ok(guards)
    }

    fn resolve_interceptors(&self, tokens: Vec<String>) -> Result<Vec<Arc<dyn Interceptor>>> {
        let mut interceptors = Vec::new();
        let global_interceptors = self.container.borrow().get_global_enhancers().interceptors;
        interceptors.extend(global_interceptors);
        for token in tokens {
            interceptors.push(self.resolve_interceptor_by_token(&token)?);
        }
        Ok(interceptors)
    }

    fn resolve_pipes(&self, tokens: Vec<String>) -> Result<Vec<Arc<dyn Pipe>>> {
        let mut pipes = Vec::new();
        let global_pipes = self.container.borrow().get_global_enhancers().pipes;
        pipes.extend(global_pipes);
        for token in tokens {
            pipes.push(self.resolve_pipe_by_token(&token)?);
        }
        Ok(pipes)
    }

    fn resolve_error_handlers(&self, tokens: Vec<String>) -> Result<Vec<Arc<dyn ErrorHandler>>> {
        let mut error_handlers = Vec::new();
        let global_error_handlers = self
            .container
            .borrow()
            .get_global_enhancers()
            .error_handlers;
        error_handlers.extend(global_error_handlers);
        for token in tokens {
            error_handlers.push(self.resolve_error_handler_by_token(&token)?);
        }
        Ok(error_handlers)
    }

    fn resolve_guard_by_token(&self, token: &str) -> Result<Arc<dyn Guard>> {
        self.container
            .borrow()
            .get_role_registry()
            .guards
            .get(token)
            .cloned()
            .ok_or_else(|| anyhow!("Guard '{}' not found in role registry", token))
    }

    fn resolve_interceptor_by_token(&self, token: &str) -> Result<Arc<dyn Interceptor>> {
        self.container
            .borrow()
            .get_role_registry()
            .interceptors
            .get(token)
            .cloned()
            .ok_or_else(|| anyhow!("Interceptor '{}' not found in role registry", token))
    }

    fn resolve_pipe_by_token(&self, token: &str) -> Result<Arc<dyn Pipe>> {
        self.container
            .borrow()
            .get_role_registry()
            .pipes
            .get(token)
            .cloned()
            .ok_or_else(|| anyhow!("Pipe '{}' not found in role registry", token))
    }

    fn resolve_error_handler_by_token(&self, token: &str) -> Result<Arc<dyn ErrorHandler>> {
        self.container
            .borrow()
            .get_role_registry()
            .error_handlers
            .get(token)
            .cloned()
            .ok_or_else(|| anyhow!("ErrorHandler '{}' not found in role registry", token))
    }
}
