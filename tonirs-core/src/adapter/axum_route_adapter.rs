use std::{collections::HashMap, str::FromStr};

use axum::{
    RequestPartsExt,
    body::to_bytes,
    extract::{Path, Query},
    http::{HeaderMap, HeaderName, HeaderValue, Request, Response, StatusCode},
    response::IntoResponse,
};
use serde_json::Value;

use crate::http_helpers::{self, Body, HttpRequest, HttpResponse};

use super::RouteAdapter;

pub struct AxumRouteAdapter;

impl RouteAdapter for AxumRouteAdapter {
    type Request = Request<axum::body::Body>;
    type Response = Response<axum::body::Body>;

    async fn adapt_request(request: Self::Request) -> HttpRequest {
        let (mut parts, body) = request.into_parts();
        let body_bytes: Vec<u8> = to_bytes(body, usize::MAX).await.unwrap().to_vec();

        let body = if let Ok(body_str) = String::from_utf8(body_bytes.to_vec()) {
            if let Ok(json) = serde_json::from_str::<Value>(&body_str) {
                Body::Json(json)
            } else {
                Body::Text(body_str)
            }
        } else {
            Body::Text(String::from_utf8_lossy(&body_bytes).to_string())
        };

        let path_params = parts
            .extract::<Path<HashMap<String, String>>>()
            .await
            .map(|Path(path_params)| path_params)
            .map_err(|err| err.into_response())
            .unwrap();

        let query_params = parts
            .extract::<Query<HashMap<String, String>>>()
            .await
            .map(|Query(params)| params)
            .map_err(|err| err.into_response())
            .unwrap();

        let headers = parts
            .headers
            .iter()
            .map(|(name, value)| (name.to_string(), value.to_str().unwrap_or("").to_string()))
            .collect();

        HttpRequest {
            body,
            headers,
            method: parts.method.to_string(),
            uri: parts.uri.to_string(),
            query_params,
            path_params,
        }
    }

    fn adapt_response(
        response: Box<dyn http_helpers::IntoResponse<Response = HttpResponse>>,
    ) -> Self::Response {
        let response = response.into_response();

        let status =
            StatusCode::from_u16(response.status).unwrap_or(StatusCode::INTERNAL_SERVER_ERROR);

        let mut body_is_json = false;

        let body = match response.body {
            Some(Body::Text(text)) => axum::body::Body::from(text),
            Some(Body::Json(json)) => {
                body_is_json = true;
                axum::body::Body::from(serde_json::to_vec(&json).unwrap())
            }
            _ => axum::body::Body::empty(),
        };

        let mut headers = HeaderMap::new();
        for (k, v) in &response.headers {
            if let Ok(header_name) = HeaderName::from_bytes(k.as_bytes()) {
                if let Ok(header_value) = HeaderValue::from_str(&v) {
                    headers.insert(header_name, header_value);
                }
            }
        }

        if !body_is_json {
            headers.insert(
                HeaderName::from_str("Content-Type").unwrap(),
                HeaderValue::from_static("text/plain"),
            );
        } else {
            headers.insert(
                HeaderName::from_str("Content-Type").unwrap(),
                HeaderValue::from_static("application/json"),
            );
        }

        let mut res = Response::builder().status(status).body(body).unwrap();

        res.headers_mut().extend(headers);

        res
    }
}
