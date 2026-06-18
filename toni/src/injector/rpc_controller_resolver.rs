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
        let guards = self.resolve_guards(controller.get_guard_tokens())?;
        let interceptors = self.resolve_interceptors(controller.get_interceptor_tokens())?;
        let pipes = self.resolve_pipes(controller.get_pipe_tokens())?;
        let error_handlers = self.resolve_error_handlers(controller.get_error_handler_tokens())?;
        let error_observers = self.container.borrow().get_global_error_observers();
        let route_metadata = controller.get_route_metadata();

        let mut handler_guards: HashMap<String, Vec<RpcGuardEntry>> = HashMap::new();
        let mut handler_interceptors: HashMap<String, Vec<RpcInterceptorEntry>> = HashMap::new();
        let mut handler_pipes: HashMap<String, Vec<RpcPipeEntry>> = HashMap::new();
        let mut handler_error_handlers: HashMap<String, Vec<RpcErrorHandlerArc>> = HashMap::new();

        for pattern in controller.get_handler_patterns() {
            handler_guards.insert(
                pattern.clone(),
                self.resolve_handler_guards(controller.get_handler_guard_tokens(&pattern))?,
            );
            handler_interceptors.insert(
                pattern.clone(),
                self.resolve_handler_interceptors(
                    controller.get_handler_interceptor_tokens(&pattern),
                )?,
            );
            handler_pipes.insert(
                pattern.clone(),
                self.resolve_handler_pipes(controller.get_handler_pipe_tokens(&pattern))?,
            );
            handler_error_handlers.insert(
                pattern.clone(),
                self.resolve_handler_error_handlers(
                    controller.get_handler_error_handler_tokens(&pattern),
                )?,
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
            // Factory guards with request-scoped deps cannot be used on RPC controllers
            // because RPC has no HTTP request context. Fail at startup, not at invocation.
            if let RpcGuardEntry::Factory(ref f) = entry {
                if f.requires_http_parts() {
                    anyhow::bail!(
                        "Guard '{}' has request-scoped dependencies and cannot be used on an \
                         RPC controller — RPC has no HTTP request context",
                        token
                    );
                }
            }
            guards.push(entry);
        }
        Ok(guards)
    }

    fn resolve_interceptors(&self, tokens: Vec<String>) -> Result<Vec<RpcInterceptorEntry>> {
        let mut interceptors = self.container.borrow().get_global_rpc_interceptors();
        for token in tokens {
            let entry = self.resolve_interceptor_by_token(&token)?;
            if let RpcInterceptorEntry::Factory(ref f) = entry {
                if f.requires_http_parts() {
                    anyhow::bail!(
                        "Interceptor '{}' has request-scoped dependencies and cannot be used on \
                         an RPC controller — RPC has no HTTP request context",
                        token
                    );
                }
            }
            interceptors.push(entry);
        }
        Ok(interceptors)
    }

    fn resolve_pipes(&self, tokens: Vec<String>) -> Result<Vec<RpcPipeEntry>> {
        let mut pipes = self.container.borrow().get_global_rpc_pipes();
        for token in tokens {
            let entry = self.resolve_pipe_by_token(&token)?;
            if let RpcPipeEntry::Factory(ref f) = entry {
                if f.requires_http_parts() {
                    anyhow::bail!(
                        "Pipe '{}' has request-scoped dependencies and cannot be used on an \
                         RPC controller — RPC has no HTTP request context",
                        token
                    );
                }
            }
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
                if let RpcGuardEntry::Factory(ref f) = entry {
                    if f.requires_http_parts() {
                        anyhow::bail!(
                            "Guard '{}' has request-scoped dependencies and cannot be used on an \
                             RPC controller — RPC has no HTTP request context",
                            token
                        );
                    }
                }
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
                if let RpcInterceptorEntry::Factory(ref f) = entry {
                    if f.requires_http_parts() {
                        anyhow::bail!(
                            "Interceptor '{}' has request-scoped dependencies and cannot be used \
                             on an RPC controller — RPC has no HTTP request context",
                            token
                        );
                    }
                }
                Ok(entry)
            })
            .collect()
    }

    fn resolve_handler_pipes(&self, tokens: Vec<String>) -> Result<Vec<RpcPipeEntry>> {
        tokens
            .into_iter()
            .map(|token| {
                let entry = self.resolve_pipe_by_token(&token)?;
                if let RpcPipeEntry::Factory(ref f) = entry {
                    if f.requires_http_parts() {
                        anyhow::bail!(
                            "Pipe '{}' has request-scoped dependencies and cannot be used on an \
                             RPC controller — RPC has no HTTP request context",
                            token
                        );
                    }
                }
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
