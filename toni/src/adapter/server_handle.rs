use std::{future::Future, net::SocketAddr, pin::Pin};

pub struct ServerHandle {
    pub local_addr: SocketAddr,
    pub serve: Pin<Box<dyn Future<Output = ()> + Send + 'static>>,
}
