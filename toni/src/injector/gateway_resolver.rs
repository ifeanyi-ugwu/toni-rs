use std::cell::RefCell;
use std::collections::HashMap;
use std::rc::Rc;
use std::sync::Arc;

use anyhow::{Result, anyhow};

use crate::traits_helpers::{ErrorHandler, GuardEntry, InterceptorEntry, PipeEntry};
use crate::websocket::{GatewayTrait, GatewayWrapper};

use super::ToniContainer;

pub struct GatewayResolver {
    container: Rc<RefCell<ToniContainer>>,
}

impl GatewayResolver {
    pub fn new(container: Rc<RefCell<ToniContainer>>) -> Self {
        Self { container }
    }

    pub fn resolve(&self) -> Result<HashMap<String, Arc<GatewayWrapper>>> {
        let raw = self.container.borrow().get_gateways().clone();
        raw.into_iter()
            .map(|(path, gateway)| {
                let wrapper = self.wrap_gateway(gateway)?;
                Ok((path, Arc::new(wrapper)))
            })
            .collect()
    }

    fn wrap_gateway(&self, gateway: Arc<Box<dyn GatewayTrait>>) -> Result<GatewayWrapper> {
        let guards = self.resolve_guards(gateway.get_guard_tokens())?;
        let interceptors = self.resolve_interceptors(gateway.get_interceptor_tokens())?;
        let pipes = self.resolve_pipes(gateway.get_pipe_tokens())?;
        let error_handlers = self.resolve_error_handlers(gateway.get_error_handler_tokens())?;
        let route_metadata = gateway.get_route_metadata();

        Ok(GatewayWrapper::new(
            gateway,
            guards,
            interceptors,
            pipes,
            error_handlers,
            route_metadata,
        ))
    }

    fn resolve_guards(&self, tokens: Vec<String>) -> Result<Vec<GuardEntry>> {
        let mut guards = self.container.borrow().get_global_enhancers().guards;
        for token in tokens {
            guards.push(self.resolve_guard_by_token(&token)?);
        }
        Ok(guards)
    }

    fn resolve_interceptors(&self, tokens: Vec<String>) -> Result<Vec<InterceptorEntry>> {
        let mut interceptors = self.container.borrow().get_global_enhancers().interceptors;
        for token in tokens {
            interceptors.push(self.resolve_interceptor_by_token(&token)?);
        }
        Ok(interceptors)
    }

    fn resolve_pipes(&self, tokens: Vec<String>) -> Result<Vec<PipeEntry>> {
        let mut pipes = self.container.borrow().get_global_enhancers().pipes;
        for token in tokens {
            pipes.push(self.resolve_pipe_by_token(&token)?);
        }
        Ok(pipes)
    }

    fn resolve_error_handlers(&self, tokens: Vec<String>) -> Result<Vec<Arc<dyn ErrorHandler>>> {
        let mut error_handlers = self.container.borrow().get_global_enhancers().error_handlers;
        for token in tokens {
            error_handlers.push(self.resolve_error_handler_by_token(&token)?);
        }
        Ok(error_handlers)
    }

    fn resolve_guard_by_token(&self, token: &str) -> Result<GuardEntry> {
        self.container
            .borrow()
            .get_role_registry()
            .guards
            .get(token)
            .cloned()
            .ok_or_else(|| anyhow!("Guard '{}' not found in role registry", token))
    }

    fn resolve_interceptor_by_token(&self, token: &str) -> Result<InterceptorEntry> {
        self.container
            .borrow()
            .get_role_registry()
            .interceptors
            .get(token)
            .cloned()
            .ok_or_else(|| anyhow!("Interceptor '{}' not found in role registry", token))
    }

    fn resolve_pipe_by_token(&self, token: &str) -> Result<PipeEntry> {
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
