use async_trait::async_trait;
use std::sync::Arc;

use crate::{
    http_helpers::{HttpRequest, HttpResponse},
    traits_helpers::middleware::{Middleware, MiddlewareResult, Next},
};

pub struct FinalHandler {
    handler: Arc<
        dyn Fn(
                HttpRequest,
            )
                -> std::pin::Pin<Box<dyn std::future::Future<Output = HttpResponse> + Send>>
            + Send
            + Sync,
    >,
}

impl FinalHandler {
    pub fn new<F>(handler: F) -> Self
    where
        F: Fn(
                HttpRequest,
            )
                -> std::pin::Pin<Box<dyn std::future::Future<Output = HttpResponse> + Send>>
            + Send
            + Sync
            + 'static,
    {
        Self {
            handler: Arc::new(handler),
        }
    }
}

#[async_trait]
impl Next for FinalHandler {
    async fn run(self: Box<Self>, req: HttpRequest) -> MiddlewareResult {
        let response = (self.handler)(req).await;
        Ok(response)
    }
}

pub struct ChainLink {
    middleware: Arc<dyn Middleware>,
    next: Box<dyn Next>,
}

impl ChainLink {
    pub fn new(middleware: Arc<dyn Middleware>, next: Box<dyn Next>) -> Self {
        Self { middleware, next }
    }
}

#[async_trait]
impl Next for ChainLink {
    async fn run(self: Box<Self>, req: HttpRequest) -> MiddlewareResult {
        self.middleware.handle(req, self.next).await
    }
}

pub struct MiddlewareChain {
    middleware_stack: Vec<Arc<dyn Middleware>>,
}

impl MiddlewareChain {
    pub fn new() -> Self {
        Self {
            middleware_stack: Vec::new(),
        }
    }

    pub fn use_middleware(&mut self, middleware: Arc<dyn Middleware>) {
        self.middleware_stack.push(middleware);
    }

    pub async fn execute<F>(&self, req: HttpRequest, final_handler: F) -> MiddlewareResult
    where
        F: Fn(
                HttpRequest,
            )
                -> std::pin::Pin<Box<dyn std::future::Future<Output = HttpResponse> + Send>>
            + Send
            + Sync
            + 'static,
    {
        let mut next: Box<dyn Next> = Box::new(FinalHandler::new(final_handler));

        for middleware in self.middleware_stack.iter().rev() {
            next = Box::new(ChainLink::new(middleware.clone(), next));
        }

        next.run(req).await
    }

    pub fn len(&self) -> usize {
        self.middleware_stack.len()
    }

    pub fn is_empty(&self) -> bool {
        self.middleware_stack.is_empty()
    }
}

impl Default for MiddlewareChain {
    fn default() -> Self {
        Self::new()
    }
}
