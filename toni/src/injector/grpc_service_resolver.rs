use std::cell::RefCell;
use std::collections::HashMap;
use std::rc::Rc;
use std::sync::Arc;

use anyhow::{Result, anyhow};

use crate::adapter::{GrpcServiceTrait, ResolvedGrpcEnhancers};
use crate::traits_helpers::{GrpcErrorHandlerArc, GrpcGuardEntry, GrpcInterceptorEntry};

use super::ToniContainer;

/// Resolves the per-service enhancer bundle from the role registry by token.
/// Mirrors [`RpcControllerResolver`](super::RpcControllerResolver) — the
/// adapter receives a `(service, enhancers)` pair per service and forwards
/// `enhancers` into [`GrpcServiceTrait::register_with`].
pub struct GrpcServiceResolver {
    container: Rc<RefCell<ToniContainer>>,
}

impl GrpcServiceResolver {
    pub fn new(container: Rc<RefCell<ToniContainer>>) -> Self {
        Self { container }
    }

    pub fn resolve(
        &self,
    ) -> Result<Vec<(Arc<Box<dyn GrpcServiceTrait>>, Arc<ResolvedGrpcEnhancers>)>> {
        let services = self
            .container
            .borrow()
            .get_grpc_services()
            .clone();
        services
            .into_values()
            .map(|svc| {
                let enhancers = self.resolve_for(svc.as_ref().as_ref())?;
                Ok((svc, Arc::new(enhancers)))
            })
            .collect()
    }

    fn resolve_for(&self, svc: &dyn GrpcServiceTrait) -> Result<ResolvedGrpcEnhancers> {
        let guards = self.resolve_guards(svc.get_guard_tokens())?;
        let interceptors = self.resolve_interceptors(svc.get_interceptor_tokens())?;
        let error_handlers = self.resolve_error_handlers(svc.get_error_handler_tokens())?;

        let mut handler_guards: HashMap<String, Vec<GrpcGuardEntry>> = HashMap::new();
        let mut handler_interceptors: HashMap<String, Vec<GrpcInterceptorEntry>> = HashMap::new();
        let mut handler_error_handlers: HashMap<String, Vec<GrpcErrorHandlerArc>> = HashMap::new();
        for method in svc.get_handler_methods() {
            handler_guards.insert(
                method.clone(),
                self.resolve_guards(svc.get_handler_guard_tokens(&method))?,
            );
            handler_interceptors.insert(
                method.clone(),
                self.resolve_interceptors(svc.get_handler_interceptor_tokens(&method))?,
            );
            handler_error_handlers.insert(
                method.clone(),
                self.resolve_error_handlers(svc.get_handler_error_handler_tokens(&method))?,
            );
        }

        let error_observers = self.container.borrow().get_global_error_observers();

        Ok(ResolvedGrpcEnhancers {
            guards,
            handler_guards,
            interceptors,
            handler_interceptors,
            error_handlers,
            handler_error_handlers,
            error_observers,
        })
    }

    fn resolve_guards(&self, tokens: Vec<String>) -> Result<Vec<GrpcGuardEntry>> {
        let mut guards = self.container.borrow().get_global_grpc_guards();
        for token in tokens {
            let entry = self.resolve_guard_by_token(&token)?;
            // gRPC has no HTTP request context — request-scoped factories
            // can't be honoured. Fail at startup rather than at first call.
            if let GrpcGuardEntry::Factory(ref f) = entry {
                if f.requires_http_parts() {
                    anyhow::bail!(
                        "Guard '{}' has request-scoped dependencies and cannot be used on a \
                         gRPC service — gRPC has no HTTP request context",
                        token
                    );
                }
            }
            guards.push(entry);
        }
        Ok(guards)
    }

    fn resolve_guard_by_token(&self, token: &str) -> Result<GrpcGuardEntry> {
        self.container
            .borrow()
            .get_role_registry()
            .grpc_guards
            .get(token)
            .cloned()
            .ok_or_else(|| {
                anyhow!(
                    "gRPC Guard '{}' not found in registry. \
                     Implement Guard<GrpcContext> and mark the provider with \
                     `#[guard(grpc)]` or a typed impl head.",
                    token
                )
            })
    }

    fn resolve_interceptors(&self, tokens: Vec<String>) -> Result<Vec<GrpcInterceptorEntry>> {
        let mut interceptors = self.container.borrow().get_global_grpc_interceptors();
        for token in tokens {
            let entry = self.resolve_interceptor_by_token(&token)?;
            if let GrpcInterceptorEntry::Factory(ref f) = entry {
                if f.requires_http_parts() {
                    anyhow::bail!(
                        "Interceptor '{}' has request-scoped dependencies and cannot be used on \
                         a gRPC service — gRPC has no HTTP request context",
                        token
                    );
                }
            }
            interceptors.push(entry);
        }
        Ok(interceptors)
    }

    fn resolve_interceptor_by_token(&self, token: &str) -> Result<GrpcInterceptorEntry> {
        self.container
            .borrow()
            .get_role_registry()
            .grpc_interceptors
            .get(token)
            .cloned()
            .ok_or_else(|| {
                anyhow!(
                    "gRPC Interceptor '{}' not found in registry. \
                     Implement Interceptor<GrpcContext> and mark the provider with \
                     `#[interceptor(grpc)]` or a typed impl head.",
                    token
                )
            })
    }

    fn resolve_error_handlers(&self, tokens: Vec<String>) -> Result<Vec<GrpcErrorHandlerArc>> {
        let mut handlers = self.container.borrow().get_global_grpc_error_handlers();
        for token in tokens {
            handlers.push(self.resolve_error_handler_by_token(&token)?);
        }
        Ok(handlers)
    }

    fn resolve_error_handler_by_token(&self, token: &str) -> Result<GrpcErrorHandlerArc> {
        self.container
            .borrow()
            .get_role_registry()
            .grpc_error_handlers
            .get(token)
            .cloned()
            .ok_or_else(|| {
                anyhow!(
                    "gRPC ErrorHandler '{}' not found in registry. \
                     Implement ErrorHandler<GrpcContext, GrpcStatus> and mark the provider \
                     with `#[error_handler(grpc)]` or a typed impl head.",
                    token
                )
            })
    }
}
