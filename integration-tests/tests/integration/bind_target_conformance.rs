//! Conformance suite for [`BindTarget::Listener`] — serving on a socket the
//! caller bound.
//!
//! The proof that the adapter adopted the socket rather than binding its own
//! is address identity: the test records the listener's address before handing
//! it over, then requires the application to report and serve on that exact
//! address. A fresh bind on port 0 would land somewhere else.
//!
//! Rocket is the one adapter that cannot adopt a listener (bind is fused into
//! `launch()` behind figment config); it must refuse at `bind()` rather than
//! silently binding elsewhere.

use std::net::TcpListener;

use toni::{controller, get, module, routes, Body as ToniBody, ToniFactory};

use crate::common::TestServer;

#[controller("/inherited")]
pub struct InheritedController {}

#[routes]
impl InheritedController {
    #[get("/ping")]
    fn ping(&self) -> ToniBody {
        ToniBody::text("pong")
    }
}

#[module(controllers: [InheritedController])]
impl BindTargetModule {}

async fn case_serves_on_caller_socket(adapter: impl toni::HttpAdapter + 'static) {
    let listener = TcpListener::bind("127.0.0.1:0").unwrap();
    let expected = listener.local_addr().unwrap();

    let server =
        TestServer::start_target(ToniFactory::new(), BindTargetModule, adapter, listener).await;

    assert_eq!(
        server.base_url,
        format!("http://{expected}"),
        "adapter reported a different address than the listener it was given"
    );

    let resp = server
        .client()
        .get(server.url("/inherited/ping"))
        .send()
        .await
        .unwrap();
    assert_eq!(resp.status(), 200);
    assert_eq!(resp.text().await.unwrap(), "pong");
}

macro_rules! bind_target_suite {
    ($adapter_mod:ident, $adapter:expr) => {
        mod $adapter_mod {
            #[tokio_localset_test::localset_test]
            async fn serves_on_caller_supplied_listener() {
                super::case_serves_on_caller_socket($adapter).await;
            }
        }
    };
}

bind_target_suite!(axum, toni_axum::AxumAdapter::new());
bind_target_suite!(poem, toni_poem::PoemAdapter::new());
bind_target_suite!(salvo, toni_salvo::SalvoAdapter::new());
bind_target_suite!(actix, toni_actix::ActixAdapter::new());

#[tokio_localset_test::localset_test]
async fn rocket_refuses_a_pre_bound_listener() {
    let listener = TcpListener::bind("127.0.0.1:0").unwrap();

    let mut app = ToniFactory::new().create_with(BindTargetModule).await;
    app.use_http_adapter(toni_rocket::RocketAdapter::new(), listener)
        .unwrap();

    let err = app
        .bind()
        .await
        .expect_err("rocket cannot serve on an existing listener");
    let msg = err.to_string();
    assert!(
        msg.contains("cannot adopt"),
        "expected a capability error naming the limitation, got: {msg}"
    );
}
