use std::sync::atomic::{AtomicU16, Ordering};
use std::time::Duration;
use toni::module_helpers::module_enum::ModuleDefinition;
use toni::toni_factory::ToniFactory;
use toni_axum::AxumAdapter;

static PORT_COUNTER: AtomicU16 = AtomicU16::new(30000);

/// Install a tracing subscriber that reads `RUST_LOG` (e.g. `RUST_LOG=toni=debug`).
/// Safe to call multiple times — only the first call takes effect.
pub fn init_tracing() {
    let _ = tracing_subscriber::fmt()
        .with_env_filter(
            tracing_subscriber::EnvFilter::try_from_default_env()
                .unwrap_or_else(|_| tracing_subscriber::EnvFilter::new("toni=error")),
        )
        .with_test_writer()
        .try_init();
}

pub struct TestServer {
    pub port: u16,
    pub base_url: String,
    client: reqwest::Client,
}

impl TestServer {
    pub async fn start(module: ModuleDefinition) -> Self {
        init_tracing();

        let port = PORT_COUNTER.fetch_add(1, Ordering::SeqCst);
        let base_url = format!("http://127.0.0.1:{}", port);

        let local = tokio::task::LocalSet::new();

        local.spawn_local(async move {
            let mut app = ToniFactory::create(module).await;
            app.use_http_adapter(AxumAdapter::new(), port, "127.0.0.1")
                .unwrap();
            let _ = app.start().await;
        });

        tokio::task::spawn_local(async move {
            local.await;
        });

        let client = reqwest::Client::new();
        tokio::time::sleep(Duration::from_millis(500)).await;

        Self {
            port,
            base_url,
            client,
        }
    }

    pub fn client(&self) -> &reqwest::Client {
        &self.client
    }

    pub fn url(&self, path: &str) -> String {
        format!("{}{}", self.base_url, path)
    }
}
