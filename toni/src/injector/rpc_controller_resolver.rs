use std::cell::RefCell;
use std::collections::HashMap;
use std::rc::Rc;

use anyhow::{Result, anyhow};

use crate::rpc::{RpcControllerTrait, RpcControllerWrapper};
use crate::traits_helpers::{RpcErrorHandlerArc, RpcGuardEntry, RpcInterceptorEntry, RpcPipeEntry};

use super::ToniContainer;

pub struct RpcControllerResolver {
    container: Rc<RefCell<ToniContainer>>,
}

impl RpcControllerResolver {
    pub fn new(container: Rc<RefCell<ToniContainer>>) -> Self {
        Self { container }
    }

    pub fn resolve(&self) -> Result<Vec<std::sync::Arc<RpcControllerWrapper>>> {
        let raw = self.container.borrow().get_rpc_controllers().clone();
        raw.into_values()
            .map(|controller| {
                let wrapper = self.wrap_controller(controller)?;
                Ok(std::sync::Arc::new(wrapper))
            })
            .collect()
    }

    fn wrap_controller(
        &self,
        controller: std::sync::Arc<Box<dyn RpcControllerTrait>>,
    ) -> Result<RpcControllerWrapper> {
        let enhancers = controller.enhancers();
        let guards = self.resolve_guards(enhancers.guard_tokens)?;
        let interceptors = self.resolve_interceptors(enhancers.interceptor_tokens)?;
        let pipes = self.resolve_pipes(enhancers.pipe_tokens)?;
        let error_handlers = self.resolve_error_handlers(enhancers.error_handler_tokens)?;
        let error_observers = self.container.borrow().get_global_error_observers();
        let route_metadata = controller.get_route_metadata();

        let mut handler_guards: HashMap<String, Vec<RpcGuardEntry>> = HashMap::new();
        let mut handler_interceptors: HashMap<String, Vec<RpcInterceptorEntry>> = HashMap::new();
        let mut handler_pipes: HashMap<String, Vec<RpcPipeEntry>> = HashMap::new();
        let mut handler_error_handlers: HashMap<String, Vec<RpcErrorHandlerArc>> = HashMap::new();

        for handler in enhancers.handlers {
            let pattern = handler.pattern;
            handler_guards.insert(
                pattern.clone(),
                self.resolve_handler_guards(handler.guard_tokens)?,
            );
            handler_interceptors.insert(
                pattern.clone(),
                self.resolve_handler_interceptors(handler.interceptor_tokens)?,
            );
            handler_pipes.insert(
                pattern.clone(),
                self.resolve_handler_pipes(handler.pipe_tokens)?,
            );
            handler_error_handlers.insert(
                pattern.clone(),
                self.resolve_handler_error_handlers(handler.error_handler_tokens)?,
            );
        }

        Ok(RpcControllerWrapper::new(
            controller,
            guards,
            interceptors,
            pipes,
            error_handlers,
            error_observers,
            route_metadata,
            handler_guards,
            handler_interceptors,
            handler_pipes,
            handler_error_handlers,
        ))
    }

    fn resolve_guards(&self, tokens: Vec<String>) -> Result<Vec<RpcGuardEntry>> {
        let mut guards = self.container.borrow().get_global_rpc_guards();
        for token in tokens {
            let entry = self.resolve_guard_by_token(&token)?;
            guards.push(entry);
        }
        Ok(guards)
    }

    fn resolve_interceptors(&self, tokens: Vec<String>) -> Result<Vec<RpcInterceptorEntry>> {
        let mut interceptors = self.container.borrow().get_global_rpc_interceptors();
        for token in tokens {
            let entry = self.resolve_interceptor_by_token(&token)?;
            interceptors.push(entry);
        }
        Ok(interceptors)
    }

    fn resolve_pipes(&self, tokens: Vec<String>) -> Result<Vec<RpcPipeEntry>> {
        let mut pipes = self.container.borrow().get_global_rpc_pipes();
        for token in tokens {
            let entry = self.resolve_pipe_by_token(&token)?;
            pipes.push(entry);
        }
        Ok(pipes)
    }

    fn resolve_error_handlers(&self, tokens: Vec<String>) -> Result<Vec<RpcErrorHandlerArc>> {
        let mut error_handlers = self.container.borrow().get_global_rpc_error_handlers();
        for token in tokens {
            error_handlers.push(self.resolve_error_handler_by_token(&token)?);
        }
        Ok(error_handlers)
    }

    fn resolve_guard_by_token(&self, token: &str) -> Result<RpcGuardEntry> {
        self.container
            .borrow()
            .get_role_registry()
            .rpc_guards
            .get(token)
            .cloned()
            .ok_or_else(|| {
                anyhow!(
                    "RPC Guard '{}' not found in registry. A guard registers automatically by \
                     implementing Guard<RpcContext>; make sure the provider is in the module's \
                     `providers` list. For `provider_factory!` under a string/const token, name \
                     the produced type so it can be detected — annotate the closure's return type \
                     (`|| -> MyGuard`) or pass a type hint.",
                    token
                )
            })
    }

    fn resolve_interceptor_by_token(&self, token: &str) -> Result<RpcInterceptorEntry> {
        self.container
            .borrow()
            .get_role_registry()
            .rpc_interceptors
            .get(token)
            .cloned()
            .ok_or_else(|| {
                anyhow!(
                    "RPC Interceptor '{}' not found in registry. An interceptor registers \
                     automatically by implementing Interceptor<RpcContext>; make sure the provider \
                     is in the module's `providers` list. For `provider_factory!` under a \
                     string/const token, name the produced type so it can be detected — annotate \
                     the closure's return type (`|| -> MyInterceptor`) or pass a type hint.",
                    token
                )
            })
    }

    fn resolve_pipe_by_token(&self, token: &str) -> Result<RpcPipeEntry> {
        self.container
            .borrow()
            .get_role_registry()
            .rpc_pipes
            .get(token)
            .cloned()
            .ok_or_else(|| {
                anyhow!(
                    "RPC Pipe '{}' not found in registry. A pipe registers automatically by \
                     implementing Pipe<RpcContext>; make sure the provider is in the module's \
                     `providers` list. For `provider_factory!` under a string/const token, name \
                     the produced type so it can be detected — annotate the closure's return type \
                     (`|| -> MyPipe`) or pass a type hint.",
                    token
                )
            })
    }

    fn resolve_error_handler_by_token(&self, token: &str) -> Result<RpcErrorHandlerArc> {
        self.container
            .borrow()
            .get_role_registry()
            .rpc_error_handlers
            .get(token)
            .cloned()
            .ok_or_else(|| {
                anyhow!(
                    "RPC ErrorHandler '{}' not found in registry. An error handler registers \
                     automatically by implementing ErrorHandler<RpcContext, RpcData>; make sure \
                     the provider is in the module's `providers` list. For `provider_factory!` \
                     under a string/const token, name the produced type so it can be detected — \
                     annotate the closure's return type or pass a type hint.",
                    token
                )
            })
    }

    fn resolve_handler_guards(&self, tokens: Vec<String>) -> Result<Vec<RpcGuardEntry>> {
        tokens
            .into_iter()
            .map(|token| {
                let entry = self.resolve_guard_by_token(&token)?;
                Ok(entry)
            })
            .collect()
    }

    fn resolve_handler_interceptors(
        &self,
        tokens: Vec<String>,
    ) -> Result<Vec<RpcInterceptorEntry>> {
        tokens
            .into_iter()
            .map(|token| {
                let entry = self.resolve_interceptor_by_token(&token)?;
                Ok(entry)
            })
            .collect()
    }

    fn resolve_handler_pipes(&self, tokens: Vec<String>) -> Result<Vec<RpcPipeEntry>> {
        tokens
            .into_iter()
            .map(|token| {
                let entry = self.resolve_pipe_by_token(&token)?;
                Ok(entry)
            })
            .collect()
    }

    fn resolve_handler_error_handlers(
        &self,
        tokens: Vec<String>,
    ) -> Result<Vec<RpcErrorHandlerArc>> {
        tokens
            .into_iter()
            .map(|t| self.resolve_error_handler_by_token(&t))
            .collect()
    }
}
