use ulo::{injectable, routes};
use ulo_http::BodyLimit;
use ulo_rpc::Payload;

#[injectable]
struct Orders;

#[routes]
#[meta(BodyLimit(1024))]
impl Orders {
    #[ulo_rpc::message("orders.get")]
    async fn get(&self, id: Payload<u64>) -> u64 {
        id.0
    }
}

fn main() {}
