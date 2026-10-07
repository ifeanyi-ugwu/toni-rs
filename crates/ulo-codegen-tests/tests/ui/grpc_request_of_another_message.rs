use ulo::{injectable, routes};
use ulo_codegen_tests::pb::{self, Number, Sum};
use ulo_grpc::Message;

#[injectable]
struct Counter;

#[routes]
impl Counter {
    // `Add` is unary, as this handler is, and takes an `AddRequest`.
    #[ulo_grpc::method(pb::counter::Add)]
    async fn add(&self, req: Message<Number>) -> Sum {
        Sum { sum: req.0.value }
    }
}

fn main() {}
