use serde_json::Value;

use super::{Body, HttpResponse};

pub trait IntoResponse {
  type Response;
  
  fn into_response(&self) -> Self::Response;
}

impl IntoResponse for HttpResponse {
  type Response = Self;
  
  fn into_response(&self) -> Self {
      self.clone()
  }
}

impl IntoResponse for Body {
    type Response = HttpResponse;

    fn into_response(&self) -> Self::Response {
        HttpResponse {
            body: Some(self.clone()),
            ..HttpResponse::new()
        }
    }
}

impl IntoResponse for u16 {
    type Response = HttpResponse;

    fn into_response(&self) -> Self::Response {
        HttpResponse {
            status: self.clone(),
            ..HttpResponse::new()
        }
    }
}

impl IntoResponse for Vec<(String, String)> {
    type Response = HttpResponse;

    fn into_response(&self) -> Self::Response {
        HttpResponse {
            headers: self.clone(),
            ..HttpResponse::new()
        }
    }
}


impl IntoResponse for (u16, Body) {
  type Response = HttpResponse;

  fn into_response(&self) -> Self::Response {
      HttpResponse {
          body: Some(self.1.clone()),
          status: self.0.clone(),
          ..HttpResponse::new()
      }
  }
}

// impl<T1, T2> IntoResponse for (T1, T2)
// where
//     T1: IntoResponse<Response = HttpResponse>,
//     T2: IntoResponse<Response = HttpResponse>,
// {
//     type Response = HttpResponse;
    
//     fn into_response(&self) -> HttpResponse {
//         let mut response = self.0.into_response();
//         let part = self.1.into_response();
        
//         response.status = part.status;
//         response.headers.extend(part.headers);
//         response.body = part.body;
        
//         response
//     }
// }

impl IntoResponse for Value {
    type Response = HttpResponse;

    fn into_response(&self) -> Self::Response {
        HttpResponse {
            body: Some(Body::Json(self.clone())),
            headers: vec![("Content-Type".to_string(), "application/json".to_string())],
            ..HttpResponse::new()
        }
    }
}

impl IntoResponse for String {
    type Response = HttpResponse;

    fn into_response(&self) -> Self::Response {
        HttpResponse {
            body: Some(Body::Text(self.clone())),
            ..HttpResponse::new()
        }
    }
}

impl IntoResponse for &'static str {
    type Response = HttpResponse;
    
    fn into_response(&self) -> Self::Response {
        HttpResponse {
            body: Some(Body::Text(self.to_string())),
            ..HttpResponse::new()
        }
    }
}