use bytes::Bytes;
use http_body_util::BodyExt;

use super::{BodyExtractionError, FromContext, take_body};
use crate::context::HttpContext;
use crate::http_helpers::{HttpRequest, RequestBody, RequestBoxBody};

pub use multer::Field;

/// Extracts a `multipart/form-data` body, yielding fields one at a time.
///
/// Each call to [`next_field`](Multipart::next_field) advances to the next
/// part. Fields can be plain text values or file uploads — use
/// [`Field::name`], [`Field::file_name`], [`Field::bytes`], and
/// [`Field::text`] to inspect them.
///
/// Only one body-consuming extractor may appear per handler.
///
/// # Example
///
/// ```rust,ignore
/// use toni::extractors::Multipart;
///
/// #[post("/upload")]
/// async fn upload(&self, mut mp: Multipart) -> String {
///     while let Some(field) = mp.next_field().await.unwrap() {
///         let name = field.name().unwrap_or("unknown").to_string();
///         let data = field.bytes().await.unwrap();
///         println!("field={name} size={}", data.len());
///     }
///     "ok".into()
/// }
/// ```
pub struct Multipart(multer::Multipart<'static>);

#[derive(Debug)]
pub enum MultipartError {
    MissingBoundary,
    Parse(multer::Error),
}

impl std::fmt::Display for MultipartError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            MultipartError::MissingBoundary => {
                write!(f, "missing multipart boundary in Content-Type")
            }
            MultipartError::Parse(e) => write!(f, "multipart parse error: {e}"),
        }
    }
}

impl std::error::Error for MultipartError {}

impl Multipart {
    pub async fn next_field(&mut self) -> Result<Option<multer::Field<'static>>, MultipartError> {
        self.0.next_field().await.map_err(MultipartError::Parse)
    }
}

fn body_into_stream(
    body: RequestBoxBody,
) -> impl futures::Stream<Item = Result<Bytes, Box<dyn std::error::Error + Send + Sync>>> + Send + 'static
{
    futures::stream::unfold(body, |mut body| async move {
        match body.frame().await {
            Some(Ok(frame)) => {
                if let Ok(data) = frame.into_data() {
                    Some((Ok(data), body))
                } else {
                    // trailers frame — not data, signal end
                    None
                }
            }
            Some(Err(e)) => Some((Err(e), body)),
            None => None,
        }
    })
}

impl FromContext<HttpContext> for Multipart {
    type Error = BodyExtractionError<MultipartError>;

    const CONSUMES: bool = true;

    async fn extract(ctx: &HttpContext) -> Result<Self, Self::Error> {
        read(take_body::<Self>(ctx)?)
            .await
            .map_err(BodyExtractionError::Extract)
    }
}

async fn read(req: HttpRequest) -> Result<Multipart, MultipartError> {
    let (parts, body) = req.into_parts();

    let boundary = parts
        .headers
        .get(http::header::CONTENT_TYPE)
        .and_then(|v| v.to_str().ok())
        .and_then(|ct| multer::parse_boundary(ct).ok())
        .ok_or(MultipartError::MissingBoundary)?;

    let box_body: RequestBoxBody = match body {
        RequestBody::Streaming(s) => s,
        RequestBody::Buffered(b) => http_body_util::Full::new(b)
            .map_err(|never: std::convert::Infallible| match never {})
            .boxed_unsync(),
    };

    Ok(Multipart(multer::Multipart::new(
        body_into_stream(box_body),
        boundary,
    )))
}
