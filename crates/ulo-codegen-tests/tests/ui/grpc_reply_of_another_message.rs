use ulo::{injectable, routes};
use ulo_codegen_tests::pb::{self, AddRequest, Number};
use ulo_grpc::Message;

#[injectable]
struct Counter;

#[routes]
impl Counter {
    // `Add` is unary, as this handler is, and replies a `Sum`.
    #[ulo_grpc::method(pb::counter::Add)]
    async fn add(&self, req: Message<AddRequest>) -> Number {
        Number { value: req.0.a + req.0.b }
    }
}

fn main() {}
