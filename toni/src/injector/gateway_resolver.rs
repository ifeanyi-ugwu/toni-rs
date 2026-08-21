use std::cell::RefCell;
use std::collections::HashMap;
use std::rc::Rc;
use std::sync::Arc;

use anyhow::{Result, anyhow};

use crate::traits_helpers::{WsErrorHandlerArc, WsGuardEntry, WsInterceptorEntry, WsPipeEntry};
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
        let enhancers = gateway.enhancers();
        let guards = self.resolve_guards(enhancers.guard_tokens)?;
        let interceptors = self.resolve_interceptors(enhancers.interceptor_tokens)?;
        let pipes = self.resolve_pipes(enhancers.pipe_tokens)?;
        let error_handlers = self.resolve_error_handlers(enhancers.error_handler_tokens)?;
        let route_metadata = gateway.get_route_metadata();
        let handler_metadata: HashMap<String, std::sync::Arc<crate::http_helpers::RouteMetadata>> =
            gateway.handler_metadata().into_iter().collect();

        let mut handler_guards: HashMap<String, Vec<WsGuardEntry>> = HashMap::new();
        let mut handler_interceptors: HashMap<String, Vec<WsInterceptorEntry>> = HashMap::new();
        let mut handler_pipes: HashMap<String, Vec<WsPipeEntry>> = HashMap::new();
        let mut handler_error_handlers: HashMap<String, Vec<WsErrorHandlerArc>> = HashMap::new();

        for handler in enhancers.handlers {
            let event = handler.event;
            handler_guards.insert(
                event.clone(),
                self.resolve_tokens_only(handler.guard_tokens)?,
            );
            handler_interceptors.insert(
                event.clone(),
                self.resolve_interceptor_tokens_only(handler.interceptor_tokens)?,
            );
            handler_pipes.insert(
                event.clone(),
                self.resolve_pipe_tokens_only(handler.pipe_tokens)?,
            );
            handler_error_handlers.insert(
                event.clone(),
                self.resolve_error_handler_tokens_only(handler.error_handler_tokens)?,
            );
        }

        let error_observers = self.container.borrow().get_global_error_observers();
        Ok(GatewayWrapper::new(
            gateway,
            guards,
            interceptors,
            pipes,
            error_handlers,
            error_observers,
            route_metadata,
            handler_metadata,
            handler_guards,
            handler_interceptors,
            handler_pipes,
            handler_error_handlers,
        ))
    }

    fn resolve_guards(&self, tokens: Vec<String>) -> Result<Vec<WsGuardEntry>> {
        let mut guards = self.container.borrow().get_global_ws_guards();
        for token in tokens {
            let entry = self.resolve_guard_by_token(&token)?;
            guards.push(entry);
        }
        Ok(guards)
    }

    fn resolve_interceptors(&self, tokens: Vec<String>) -> Result<Vec<WsInterceptorEntry>> {
        let mut interceptors = self.container.borrow().get_global_ws_interceptors();
        for token in tokens {
            let entry = self.resolve_interceptor_by_token(&token)?;
            interceptors.push(entry);
        }
        Ok(interceptors)
    }

    fn resolve_pipes(&self, tokens: Vec<String>) -> Result<Vec<WsPipeEntry>> {
        let mut pipes = self.container.borrow().get_global_ws_pipes();
        for token in tokens {
            let entry = self.resolve_pipe_by_token(&token)?;
            pipes.push(entry);
        }
        Ok(pipes)
    }

    fn resolve_error_handlers(&self, tokens: Vec<String>) -> Result<Vec<WsErrorHandlerArc>> {
        let mut error_handlers = self.container.borrow().get_global_ws_error_handlers();
        for token in tokens {
            error_handlers.push(self.resolve_error_handler_by_token(&token)?);
        }
        Ok(error_handlers)
    }

    fn resolve_tokens_only(&self, tokens: Vec<String>) -> Result<Vec<WsGuardEntry>> {
        tokens
            .into_iter()
            .map(|token| {
                let entry = self.resolve_guard_by_token(&token)?;
                Ok(entry)
            })
            .collect()
    }

    fn resolve_interceptor_tokens_only(
        &self,
        tokens: Vec<String>,
    ) -> Result<Vec<WsInterceptorEntry>> {
        tokens
            .into_iter()
            .map(|token| {
                let entry = self.resolve_interceptor_by_token(&token)?;
                Ok(entry)
            })
            .collect()
    }

    fn resolve_pipe_tokens_only(&self, tokens: Vec<String>) -> Result<Vec<WsPipeEntry>> {
        tokens
            .into_iter()
            .map(|token| {
                let entry = self.resolve_pipe_by_token(&token)?;
                Ok(entry)
            })
            .collect()
    }

    fn resolve_error_handler_tokens_only(
        &self,
        tokens: Vec<String>,
    ) -> Result<Vec<WsErrorHandlerArc>> {
        tokens
            .into_iter()
            .map(|t| self.resolve_error_handler_by_token(&t))
            .collect()
    }

    fn resolve_guard_by_token(&self, token: &str) -> Result<WsGuardEntry> {
        self.container
            .borrow()
            .get_role_registry()
            .ws_guards
            .get(token)
            .cloned()
            .ok_or_else(|| {
                anyhow!(
                    "WS Guard '{}' not found in registry. A guard registers automatically by \
                     implementing Guard<WsContext>; make sure the provider is in the module's \
                     `providers` list. For `provider_factory!` under a string/const token, name \
                     the produced type so it can be detected — annotate the closure's return type \
                     (`|| -> MyGuard`) or pass a type hint.",
                    token
                )
            })
    }

    fn resolve_interceptor_by_token(&self, token: &str) -> Result<WsInterceptorEntry> {
        self.container
            .borrow()
            .get_role_registry()
            .ws_interceptors
            .get(token)
            .cloned()
            .ok_or_else(|| {
                anyhow!(
                    "WS Interceptor '{}' not found in registry. An interceptor registers \
                     automatically by implementing Interceptor<WsContext>; make sure the provider \
                     is in the module's `providers` list. For `provider_factory!` under a \
                     string/const token, name the produced type so it can be detected — annotate \
                     the closure's return type (`|| -> MyInterceptor`) or pass a type hint.",
                    token
                )
            })
    }

    fn resolve_pipe_by_token(&self, token: &str) -> Result<WsPipeEntry> {
        self.container
            .borrow()
            .get_role_registry()
            .ws_pipes
            .get(token)
            .cloned()
            .ok_or_else(|| {
                anyhow!(
                    "WS Pipe '{}' not found in registry. A pipe registers automatically by \
                     implementing Pipe<WsContext>; make sure the provider is in the module's \
                     `providers` list. For `provider_factory!` under a string/const token, name \
                     the produced type so it can be detected — annotate the closure's return type \
                     (`|| -> MyPipe`) or pass a type hint.",
                    token
                )
            })
    }

    fn resolve_error_handler_by_token(&self, token: &str) -> Result<WsErrorHandlerArc> {
        self.container
            .borrow()
            .get_role_registry()
            .ws_error_handlers
            .get(token)
            .cloned()
            .ok_or_else(|| {
                anyhow!(
                    "WS ErrorHandler '{}' not found in registry. An error handler registers \
                     automatically by implementing ErrorHandler<WsContext, WsMessage>; make sure \
                     the provider is in the module's `providers` list. For `provider_factory!` \
                     under a string/const token, name the produced type so it can be detected — \
                     annotate the closure's return type or pass a type hint.",
                    token
                )
            })
    }
}
