//! Adopting a socket bound by another process.
//!
//! The app takes its listening socket from whoever started it instead of
//! binding one itself. Because that socket stays open across restarts, requests
//! arriving while the app is down wait in the kernel accept queue rather than
//! being refused — the connection reset during a rebuild disappears.
//!
//! The same code covers two cases: `toni dev --listen 3000` in development, and
//! systemd socket activation in production. Both announce the socket through
//! `LISTEN_FDS`, which `listenfd` reads.
//!
//! Run under the dev server:
//!   toni dev --listen 3000
//!
//! Run standalone — no socket passed, so it binds 127.0.0.1:3000 itself:
//!   cargo run --example socket_activation

use listenfd::ListenFd;
use toni::*;
use toni_macros::{controller, get, module, routes};

#[controller("/")]
pub struct HelloController;

#[routes]
impl HelloController {
    #[get("/")]
    async fn hello(&self) -> Result<String, HttpError> {
        Ok(format!("served by pid {}", std::process::id()))
    }
}

#[module(controllers: [HelloController])]
struct AppModule;

/// Take the inherited socket if there is one, otherwise bind normally.
///
/// Keeping the fallback means the binary still runs on its own — under a
/// debugger, in tests, or from `cargo run` — with no launcher involved.
fn http_target() -> anyhow::Result<BindTarget> {
    let mut fds = ListenFd::from_env();
    match fds.take_tcp_listener(0)? {
        Some(listener) => {
            println!("adopted the inherited socket at {}", listener.local_addr()?);
            Ok(listener.into())
        }
        None => {
            println!("no socket passed in, binding 127.0.0.1:3000");
            Ok(("127.0.0.1", 3000).into())
        }
    }
}

#[tokio::main]
async fn main() -> anyhow::Result<()> {
    let mut app = ToniFactory::new().create_with(AppModule).await?;
    app.use_http_adapter(toni_axum::AxumAdapter::new(), http_target()?)?;

    let bound = app.bind().await?;
    if let Some(addr) = bound.http {
        println!("listening on http://{addr}");
    }

    app.run().await;
    Ok(())
}
