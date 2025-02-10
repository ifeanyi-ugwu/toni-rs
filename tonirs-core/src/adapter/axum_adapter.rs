use anyhow::{Context, Result};
use std::sync::Arc;
use tokio::net::TcpListener;

use crate::{http_adapter::HttpAdapter, http_helpers::HttpMethod, traits_helpers::ControllerTrait};
use axum::{
    Router,
    body::Body,
    http::Request,
    routing::{delete, get, head, options, patch, post, put},
};

use super::{AxumRouteAdapter, RouteAdapter};

#[derive(Clone)]
pub struct AxumAdapter {
    instance: Router,
}

impl HttpAdapter for AxumAdapter {
    fn new() -> Self {
        Self {
            instance: Router::new(),
        }
    }

    fn add_route(
        &mut self,
        path: &str,
        method: HttpMethod,
        handler: Arc<Box<dyn ControllerTrait>>,
    ) {
        let route_handler = move |req: Request<Body>| {
            let handler = handler.clone();
            Box::pin(async move {
                AxumRouteAdapter::handle_request(req, handler)
                    .await
                    .unwrap()
            })
        };
        println!("Adding route: {} {:?}", path, method);

        self.instance = match method {
            HttpMethod::GET => self.instance.clone().route(path, get(route_handler)),
            HttpMethod::POST => self.instance.clone().route(path, post(route_handler)),
            HttpMethod::PUT => self.instance.clone().route(path, put(route_handler)),
            HttpMethod::DELETE => self.instance.clone().route(path, delete(route_handler)),
            HttpMethod::HEAD => self.instance.clone().route(path, head(route_handler)),
            HttpMethod::PATCH => self.instance.clone().route(path, patch(route_handler)),
            HttpMethod::OPTIONS => self.instance.clone().route(path, options(route_handler)),
        };
    }

    async fn listen(self, port: u16, hostname: &str) -> Result<()> {
        let addr = format!("{}:{}", hostname, port);
        let listener: TcpListener = TcpListener::bind(&addr)
            .await
            .unwrap_or_else(|_| panic!("Failed to bind to address {}", addr));

        println!("Listening on {}", addr);

        axum::serve(listener, self.instance)
            .await
            .with_context(|| "Axum server encountered an error")?;
        Ok(())
    }
}
