use futures_util::StreamExt;
use ulo::{injectable, routes};
use ulo_codegen_tests::pb::{self, AddRequest, Sum};
use ulo_grpc::Streaming;

#[injectable]
struct Counter;

#[routes]
impl Counter {
    // `Add` takes one request message.
    #[ulo_grpc::method(pb::counter::Add)]
    async fn add(&self, mut items: Streaming<AddRequest>) -> Sum {
        let mut sum = 0;
        while let Some(Ok(item)) = items.next().await {
            sum += item.a + item.b;
        }
        Sum { sum }
    }
}

fn main() {}
