use anyhow::{anyhow, Context, Result};
use std::pin::Pin;
use std::sync::Arc;

use actix_web::{
    dev::Server, web, web::Bytes, App, HttpRequest as ActixHttpRequest,
    HttpResponse as ActixHttpResponse, HttpServer,
};
use toni::{
    AdapterContext, HttpAdapter, HttpMethod, HttpRequest, RequestHandler,
    RouteTableBuilder, http_helpers::RequestBody,
};


pub struct ActixAdapter {
    route_builder: RouteTableBuilder,
}

impl ActixAdapter {
    pub fn new() -> Self {
        Self {
            route_builder: RouteTableBuilder::new(),
        }
    }
}

impl Default for ActixAdapter {
    fn default() -> Self {
        Self::new()
    }
}

impl ActixAdapter {
    async fn adapt_request(request: (ActixHttpRequest, Bytes)) -> Result<HttpRequest> {
        let (req, body) = request;

        let method = req
            .method()
            .as_str()
            .parse::<http::Method>()
            .unwrap_or(http::Method::GET);
        let uri = req
            .uri()
            .to_string()
            .parse::<http::Uri>()
            .unwrap_or_else(|_| http::Uri::default());

        let mut builder = http::Request::builder().method(method).uri(uri);
        for (name, value) in req.headers().iter() {
            if let Ok(val) = http::HeaderValue::from_bytes(value.as_bytes()) {
                if let Ok(key) = http::HeaderName::from_bytes(name.as_str().as_bytes()) {
                    builder = builder.header(key, val);
                }
            }
        }
        let (http_parts, _) = builder.body(()).unwrap().into_parts();

        Ok(HttpRequest::from_parts(
            http_parts,
            RequestBody::Buffered(web::Bytes::from(body.to_vec())),
        ))
    }

    async fn adapt_response(response: HttpResponse) -> Result<ActixHttpResponse> {
        let status = actix_web::http::StatusCode::from_u16(response.status)
            .unwrap_or(actix_web::http::StatusCode::INTERNAL_SERVER_ERROR);

        let mut builder = ActixHttpResponse::build(status);

        let actix_response = match response.body {
            Some(toni_body) => {
                let ct = toni_body
                    .content_type()
                    .unwrap_or("application/octet-stream")
                    .to_string();
                let bytes = {
                    use http_body_util::BodyExt;
                    toni_body
                        .into_box_body()
                        .collect()
                        .await
                        .map(|c| c.to_bytes())
                        .unwrap_or_default()
                };
                builder.content_type(ct.as_str()).body(bytes.to_vec())
            }
            None => builder.finish(),
        };

        let mut actix_response = actix_response;
        for (key, value) in response.headers {
            actix_response.headers_mut().insert(
                actix_web::http::header::HeaderName::from_bytes(key.as_bytes())
                    .map_err(|e| anyhow!("Failed to parse header name: {}", e))?,
                actix_web::http::header::HeaderValue::from_str(&value)
                    .map_err(|e| anyhow!("Failed to parse header value: {}", e))?,
            );
        }

        Ok(actix_response)
    }
}

impl HttpAdapter for ActixAdapter {
    fn bind(&mut self, method: HttpMethod, path: &str, handler: Arc<dyn RequestHandler>) -> Result<()> {
        self.route_builder.insert(method, path, handler);
        Ok(())
    }

    fn create(
        &mut self,
        port: u16,
        hostname: &str,
        ctx: AdapterContext,
    ) -> Result<Pin<Box<dyn std::future::Future<Output = ()> + Send + 'static>>> {
        let addr = format!("{}:{}", hostname, port);
        let builder = std::mem::replace(&mut self.route_builder, RouteTableBuilder::new());
        let table = Arc::new(builder.build());
        let ctx = Arc::new(ctx);

        let server: Server = HttpServer::new(move || {
            let table = table.clone();
            let ctx = ctx.clone();
            App::new().default_service(web::to(move |req: ActixHttpRequest, body: Bytes| {
                let table = table.clone();
                let ctx = ctx.clone();
                async move {
                    let http_req = match Self::adapt_request((req, body)).await {
                        Ok(r) => r,
                        Err(_) => return ActixHttpResponse::InternalServerError().finish(),
                    };
                    let http_res = ctx.execute(http_req, move |req| {
                        let table = table.clone();
                        Box::pin(async move { table.dispatch(req).await })
                    }).await;
                    Self::adapt_response(http_res).await.unwrap_or_else(|_| {
                        ActixHttpResponse::InternalServerError().finish()
                    })
                }
            }))
        })
        .bind(&addr)
        .with_context(|| format!("Failed to bind to {}", addr))?
        .run();

        Ok(Box::pin(async move {
            if let Err(e) = server
                .await
                .with_context(|| "Actix server encountered an error")
            {
                tracing::error!(error = %e, "Actix server error");
                std::process::exit(1);
            }
        }))
    }
}
