use app::app_module::AppModule;
use toni::ToniFactory;
use toni_axum::AxumAdapter;

mod app;

#[tokio::main]
async fn main() {
    let mut app = ToniFactory::new()
        .create_with(AppModule)
        .await;

    app.use_http_adapter(AxumAdapter::new(), ("127.0.0.1", 3000))
        .unwrap();

    app.start().await.unwrap();
}
