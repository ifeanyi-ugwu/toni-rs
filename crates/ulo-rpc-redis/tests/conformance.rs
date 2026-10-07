//! The RPC conformance suite over the Redis link, each scenario against a Redis server in a
//! container of its own, since Pub/Sub channels span every database of a server. The client
//! reaches it through a relay that `disrupt` cuts.

#![cfg(feature = "integration")]

use std::net::{Ipv4Addr, SocketAddr};
use std::time::Duration;

use testcontainers::core::{IntoContainerPort, WaitFor};
use testcontainers::runners::AsyncRunner;
use testcontainers::{ContainerAsync, GenericImage};
use ulo::BoundAddr;
use ulo_rpc_conformance::{Broker, report, startup_failed};
use ulo_rpc_conformance::relay::{Relay, reachable, unshadowed};
use ulo_rpc_redis::Redis;

const PORT: u16 = 6379;

struct RedisServer {
    server: SocketAddr,
    relay: Relay,
    _container: ContainerAsync<GenericImage>,
}

impl Broker for RedisServer {
    type Link = Redis;

    async fn start() -> Self {
        let (container, addrs) = unshadowed("the Redis container", || async {
            let container = GenericImage::new("redis", "7-alpine")
                .with_exposed_port(PORT.tcp())
                .with_wait_for(WaitFor::message_on_stdout("Ready to accept connections"))
                .start()
                .await
                .unwrap_or_else(|error| startup_failed!("the Redis container did not start: {}", report(&error)));
            let port = container.get_host_port_ipv4(PORT).await
                .unwrap_or_else(|error| startup_failed!("the Redis port is not mapped: {}", report(&error)));
            (container, vec![SocketAddr::from((Ipv4Addr::LOCALHOST, port))])
        })
        .await;
        let server = addrs[0];
        reachable(server, Duration::from_secs(10)).await;
        RedisServer { server, relay: Relay::start(server).await, _container: container }
    }

    fn link(&self) -> Redis {
        Redis::url(format!("redis://{}", self.server))
    }

    fn client_link(&self, _server: &[BoundAddr]) -> Redis {
        Redis::url(format!("redis://{}", self.relay.addr()))
    }

    async fn disrupt(&self) {
        self.relay.cut().await;
    }

    async fn client_connections(&self) -> Option<usize> {
        Some(self.relay.open().await)
    }
}

ulo_rpc_conformance::conformance_suite!(RedisServer);
