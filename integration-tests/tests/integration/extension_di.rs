//! `Extension<T>` carries a guard's write down the injection tree.
//!
//! The reader here is not the handler and not an enhancer — it is a service two
//! constructions below the controller, which has neither a context nor a handler
//! parameter. Declaring the dependency is the only way it sees the value.
//!
//! `extension_bus.rs` covers the enhancer and handler halves.

use std::sync::atomic::{AtomicUsize, Ordering};

use toni::async_trait;
use toni::context::{Extensions, HttpContext};
use toni::traits_helpers::Guard;
use toni::{controller, get, injectable, module, routes, Body as ToniBody, Extension};

use crate::common::TestServer;

#[derive(Clone, Debug, PartialEq)]
pub struct CurrentUser(String);

/// Writes through the DI view, the way application code would.
#[injectable(scope = "request")]
pub struct AuthGuard {
    #[inject]
    user: Extension<CurrentUser>,
}

#[async_trait]
impl Guard<HttpContext> for AuthGuard {
    async fn can_activate(&self, _ctx: &HttpContext) -> bool {
        self.user.set(CurrentUser("alice".into()));
        true
    }
}

/// Two levels below the controller, with no route and no context of its own.
#[injectable(scope = "request")]
pub struct AuditLog {
    #[inject]
    user: Extension<CurrentUser>,
}

impl AuditLog {
    fn who(&self) -> String {
        self.user
            .get()
            .map(|u| u.0)
            .unwrap_or_else(|| "ABSENT".into())
    }
}

#[injectable(scope = "request")]
pub struct OrderService {
    #[inject]
    audit: AuditLog,
}

impl OrderService {
    fn place(&self) -> String {
        format!("order-by-{}", self.audit.who())
    }
}

#[controller("/orders")]
pub struct OrderController {
    #[inject]
    orders: OrderService,
    #[inject]
    bag: Extensions,
}

#[routes]
#[use_guards(AuthGuard)]
impl OrderController {
    #[get("/place")]
    fn place(&self) -> ToniBody {
        ToniBody::text(self.orders.place())
    }

    /// The bag injects directly too, without declaring a view per type.
    #[get("/raw")]
    fn raw(&self) -> ToniBody {
        let who = self
            .bag
            .get::<CurrentUser>()
            .map(|u| u.0)
            .unwrap_or_else(|| "ABSENT".into());
        ToniBody::text(who)
    }
}

#[module(
    controllers: [OrderController],
    providers: [Extension::<CurrentUser>, AuthGuard, AuditLog, OrderService]
)]
impl OrderModule {}

async fn get(server: &TestServer, path: &str) -> String {
    server
        .client()
        .get(server.url(path))
        .send()
        .await
        .unwrap()
        .text()
        .await
        .unwrap()
}

#[tokio_localset_test::localset_test]
async fn a_nested_service_reads_what_the_guard_attached() {
    let server = TestServer::start(OrderModule).await;

    assert_eq!(get(&server, "/orders/place").await, "order-by-alice");
}

#[tokio_localset_test::localset_test]
async fn the_bag_injects_without_a_per_type_view() {
    let server = TestServer::start(OrderModule).await;

    assert_eq!(get(&server, "/orders/raw").await, "alice");
}

/// Writes on its first run only, so a value seen on the second request could
/// only have survived from the first.
#[injectable(scope = "request")]
pub struct OnceGuard {
    #[inject]
    user: Extension<CurrentUser>,
}

static GUARD_RUNS: AtomicUsize = AtomicUsize::new(0);

#[async_trait]
impl Guard<HttpContext> for OnceGuard {
    async fn can_activate(&self, _ctx: &HttpContext) -> bool {
        if GUARD_RUNS.fetch_add(1, Ordering::SeqCst) == 0 {
            self.user.set(CurrentUser("alice".into()));
        }
        true
    }
}

#[controller("/once")]
pub struct OnceController {
    #[inject]
    user: Extension<CurrentUser>,
}

#[routes]
#[use_guards(OnceGuard)]
impl OnceController {
    #[get("/who")]
    fn who(&self) -> ToniBody {
        ToniBody::text(
            self.user
                .get()
                .map(|u| u.0)
                .unwrap_or_else(|| "ABSENT".into()),
        )
    }
}

#[module(controllers: [OnceController], providers: [Extension::<CurrentUser>, OnceGuard])]
impl OnceModule {}

#[tokio_localset_test::localset_test]
async fn values_do_not_survive_into_the_next_request() {
    let server = TestServer::start(OnceModule).await;

    assert_eq!(get(&server, "/once/who").await, "alice");
    assert_eq!(get(&server, "/once/who").await, "ABSENT");
}
