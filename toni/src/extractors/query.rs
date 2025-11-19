//! Query parameter extractor

use serde::de::DeserializeOwned;

use super::FromRequest;
use crate::http_helpers::HttpRequest;

/// Extractor for query parameters
///
/// # Example
///
/// ```rust,ignore
/// #[derive(Deserialize)]
/// struct SearchParams {
///     q: String,
///     limit: Option<i32>,
/// }
///
/// #[get("/search")]
/// fn search(&self, Query(params): Query<SearchParams>) -> String {
///     format!("Searching for: {}", params.q)
/// }
/// ```
#[derive(Debug, Clone)]
pub struct Query<T>(pub T);

impl<T> Query<T> {
    /// Extract the inner value
    pub fn into_inner(self) -> T {
        self.0
    }
}

impl<T> std::ops::Deref for Query<T> {
    type Target = T;

    fn deref(&self) -> &Self::Target {
        &self.0
    }
}

impl<T> std::ops::DerefMut for Query<T> {
    fn deref_mut(&mut self) -> &mut Self::Target {
        &mut self.0
    }
}

/// Error type for query extraction
#[derive(Debug)]
pub enum QueryError {
    /// Failed to deserialize query parameters
    DeserializeError(String),
}

impl std::fmt::Display for QueryError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            QueryError::DeserializeError(msg) => {
                write!(f, "Failed to deserialize query parameters: {}", msg)
            }
        }
    }
}

impl std::error::Error for QueryError {}

impl<T: DeserializeOwned> FromRequest for Query<T> {
    type Error = QueryError;

    fn from_request(req: &HttpRequest) -> Result<Self, Self::Error> {
        // Convert query_params HashMap to a format serde can deserialize
        let query_string: String = req
            .query_params
            .iter()
            .map(|(k, v)| format!("{}={}", k, v))
            .collect::<Vec<_>>()
            .join("&");

        let value: T = serde_urlencoded::from_str(&query_string)
            .map_err(|e| QueryError::DeserializeError(e.to_string()))?;

        Ok(Query(value))
    }
}
