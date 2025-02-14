use std::{collections::HashMap, str::FromStr};

use anyhow::{Result, anyhow};
use axum::{
    RequestPartsExt,
    body::to_bytes,
    extract::{Path, Query},
    http::{HeaderMap, HeaderName, HeaderValue, Request, Response, StatusCode}
};
use serde_json::Value;

use crate::http_helpers::{self, Body, HttpRequest, HttpResponse};

use super::RouteAdapter;

pub struct AxumRouteAdapter;

impl RouteAdapter for AxumRouteAdapter {
    type Request = Request<axum::body::Body>;
    type Response = Response<axum::body::Body>;

    async fn adapt_request(request: Self::Request) -> Result<HttpRequest> {
        let (mut parts, body) = request.into_parts();
        let body_bytes = to_bytes(body, usize::MAX).await?;
        let bytes = body_bytes.to_vec();

        let body = if let Ok(body_str) = String::from_utf8(bytes) {
            if let Ok(json) = serde_json::from_str::<Value>(&body_str) {
                Body::Json(json)
            } else {
                Body::Text(body_str)
            }
        } else {
            Body::Text(String::from_utf8_lossy(&body_bytes).to_string())
        };

        let Path(path_params) = parts
            .extract::<Path<HashMap<String, String>>>()
            .await
            .map_err(|e| anyhow!("Failed to extract path parameters: {:?}", e))?;

        let Query(query_params) = parts
            .extract::<Query<HashMap<String, String>>>()
            .await
            .map_err(|e| anyhow!("Failed to extract query parameters: {:?}", e))?;

        let headers = parts
            .headers
            .iter()
            .map(|(name, value)| (name.to_string(), value.to_str().unwrap_or("").to_string()))
            .collect();

        Ok(HttpRequest {
            body,
            headers,
            method: parts.method.to_string(),
            uri: parts.uri.to_string(),
            query_params,
            path_params,
        })
    }

    fn adapt_response(
        response: Box<dyn http_helpers::IntoResponse<Response = HttpResponse>>,
    ) -> Result<Self::Response> {
        let response = response.to_response();

        let status =
            StatusCode::from_u16(response.status).unwrap_or(StatusCode::INTERNAL_SERVER_ERROR);

        let mut body_is_json = false;

        let body = match response.body {
            Some(Body::Text(text)) => axum::body::Body::from(text),
            Some(Body::Json(json)) => {
                body_is_json = true;
                let vec = serde_json::to_vec(&json)
                    .map_err(|e| anyhow::anyhow!("Failed to serialize JSON: {}", e))?;
                axum::body::Body::from(vec)
            }
            _ => axum::body::Body::empty(),
        };

        let mut headers = HeaderMap::new();
        for (k, v) in &response.headers {
            if let Ok(header_name) = HeaderName::from_bytes(k.as_bytes()) {
                if let Ok(header_value) = HeaderValue::from_str(v) {
                    headers.insert(header_name, header_value);
                }
            }
        }

        let content_type = if body_is_json {
            "application/json"
        } else {
            "text/plain"
        };
        headers.insert(
            HeaderName::from_str("Content-Type")
                .map_err(|e| anyhow::anyhow!("Failed to parse header name Content-Type: {}", e))?,
            HeaderValue::from_static(content_type),
        );

        let mut res = Response::builder()
            .status(status)
            .body(body)
            .map_err(|e| anyhow::anyhow!("Failed to build response: {}", e))?;

        res.headers_mut().extend(headers);

        Ok(res)
    }
}
