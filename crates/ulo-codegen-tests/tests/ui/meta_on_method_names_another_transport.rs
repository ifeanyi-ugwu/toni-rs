use std::time::Duration;

use ulo::{injectable, routes};
use ulo_http::Timeout;
use ulo_rpc::Payload;

#[injectable]
struct Orders;

#[routes]
impl Orders {
    #[ulo_rpc::message("orders.get")]
    #[meta(Timeout::after(Duration::from_secs(5)))]
    async fn get(&self, id: Payload<u64>) -> u64 {
        id.0
    }
}

fn main() {}
