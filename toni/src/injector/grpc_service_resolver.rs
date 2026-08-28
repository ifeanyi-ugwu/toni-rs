use std::cell::RefCell;
use std::collections::HashMap;
use std::rc::Rc;

use anyhow::{Result, anyhow};

use crate::adapter::{GrpcServiceSource, ResolvedGrpcEnhancers};
use crate::traits_helpers::{GrpcErrorHandlerArc, GrpcGuardEntry, GrpcInterceptorEntry};

use super::ToniContainer;

/// Resolves one gRPC service's enhancer bundle from the role registry by token.
/// Mirrors [`RpcControllerResolver`](super::RpcControllerResolver) — called by the instance
/// loader while services are stored, so a misdeclared token fails `create()`. Bind hands the
/// stored `(service, enhancers)` pair to the adapter, which forwards `enhancers` into
/// [`GrpcServiceSource::register_with`].
pub struct GrpcServiceResolver {
    container: Rc<RefCell<ToniContainer>>,
}

impl GrpcServiceResolver {
    pub fn new(container: Rc<RefCell<ToniContainer>>) -> Self {
        Self { container }
    }

    pub(crate) fn resolve_for(&self, svc: &dyn GrpcServiceSource) -> Result<ResolvedGrpcEnhancers> {
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
                    "gRPC Guard '{}' not found in registry. A guard registers automatically by \
                     implementing Guard<GrpcContext>; make sure the provider is in the module's \
                     `providers` list. For `provider_factory!` under a string/const token, name \
                     the produced type so it can be detected — annotate the closure's return type \
                     (`|| -> MyGuard`) or pass a type hint.",
                    token
                )
            })
    }

    fn resolve_interceptors(&self, tokens: Vec<String>) -> Result<Vec<GrpcInterceptorEntry>> {
        let mut interceptors = self.container.borrow().get_global_grpc_interceptors();
        for token in tokens {
            let entry = self.resolve_interceptor_by_token(&token)?;
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
                    "gRPC Interceptor '{}' not found in registry. An interceptor registers \
                     automatically by implementing Interceptor<GrpcContext>; make sure the \
                     provider is in the module's `providers` list. For `provider_factory!` under a \
                     string/const token, name the produced type so it can be detected — annotate \
                     the closure's return type (`|| -> MyInterceptor`) or pass a type hint.",
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
                    "gRPC ErrorHandler '{}' not found in registry. An error handler registers \
                     automatically by implementing ErrorHandler<GrpcContext, GrpcStatus>; make \
                     sure the provider is in the module's `providers` list. For `provider_factory!` \
                     under a string/const token, name the produced type so it can be detected — \
                     annotate the closure's return type or pass a type hint.",
                    token
                )
            })
    }
}
