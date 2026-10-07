//! `#[ulo_grpc::method]` on a handler of each call shape, with the parameter kinds and the reply
//! forms.

use futures_util::{Stream, StreamExt, stream};
use tonic::Status;
use ulo::{injectable, routes};
use ulo_grpc::{GrpcCx, GrpcMetadata, Message, Request, Response, Streaming};

use crate::derives::Refusal;
use crate::enhancers::{Allow, Pass, Relay, Tag};
use crate::lints::{self, Number};

#[injectable]
pub struct LintsService;

#[routes]
#[guards(grpc = Allow)]
#[interceptors(value = Pass)]
#[error_handlers(Relay)]
#[meta(Tag("lints"))]
impl LintsService {
    #[ulo_grpc::method(lints::lints::Unary)]
    async fn unary(&self, req: Request<Number>, cx: GrpcCx) -> Result<Response<Number>, Refusal> {
        let _ = cx.path();
        Ok(Response::new(*req.message()).metadata("x-lint", "unary"))
    }

    #[ulo_grpc::method(lints::lints::Server)]
    #[guards(with = || Allow)]
    fn server(&self, req: Message<Number>, metadata: GrpcMetadata) -> impl Stream<Item = Result<Number, Refusal>> {
        let _ = metadata.get("x-lint");
        stream::iter((1..=req.0.value).map(|value| Ok(Number { value })))
    }

    #[ulo_grpc::method(lints::lints::Client)]
    async fn client(&self, items: Streaming<Number>) -> Result<Number, Status> {
        let mut items = items;
        let mut value = 0;
        while let Some(item) = items.next().await {
            value += item.map_err(|_| Status::invalid_argument("bad item"))?.value;
        }
        Ok(Number { value })
    }

    #[ulo_grpc::method(lints::lints::Bidi)]
    async fn bidi(&self, items: Streaming<Number>) -> Result<impl Stream<Item = Result<Number, Refusal>>, Refusal> {
        Ok(items.map(|item| item.map(|n| Number { value: n.value * 2 }).map_err(|_| Refusal)))
    }
}
