//! Passing request-scoped data from a guard to the code that needs it
//!
//! A guard authenticates the caller and attaches the result. Everything else in
//! the request reads it by declaring `Extension<CurrentUser>` — the controller,
//! and a service two constructions below it that has no route and no request of
//! its own. Nothing threads the user through call signatures.
//!
//! Run with:  cargo run --example request_scoped_context
//! Test:      curl -H 'authorization: Bearer alice-token' http://127.0.0.1:3000/orders
//!            curl http://127.0.0.1:3000/orders

use toni::async_trait;
use toni::context::HttpContext;
use toni::traits_helpers::Guard;
use toni::*;
use toni_axum::AxumAdapter;

#[derive(Clone, Debug)]
pub struct CurrentUser {
    id: String,
    admin: bool,
}

/// Reads the credential off the request and attaches the caller. Rejecting here
/// means the handler never runs, so everything downstream can assume a user.
#[injectable(scope = "request")]
pub struct AuthGuard {
    #[inject]
    user: Extension<CurrentUser>,
    #[inject]
    request: Request,
}

#[async_trait]
impl Guard<HttpContext> for AuthGuard {
    async fn can_activate(&self, _ctx: &mut HttpContext) -> bool {
        let Some(token) = self
            .request
            .header("authorization")
            .and_then(|h| h.strip_prefix("Bearer "))
        else {
            return false;
        };

        let Some(id) = token.strip_suffix("-token") else {
            return false;
        };

        self.user.set(CurrentUser {
            id: id.to_string(),
            admin: id == "root",
        });
        true
    }
}

/// Deep in the call tree: no route, no context, no request parameter. It
/// declares what it needs and the container supplies this request's value.
#[injectable(scope = "request")]
pub struct AuditLog {
    #[inject]
    user: Extension<CurrentUser>,
}

impl AuditLog {
    fn record(&self, action: &str) -> String {
        match self.user.get() {
            Some(user) => format!("[audit] {} performed {}", user.id, action),
            None => format!("[audit] anonymous performed {}", action),
        }
    }
}

#[injectable(scope = "request")]
pub struct OrderService {
    #[inject]
    audit: AuditLog,
}

impl OrderService {
    fn list(&self) -> String {
        self.audit.record("orders.list")
    }
}

#[controller("/orders")]
pub struct OrderController {
    #[inject]
    orders: OrderService,
    #[inject]
    user: Extension<CurrentUser>,
}

#[routes]
#[use_guards(AuthGuard)]
impl OrderController {
    #[get("/")]
    fn list(&self) -> Body {
        // The same value the guard attached and the audit log will read.
        let user = self
            .user
            .get()
            .expect("AuthGuard rejects anonymous callers");

        Body::json(serde_json::json!({
            "user": user.id,
            "admin": user.admin,
            "audit": self.orders.list(),
        }))
    }
}

// `Extension::<CurrentUser>` registers the payload type; every injection site
// above resolves to the same per-request value.
#[module(
    controllers: [OrderController],
    providers: [Extension::<CurrentUser>, AuthGuard, AuditLog, OrderService]
)]
impl AppModule {}

#[tokio::main]
async fn main() -> anyhow::Result<()> {
    println!("🔐 request-scoped context\n");
    println!("  curl -H 'authorization: Bearer alice-token' http://127.0.0.1:3000/orders");
    println!("  curl http://127.0.0.1:3000/orders    # 403, the guard rejects");
    println!();

    let mut app = ToniFactory::new().create_with(AppModule).await;

    app.use_http_adapter(AxumAdapter::new(), ("127.0.0.1", 3000))?;
    app.start().await?;

    Ok(())
}
