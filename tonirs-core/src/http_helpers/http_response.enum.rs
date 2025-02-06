use serde_json::Value;

use super::{Body, IntoResponse};

#[derive(Debug)]
pub struct HttpResponseFull {
    body: Option<Body>,
    status: Option<u16>,
    headers: Option<Vec<(String, String)>>,
    // Binary(Vec<u8>),
    // Text(String),
    // Json(Value),
}

#[derive(Debug)]
pub struct HttpResponseBody {
    body: Option<Body>,
}

#[derive(Debug)]
pub struct HttpResponseStatus {
    status: Option<u16>,
}

#[derive(Debug)]
pub struct HttpResponseHeaders {
    headers: Option<Vec<(String, String)>>,
}

#[derive(Debug)]
pub struct HttpResponseDefault {
    pub body: Option<Body>,
    pub status: Option<u16>
}

// pub enum HttpResponse {
//     Full(HttpResponseFull),
//     Body(HttpResponseBody),
//     Status(HttpResponseStatus),
//     Headers(HttpResponseHeaders),
//     Default(HttpResponseDefault),
// }

#[derive(Debug, Clone)]
pub struct HttpResponse {
    pub body: Option<Body>,
    pub status: u16,
    pub headers: Vec<(String, String)>,
}
impl HttpResponse {
    pub fn new() -> Self {
        Self { 
            body: None,
            status: 200,
            headers: vec![],
        }
    }

    // pub fn from_parts(status: u16, headers: Vec<(String, String)>, body: Option<Body>) -> Self {
    //     Self {
    //         body,
    //         status,
    //         headers,
    //     }
    // }
}
