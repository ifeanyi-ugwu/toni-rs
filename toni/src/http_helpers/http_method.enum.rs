#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum HttpMethod {
    GET,
    POST,
    PUT,
    DELETE,
    HEAD,
    PATCH,
    OPTIONS,
    TRACE,
    CONNECT,
}

impl HttpMethod {
    /// Parse an HTTP method from a string (case-insensitive)
    pub fn from_string(method: &str) -> Option<Self> {
        match method.to_lowercase().as_str() {
            "get" => Some(HttpMethod::GET),
            "post" => Some(HttpMethod::POST),
            "put" => Some(HttpMethod::PUT),
            "delete" => Some(HttpMethod::DELETE),
            "patch" => Some(HttpMethod::PATCH),
            "options" => Some(HttpMethod::OPTIONS),
            "head" => Some(HttpMethod::HEAD),
            "trace" => Some(HttpMethod::TRACE),
            "connect" => Some(HttpMethod::CONNECT),
            _ => None,
        }
    }

    /// Convert to uppercase string representation
    pub fn as_str(&self) -> &'static str {
        match self {
            HttpMethod::GET => "GET",
            HttpMethod::POST => "POST",
            HttpMethod::PUT => "PUT",
            HttpMethod::DELETE => "DELETE",
            HttpMethod::PATCH => "PATCH",
            HttpMethod::HEAD => "HEAD",
            HttpMethod::OPTIONS => "OPTIONS",
            HttpMethod::TRACE => "TRACE",
            HttpMethod::CONNECT => "CONNECT",
        }
    }
}

impl From<HttpMethod> for String {
    fn from(method: HttpMethod) -> Self {
        method.as_str().to_string()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_from_string() {
        assert_eq!(HttpMethod::from_string("get"), Some(HttpMethod::GET));
        assert_eq!(HttpMethod::from_string("GET"), Some(HttpMethod::GET));
        assert_eq!(HttpMethod::from_string("Post"), Some(HttpMethod::POST));
        assert_eq!(HttpMethod::from_string("DELETE"), Some(HttpMethod::DELETE));
        assert_eq!(HttpMethod::from_string("invalid"), None);
    }

    #[test]
    fn test_as_str() {
        assert_eq!(HttpMethod::GET.as_str(), "GET");
        assert_eq!(HttpMethod::POST.as_str(), "POST");
        assert_eq!(HttpMethod::PUT.as_str(), "PUT");
        assert_eq!(HttpMethod::DELETE.as_str(), "DELETE");
        assert_eq!(HttpMethod::PATCH.as_str(), "PATCH");
        assert_eq!(HttpMethod::HEAD.as_str(), "HEAD");
        assert_eq!(HttpMethod::OPTIONS.as_str(), "OPTIONS");
        assert_eq!(HttpMethod::TRACE.as_str(), "TRACE");
        assert_eq!(HttpMethod::CONNECT.as_str(), "CONNECT");
    }

    #[test]
    fn test_to_string() {
        let method: String = HttpMethod::POST.into();
        assert_eq!(method, "POST");

        let method: String = HttpMethod::GET.into();
        assert_eq!(method, "GET");
    }

    #[test]
    fn test_clone_and_copy() {
        let method1 = HttpMethod::GET;
        let method2 = method1; // Copy
        let method3 = method1.clone(); // Clone

        assert_eq!(method1, method2);
        assert_eq!(method1, method3);
    }

    #[test]
    fn test_eq() {
        assert_eq!(HttpMethod::GET, HttpMethod::GET);
        assert_ne!(HttpMethod::GET, HttpMethod::POST);
    }
}
