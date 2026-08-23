use super::FromContext;
use crate::context::HttpContext;
use crate::http_helpers::{PathParams, RequestPart};
use serde::de::DeserializeOwned;
use std::str::FromStr;

/// Extracts typed path parameters from the URL.
///
/// # Example
///
/// ```rust,ignore
/// #[get("/users/{id}")]
/// fn get_user(&self, Path(id): Path<i32>) -> String {
///     format!("User {}", id)
/// }
/// ```
#[derive(Debug, Clone)]
pub struct Path<T>(pub T);

impl<T> Path<T> {
    pub fn into_inner(self) -> T {
        self.0
    }
}

impl<T> std::ops::Deref for Path<T> {
    type Target = T;
    fn deref(&self) -> &Self::Target {
        &self.0
    }
}

impl<T> std::ops::DerefMut for Path<T> {
    fn deref_mut(&mut self) -> &mut Self::Target {
        &mut self.0
    }
}

#[derive(Debug)]
pub enum PathError {
    NotFound(String),
    ParseError(String),
}

impl std::fmt::Display for PathError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            PathError::NotFound(name) => write!(f, "Path parameter '{}' not found", name),
            PathError::ParseError(msg) => write!(f, "Failed to parse path parameter: {}", msg),
        }
    }
}

impl std::error::Error for PathError {}

pub fn extract_path_param<T: FromStr>(parts: &RequestPart, name: &str) -> Result<T, PathError>
where
    T::Err: std::fmt::Display,
{
    let params = parts.extensions.get::<PathParams>();
    let value = params
        .and_then(|p| p.0.get(name))
        .ok_or_else(|| PathError::NotFound(name.to_string()))?;

    value
        .parse::<T>()
        .map_err(|e| PathError::ParseError(format!("{}: {}", name, e)))
}

impl<T: DeserializeOwned> FromContext<HttpContext> for Path<T> {
    type Error = PathError;

    async fn extract(ctx: &HttpContext) -> Result<Self, Self::Error> {
        let empty = std::collections::HashMap::new();
        let params = ctx
            .request()
            .extensions
            .get::<PathParams>()
            .map_or(&empty, |p| &p.0);

        // Structs (e.g. `Path<MyParams>`): urlencoded round-trip, the same
        // deserializer Query uses. Unlike a serde_json::Value round-trip it
        // forwards T's type hints, so numeric and bool fields parse from the
        // raw string values.
        let encoded = serde_urlencoded::to_string(params)
            .map_err(|e| PathError::ParseError(format!("Failed to encode path params: {}", e)))?;
        if let Ok(v) = serde_urlencoded::from_str::<T>(&encoded) {
            return Ok(Path(v));
        }

        // Bare scalars from a single param: urlencoded is map-shaped and cannot
        // produce them. String forms first so `Path<String>` receives "42" or
        // "true" verbatim; JSON re-lexing after that covers numbers and bools.
        if params.len() == 1 {
            let raw = params.values().next().unwrap();
            if let Ok(v) = serde_json::from_value::<T>(serde_json::Value::String(raw.clone())) {
                return Ok(Path(v));
            }
            if let Ok(v) = serde_json::from_str::<T>(raw) {
                return Ok(Path(v));
            }
        }

        Err(PathError::ParseError(format!(
            "Failed to deserialize path params from {:?}",
            params
        )))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde::Deserialize;
    use std::collections::HashMap;

    fn ctx_with(params: &[(&str, &str)]) -> HttpContext {
        let (mut parts, _) = http::Request::builder()
            .uri("/")
            .body(())
            .unwrap()
            .into_parts();
        let map: HashMap<String, String> = params
            .iter()
            .map(|(k, v)| (k.to_string(), v.to_string()))
            .collect();
        parts.extensions.insert(PathParams(map));
        HttpContext::from_parts(parts)
    }

    #[tokio::test]
    async fn scalar_i32() {
        let ctx = ctx_with(&[("id", "42")]);
        let Path(id) = Path::<i32>::extract(&ctx).await.unwrap();
        assert_eq!(id, 42);
    }

    #[tokio::test]
    async fn scalar_bool() {
        let ctx = ctx_with(&[("flag", "true")]);
        let Path(flag) = Path::<bool>::extract(&ctx).await.unwrap();
        assert!(flag);
    }

    #[tokio::test]
    async fn scalar_string_stays_verbatim() {
        // Values that lex as JSON scalars must not be re-typed for Path<String>.
        for raw in ["42", "true", "007"] {
            let ctx = ctx_with(&[("v", raw)]);
            let Path(v) = Path::<String>::extract(&ctx).await.unwrap();
            assert_eq!(v, raw);
        }
    }

    #[tokio::test]
    async fn scalar_parse_failure_is_an_error() {
        let ctx = ctx_with(&[("id", "abc")]);
        assert!(Path::<i32>::extract(&ctx).await.is_err());
    }

    #[derive(Debug, Deserialize)]
    struct Mixed {
        id: i32,
        name: String,
    }

    #[tokio::test]
    async fn struct_with_typed_fields() {
        let ctx = ctx_with(&[("id", "42"), ("name", "alice")]);
        let Path(m) = Path::<Mixed>::extract(&ctx).await.unwrap();
        assert_eq!(m.id, 42);
        assert_eq!(m.name, "alice");
    }

    #[derive(Debug, Deserialize)]
    struct Code {
        code: String,
    }

    #[tokio::test]
    async fn struct_string_field_keeps_leading_zeros() {
        let ctx = ctx_with(&[("code", "007")]);
        let Path(c) = Path::<Code>::extract(&ctx).await.unwrap();
        assert_eq!(c.code, "007");
    }

    #[tokio::test]
    async fn value_with_spaces_round_trips() {
        let ctx = ctx_with(&[("name", "a b c")]);
        let Path(v) = Path::<String>::extract(&ctx).await.unwrap();
        assert_eq!(v, "a b c");
    }

    #[derive(Debug, Deserialize)]
    struct AllOptional {
        id: Option<i32>,
    }

    #[tokio::test]
    async fn missing_params_extension_deserializes_optionals() {
        let (parts, _) = http::Request::builder()
            .uri("/")
            .body(())
            .unwrap()
            .into_parts();
        let ctx = HttpContext::from_parts(parts);
        let Path(v) = Path::<AllOptional>::extract(&ctx).await.unwrap();
        assert_eq!(v.id, None);
    }
}
