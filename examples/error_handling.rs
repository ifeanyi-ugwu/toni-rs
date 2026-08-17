//! Error handling.
//!
//! Demonstrates:
//! 1. Domain error types with `#[derive(toni::Error)]` — the 95% path
//! 2. Returning `Result<T, MyError>` from handlers — auto-converted into
//!    `HttpError` at the dispatcher boundary and rendered to a canonical
//!    JSON envelope
//! 3. Custom envelope rendering via a `#[catch(MyError)]` chain handler —
//!    the override path for adding headers or restructuring the body
//! 4. `HttpError` as a user-convenience type (for trivial cases that don't
//!    warrant a dedicated error)
//! 5. `#[catch(GuardRejection)]` as the escape hatch for re-shaping
//!    framework-generated events — chain dispatch fires for both
//!    framework events *and* user errors uniformly
//!
//! Run with:
//! ```bash
//! cargo run --example error_handling
//! ```

use serde::Serialize;
use serde_json::json;
use toni::{
    async_trait, catch, context::HttpContext, controller, errors::HttpError, get,
    http_helpers::Body, injectable, module, post, routes, toni_factory::ToniFactory,
    traits_helpers::Guard, Error, HttpRequest, HttpResponse,
};
use toni_axum::AxumAdapter;
use toni_macros::use_guards;

// ---- Domain error: derived toni::Error, default canonical envelope -------------

#[derive(Debug, toni::Error)]
enum UserError {
    #[error_kind(NotFound)]
    NotFound(String),

    #[error_kind(BadRequest)]
    InvalidId(String),

    #[error_kind(Conflict)]
    EmailTaken(String),

    #[error_kind(UnprocessableEntity)]
    InvalidEmail(String),
}

impl std::fmt::Display for UserError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::NotFound(id) => write!(f, "user {id} not found"),
            Self::InvalidId(id) => write!(f, "invalid user id: {id}"),
            Self::EmailTaken(email) => write!(f, "email already in use: {email}"),
            Self::InvalidEmail(email) => write!(f, "malformed email: {email}"),
        }
    }
}

impl std::error::Error for UserError {}

// ---- Domain error with a custom HTTP envelope override ----------------------

#[derive(Debug)]
struct PaymentDeclined {
    reason_code: &'static str,
    retry_after_secs: u32,
}

impl std::fmt::Display for PaymentDeclined {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "payment declined ({})", self.reason_code)
    }
}

impl std::error::Error for PaymentDeclined {}

impl Error for PaymentDeclined {
    fn kind(&self) -> toni::ErrorKind {
        toni::ErrorKind::UnprocessableEntity
    }
}

// Override path: a `#[catch]` chain handler. Renders `PaymentDeclined`
// with a Retry-After header and a domain-specific JSON shape. Without
// this handler, the canonical envelope (status 422 + `{"statusCode":...}`)
// would render via the `From<PaymentDeclined> for HttpError` blanket.
#[catch(PaymentDeclined)]
async fn render_payment_declined(err: &PaymentDeclined, _ctx: &HttpContext) -> HttpResponse {
    HttpResponse::builder()
        .status(toni::errors::http_status(err.kind()))
        .header("Retry-After", err.retry_after_secs.to_string())
        .json(json!({
            "type": "payment_declined",
            "reason_code": err.reason_code,
            "retry_after_secs": err.retry_after_secs,
        }))
        .build()
}

// ---- Service ----------------------------------------------------------------

#[derive(Serialize)]
struct User {
    id: String,
    name: String,
    email: String,
}

#[injectable]
pub struct UserService {}
impl UserService {
    fn find_user(&self, id: &str) -> Result<User, UserError> {
        if id == "1" {
            Ok(User {
                id: "1".into(),
                name: "John Doe".into(),
                email: "john@example.com".into(),
            })
        } else if id == "invalid" {
            Err(UserError::InvalidId(id.into()))
        } else {
            Err(UserError::NotFound(id.into()))
        }
    }

    fn create_user(&self, email: &str) -> Result<User, UserError> {
        if email == "existing@example.com" {
            Err(UserError::EmailTaken(email.into()))
        } else if !email.contains('@') {
            Err(UserError::InvalidEmail(email.into()))
        } else {
            Ok(User {
                id: "new-123".into(),
                name: "New User".into(),
                email: email.into(),
            })
        }
    }
}

// ---- Guard for chain demonstration -----------------------------------------

pub struct AuthGuard;

#[async_trait]
impl Guard<HttpContext> for AuthGuard {
    async fn can_activate(&self, ctx: &HttpContext) -> bool {
        ctx.request().headers.contains_key("x-auth-token")
    }
}

// ---- #[catch] escape hatch: reshape guard-rejection 4xx --------------------
//
// User errors render directly through the active transport. The chain only fires for framework-
// generated events — `GuardRejection`, `MiddlewareFailure`, etc. A `#[catch]`
// registered on a controller dispatches on the typed event and reshapes the
// response.

#[catch(toni::errors::GuardRejection)]
async fn auth_failure(err: &toni::errors::GuardRejection, _ctx: &HttpContext) -> HttpResponse {
    HttpResponse::builder()
        .status(toni::errors::http_status(err.kind()))
        .json(json!({
            "error": "auth_required",
            "hint": "Send `x-auth-token: <token>`",
            "detail": err.message(),
        }))
        .build()
}

// ---- Controllers ------------------------------------------------------------

#[controller("/users")]
pub struct UserController {
    #[inject]
    service: UserService,
}

#[routes]
impl UserController {
    /// Returning `Result<T, UserError>` — the framework lifts UserError
    /// into HttpError via the `From<E: Error>` blanket and renders the
    /// canonical envelope. No chain handler registered for UserError, so
    /// the dispatcher's fallback rendering applies.
    #[get("/{id}")]
    fn get_user(&self, req: HttpRequest) -> Result<Body, UserError> {
        let id = req
            .extensions()
            .get::<toni::http_helpers::PathParams>()
            .and_then(|p| p.0.get("id").map(|s| s.as_str()))
            .ok_or_else(|| UserError::InvalidId("(missing)".into()))?;

        let user = self.service.find_user(id)?;
        Ok(Body::json(serde_json::to_value(user).unwrap()))
    }

    #[post("/")]
    async fn create_user(
        &self,
        toni::extractors::Json(body): toni::extractors::Json<serde_json::Value>,
    ) -> Result<HttpResponse, UserError> {
        let email = body
            .get("email")
            .and_then(|v| v.as_str())
            .ok_or_else(|| UserError::InvalidEmail("(missing)".into()))?;

        let user = self.service.create_user(email)?;

        Ok(HttpResponse::created()
            .header("Location", format!("/users/{}", user.id))
            .json(serde_json::to_value(user).unwrap())
            .build())
    }

    /// `HttpError` is a convenience type for trivial cases that don't
    /// warrant a dedicated error type — return it directly and the
    /// dispatcher renders it via `HttpError::to_response`.
    #[get("/-/teapot")]
    fn teapot(&self) -> Result<Body, HttpError> {
        Err(HttpError::custom(418, "I'm a teapot"))
    }
}

#[controller("/billing")]
pub struct BillingController {}

#[routes]
#[toni_macros::use_error_handlers(render_payment_declined)]
impl BillingController {
    /// `PaymentDeclined` is a plain `toni::Error`; the registered
    /// `#[catch(PaymentDeclined)]` handler reshapes it with a Retry-After
    /// header and a domain-specific JSON body.
    #[post("/charge")]
    fn charge(&self) -> Result<Body, PaymentDeclined> {
        Err(PaymentDeclined {
            reason_code: "insufficient_funds",
            retry_after_secs: 60,
        })
    }
}

#[controller("/admin")]
pub struct AdminController {}

#[routes]
#[use_guards(AuthGuard {})]
#[toni_macros::use_error_handlers(auth_failure)]
impl AdminController {
    /// Guard rejection is a framework-generated error. The `#[catch]` handler
    /// registered above reshapes the 403 envelope.
    #[get("/dashboard")]
    fn dashboard(&self) -> Body {
        Body::json(json!({"status": "ok"}))
    }
}

// ---- Wiring -----------------------------------------------------------------

#[module(
    controllers: [UserController, BillingController, AdminController],
    providers: [UserService],
)]
pub struct AppModule;

#[tokio::main]
async fn main() -> anyhow::Result<()> {
    println!("Server running on http://localhost:3000\n");
    println!("Canonical-envelope responses (auto, no chain):");
    println!("  GET  /users/1            -> 200 OK");
    println!("  GET  /users/missing      -> 404 (UserError::NotFound)");
    println!("  GET  /users/invalid      -> 400 (UserError::InvalidId)");
    println!("  POST /users (existing)   -> 409 (UserError::EmailTaken)");
    println!("  GET  /users/-/teapot     -> 418 (HttpError::Custom)\n");
    println!("Custom envelope override (via #[catch]):");
    println!("  POST /billing/charge     -> 422 + Retry-After + custom JSON\n");
    println!("Chain on framework-generated error (guard rejection):");
    println!("  GET  /admin/dashboard         -> 403 reshaped by #[catch(GuardRejection)]");
    println!("  GET  /admin/dashboard with x-auth-token: any -> 200 OK\n");

    let factory = ToniFactory::new();
    let mut app = factory.create_with(AppModule).await;
    app.use_http_adapter(AxumAdapter::new(), ("127.0.0.1", 3000))?;
    app.run().await;
    Ok(())
}
