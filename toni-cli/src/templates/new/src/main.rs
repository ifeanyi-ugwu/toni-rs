use app::app_module::AppModule;
use toni::ToniFactory;
use toni_axum::AxumAdapter;

mod app;

#[tokio::main]
async fn main() {
    let mut app = ToniFactory::new()
        .create_with(AppModule::module_definition())
        .await;

    app.use_http_adapter(AxumAdapter::new(), 3000, "127.0.0.1")
        .unwrap();

    app.start().await.unwrap();
}
