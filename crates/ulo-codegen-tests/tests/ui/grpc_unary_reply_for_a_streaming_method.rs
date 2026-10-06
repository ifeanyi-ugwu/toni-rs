use ulo::{injectable, routes};
use ulo_codegen_tests::pb::{self, CountRequest, Number};
use ulo_grpc::Message;

#[injectable]
struct Counter;

#[routes]
impl Counter {
    // `Count` streams its reply.
    #[ulo_grpc::method(pb::counter::Count)]
    async fn count(&self, req: Message<CountRequest>) -> Number {
        Number { value: i64::from(req.0.up_to) }
    }
}

fn main() {}
