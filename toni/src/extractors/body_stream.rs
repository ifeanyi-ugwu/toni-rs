use std::convert::Infallible;

use bytes::Bytes;
use http_body_util::BodyExt;

use super::{BodyExtractionError, FromContext, take_body};
use crate::context::HttpContext;
use crate::http_helpers::{HttpRequest, RequestBody, RequestBoxBody};

/// Extracts the request body as a raw, unbuffered stream.
///
/// Use this when you need to process large uploads without loading the entire
/// body into memory. Only one body extractor may appear per handler — the body
/// is single-use.
///
/// # Example
///
/// ```rust,ignore
/// use toni::BodyStream;
/// use futures::StreamExt;
///
/// #[post("/upload")]
/// async fn upload(&self, stream: BodyStream) -> ToniBody {
///     let mut total = 0usize;
///     let mut s = stream.into_stream();
///     while let Some(chunk) = s.next().await {
///         total += chunk.unwrap().len();
///     }
///     ToniBody::text(format!("received {} bytes", total))
/// }
/// ```
pub struct BodyStream(pub(crate) RequestBoxBody);

impl BodyStream {
    /// Consume into a [`futures::Stream`] of `Bytes` chunks.
    pub fn into_stream(
        self,
    ) -> impl futures::Stream<Item = Result<Bytes, Box<dyn std::error::Error + Send + Sync>>> {
        futures::stream::unfold(self.0, |mut body| async move {
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

    /// Buffer the entire stream into [`Bytes`].
    pub async fn collect(self) -> Result<Bytes, Box<dyn std::error::Error + Send + Sync>> {
        let collected = self.0.collect().await?;
        Ok(collected.to_bytes())
    }
}

/// Fallible only in the way every body extractor is: the body may already have
/// been read. A buffered body is wrapped rather than rejected, so there is
/// nothing else to fail at.
impl FromContext<HttpContext> for BodyStream {
    type Error = BodyExtractionError<Infallible>;

    async fn extract(ctx: &HttpContext) -> Result<Self, Self::Error> {
        Ok(read(take_body::<Self>(ctx)?))
    }
}

fn read(req: HttpRequest) -> BodyStream {
    let (_, body) = req.into_parts();
    match body {
        RequestBody::Streaming(s) => BodyStream(s),
        RequestBody::Buffered(b) => {
            use http_body_util::{BodyExt as _, Full};
            let box_body = Full::new(b)
                .map_err(|never: Infallible| match never {})
                .boxed_unsync();
            BodyStream(box_body)
        }
    }
}
