use std::collections::HashMap;

use actix_web::{web::Bytes, HttpRequest as ActixHttpRequest, HttpResponse as ActixHttpResponse};
use anyhow::{anyhow, Result};
use serde_json::Value;

use toni::{Body, HttpRequest, HttpResponse, IntoResponse, RouteAdapter};

pub struct ActixRouteAdapter;

impl ActixRouteAdapter {
    async fn adapt_actix_request(req: ActixHttpRequest, body: Bytes) -> Result<HttpRequest> {
        // Parse body
        let body = if let Ok(body_str) = String::from_utf8(body.to_vec()) {
            if let Ok(json) = serde_json::from_str::<Value>(&body_str) {
                Body::Json(json)
            } else {
                Body::Text(body_str)
            }
        } else {
            Body::Text(String::from_utf8_lossy(&body).to_string())
        };

        // Extract path parameters
        let path_params: HashMap<String, String> = req
            .match_info()
            .iter()
            .map(|(k, v)| (k.to_string(), v.to_string()))
            .collect();

        // Extract query parameters
        let query_params: HashMap<String, String> = req
            .query_string()
            .split('&')
            .filter_map(|pair| {
                let mut parts = pair.split('=');
                let key = parts.next()?;
                let value = parts.next().unwrap_or("");
                Some((key.to_string(), value.to_string()))
            })
            .collect();

        // Extract headers
        let headers: Vec<(String, String)> = req
            .headers()
            .iter()
            .map(|(name, value)| (name.to_string(), value.to_str().unwrap_or("").to_string()))
            .collect();

        Ok(HttpRequest {
            body,
            headers,
            method: req.method().to_string(),
            uri: req.uri().to_string(),
            query_params,
            path_params,
        })
    }

    fn adapt_actix_response(
        response: Box<dyn IntoResponse<Response = HttpResponse>>,
    ) -> Result<ActixHttpResponse> {
        let response = response.to_response();

        let status = actix_web::http::StatusCode::from_u16(response.status)
            .unwrap_or(actix_web::http::StatusCode::INTERNAL_SERVER_ERROR);

        let mut actix_response = ActixHttpResponse::build(status);

        // Set headers
        for (key, value) in response.headers {
            actix_response.insert_header((key.as_str(), value.as_str()));
        }

        // Set body
        let actix_response = match response.body {
            Some(Body::Text(text)) => actix_response.content_type("text/plain").body(text),
            Some(Body::Json(json)) => {
                let json_str = serde_json::to_string(&json)
                    .map_err(|e| anyhow!("Failed to serialize JSON: {}", e))?;
                actix_response
                    .content_type("application/json")
                    .body(json_str)
            }
            None => actix_response.finish(),
        };

        Ok(actix_response)
    }
}

impl RouteAdapter for ActixRouteAdapter {
    type Request = (ActixHttpRequest, Bytes);
    type Response = ActixHttpResponse;

    async fn adapt_request(request: Self::Request) -> Result<HttpRequest> {
        Self::adapt_actix_request(request.0, request.1).await
    }

    fn adapt_response(
        response: Box<dyn IntoResponse<Response = HttpResponse>>,
    ) -> Result<Self::Response> {
        Self::adapt_actix_response(response)
    }
}
