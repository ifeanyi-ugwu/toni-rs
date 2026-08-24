use toni::toni_factory::ToniFactory;
use toni::ModuleMetadata;
use toni_axum::AxumAdapter;

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
    pub async fn start(module: impl ModuleMetadata + 'static) -> Self {
        Self::start_with(ToniFactory::new(), module).await
    }

    /// Boot with a pre-configured factory (global middleware, enhancers, …).
    pub async fn start_with(factory: ToniFactory, module: impl ModuleMetadata + 'static) -> Self {
        Self::start_adapter(factory, module, AxumAdapter::new()).await
    }

    /// Boot on a specific HTTP adapter — the parameterization point for
    /// suites that must run against every adapter (global-chain conformance).
    pub async fn start_adapter(
        factory: ToniFactory,
        module: impl ModuleMetadata + 'static,
        adapter: impl toni::HttpAdapter + 'static,
    ) -> Self {
        Self::start_target(factory, module, adapter, ("127.0.0.1", 0)).await
    }

    /// Boot on an explicit [`BindTarget`] — the parameterization point for
    /// suites that hand the application a socket they bound themselves.
    pub async fn start_target(
        factory: ToniFactory,
        module: impl ModuleMetadata + 'static,
        adapter: impl toni::HttpAdapter + 'static,
        target: impl Into<toni::BindTarget>,
    ) -> Self {
        init_tracing();

        let target = target.into();
        let (addr_tx, addr_rx) = tokio::sync::oneshot::channel::<std::net::SocketAddr>();

        let local = tokio::task::LocalSet::new();
        local.spawn_local(async move {
            let mut app = factory.create_with(module).await.unwrap();
            app.use_http_adapter(adapter, target).unwrap();
            let bound = app.bind().await.unwrap();
            let addr = bound.http.expect("HTTP adapter not bound");
            let _ = addr_tx.send(addr);
            app.run().await;
        });

        tokio::task::spawn_local(async move {
            local.await;
        });

        let addr = addr_rx.await.unwrap();

        Self {
            port: addr.port(),
            base_url: format!("http://{}", addr),
            client: reqwest::Client::new(),
        }
    }

    pub fn client(&self) -> &reqwest::Client {
        &self.client
    }

    pub fn url(&self, path: &str) -> String {
        format!("{}{}", self.base_url, path)
    }
}
