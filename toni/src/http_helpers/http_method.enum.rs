#[derive(Debug)]
pub enum HttpMethod {
  GET,
  POST,
  PUT,
  DELETE,
  HEAD,
  PATCH,
  OPTIONS,
}

impl HttpMethod {
  pub fn from_string(method: &str) -> Option<Self> {
      match method.to_lowercase().as_str() {
          "get" => Some(HttpMethod::GET),
          "post" => Some(HttpMethod::POST),
          "put" => Some(HttpMethod::PUT),
          "delete" => Some(HttpMethod::DELETE),
          "patch" => Some(HttpMethod::PATCH),
          "options" => Some(HttpMethod::OPTIONS),
          "head" => Some(HttpMethod::HEAD),
          _ => None,
      }
  }
}