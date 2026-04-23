use super::FromRequest;
use crate::http_helpers::HttpRequest;

/// Extracts the raw request body as bytes.
///
/// For structured data, use [`Json`](super::json::Json) or
/// [`Body`](super::body::Body) instead.
///
/// # Example
///
/// ```rust,ignore
/// #[post("/upload")]
/// fn upload(&self, Bytes(data): Bytes) -> String {
///     format!("Uploaded {} bytes", data.len())
/// }
/// ```
#[derive(Debug, Clone)]
pub struct Bytes(pub Vec<u8>);

impl Bytes {
    pub fn into_inner(self) -> Vec<u8> {
        self.0
    }

    pub fn as_slice(&self) -> &[u8] {
        &self.0
    }
}

impl std::ops::Deref for Bytes {
    type Target = Vec<u8>;
    fn deref(&self) -> &Self::Target {
        &self.0
    }
}

impl std::ops::DerefMut for Bytes {
    fn deref_mut(&mut self) -> &mut Self::Target {
        &mut self.0
    }
}

#[derive(Debug)]
pub enum BytesError {
    MissingBody,
    CollectError(String),
}

impl std::fmt::Display for BytesError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            BytesError::MissingBody => write!(f, "No body provided"),
            BytesError::CollectError(msg) => write!(f, "Failed to read request body: {}", msg),
        }
    }
}

impl std::error::Error for BytesError {}

impl FromRequest for Bytes {
    type Error = BytesError;

    async fn from_request(req: HttpRequest) -> Result<Self, Self::Error> {
        let (_, body) = req.into_parts();
        let bytes = body
            .collect()
            .await
            .map_err(|e| BytesError::CollectError(e.to_string()))?;
        Ok(Bytes(bytes.to_vec()))
    }
}
