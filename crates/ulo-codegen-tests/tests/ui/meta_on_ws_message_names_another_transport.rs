use std::time::Duration;

use ulo::{injectable, routes};
use ulo_http::Timeout;

#[injectable]
struct Chat;

#[routes]
#[ulo_ws::gateway(path = "/chat")]
impl Chat {
    #[ulo_ws::message("chat.send")]
    #[meta(Timeout::after(Duration::from_secs(5)))]
    async fn send(&self) {}
}

fn main() {}
