//! Route Metadata Example
//!
//! Demonstrates how to use #[set_metadata(...)] to pass handler-level configuration
//! to guards, interceptors, and other enhancers.
//!
//! This is Toni's equivalent to NestJS's @SetMetadata() + Reflector pattern,
//! but type-safe and without runtime reflection.
//!
//! ## How It Works
//!
//! 1. Define metadata types (any Clone + Send + Sync + 'static type)
//! 2. Attach metadata with `#[set_metadata(YourType { ... })]`, on the impl block for
//!    everything below it or on one handler for that handler alone
//! 3. Guards/interceptors read via `context.metadata()` + `.get::<YourType>()`
//!
//! ## Two levels
//!
//! A handler that declares the same type as its impl block replaces that entry and keeps the
//! others. `/moderate` below overrides the block's `Roles` while inheriting its `RateLimit`.
//!
//! ## One guard, any transport
//!
//! `metadata()` and `extensions()` are both on `HandlerContext`, so a guard written over it
//! registers unchanged on an HTTP controller, a WebSocket gateway or an RPC controller.
//! `RolesGuard` below is written that way: it reads the requirement from metadata and the caller
//! from the extension bag, leaving how the caller got there to whatever is transport-specific.

use toni::{
    async_trait,
    context::{HandlerContext, HttpContext},
    controller, get,
    http_helpers::Body as ToniBody,
    module, routes, set_metadata,
    traits_helpers::Guard,
    use_guards,
};

// ============================================================================
// Metadata Types
// ============================================================================

/// Required roles to access a route
#[derive(Clone)]
pub struct Roles(pub &'static [&'static str]);

/// Rate limiting configuration
#[derive(Clone)]
pub struct RateLimit {
    pub max_requests: u32,
    pub window_secs: u32,
}

/// Marks a route as publicly accessible (bypasses auth)
#[derive(Clone)]
pub struct Public;

/// Who is calling, put in the extension bag by whatever knows how to find that on this transport.
#[derive(Clone)]
pub struct Caller(pub String);

// ============================================================================
// Guards That Read Metadata
// ============================================================================

/// Reads the caller off the HTTP request and leaves it where any transport's guards can find it.
/// A gateway would do this from the handshake, an RPC controller from a header — the policy below
/// never learns which.
pub struct IdentifyCaller;

#[async_trait]
impl Guard<HttpContext> for IdentifyCaller {
    async fn can_activate(&self, context: &HttpContext) -> bool {
        let role = context
            .request()
            .headers
            .get("x-user-role")
            .and_then(|v| v.to_str().ok())
            .unwrap_or("guest");
        context.extensions().insert(Caller(role.to_string()));
        true
    }
}

/// The policy, over `HandlerContext` rather than one transport's context.
///
/// Registers unchanged on a `#[routes]`, `#[subscriptions]` or `#[patterns]` impl. Before
/// `#[set_metadata]` was collected on every transport this still compiled off HTTP — and read an
/// empty map, so it admitted everything it was written to refuse.
pub struct RolesGuard;

#[async_trait]
impl<C: HandlerContext> Guard<C> for RolesGuard {
    async fn can_activate(&self, context: &C) -> bool {
        let Some(metadata) = context.metadata() else {
            return true;
        };

        if metadata.get::<Public>().is_some() {
            return true;
        }

        let Some(Roles(required)) = metadata.get::<Roles>() else {
            return true;
        };

        let caller = context
            .extensions()
            .get::<Caller>()
            .map(|c| c.0)
            .unwrap_or_else(|| "guest".to_string());

        required.iter().any(|&r| r == caller)
    }
}

pub struct RateLimitGuard;

#[async_trait]
impl<C: HandlerContext> Guard<C> for RateLimitGuard {
    async fn can_activate(&self, context: &C) -> bool {
        let Some(metadata) = context.metadata() else {
            return true;
        };

        let Some(RateLimit {
            max_requests,
            window_secs,
        }) = metadata.get::<RateLimit>()
        else {
            return true;
        };

        // In production: check rate limit against Redis/in-memory store
        println!(
            "Rate limit check: {} requests per {} seconds",
            max_requests, window_secs
        );

        true
    }
}

// ============================================================================
// Controller With Metadata
// ============================================================================

#[controller("/api")]
pub struct ApiController {}

/// The two entries here apply to every handler below. A handler declaring the same type replaces
/// that one and keeps the rest.
#[routes]
#[use_guards(IdentifyCaller{}, RolesGuard{}, RateLimitGuard{})]
#[set_metadata(Roles(&["user", "admin"]))]
#[set_metadata(RateLimit { max_requests: 100, window_secs: 60 })]
impl ApiController {
    /// Public health check - no auth required.
    /// `Public` is declared here and nowhere above, so it applies to this handler alone.
    #[set_metadata(Public)]
    #[get("/health")]
    fn health(&self) -> ToniBody {
        ToniBody::json(serde_json::json!({ "status": "ok" }))
    }

    /// Inherits both of the block's entries and declares nothing itself.
    #[get("/profile")]
    fn profile(&self) -> ToniBody {
        ToniBody::json(serde_json::json!({ "user": "current_user" }))
    }

    /// Overrides `Roles` and keeps the block's `RateLimit`.
    #[set_metadata(Roles(&["admin"]))]
    #[get("/admin/stats")]
    fn admin_stats(&self) -> ToniBody {
        ToniBody::json(serde_json::json!({ "total_users": 1000 }))
    }

    /// Overrides `Roles` with a wider set; the rate limit is still the block's.
    #[set_metadata(Roles(&["admin", "moderator"]))]
    #[get("/moderate")]
    fn moderate(&self) -> ToniBody {
        ToniBody::json(serde_json::json!({ "queue": [] }))
    }
}

#[module(
    controllers: [ApiController],
    providers: [],
)]
pub struct AppModule;

fn main() {
    println!("Route Metadata Example");
    println!("======================");
    println!();
    println!("The impl block declares Roles(user, admin) and a RateLimit for every handler.");
    println!();
    println!("Available endpoints:");
    println!("  GET /api/health      - Public, declared on the handler alone");
    println!("  GET /api/profile     - inherits both of the block's entries");
    println!("  GET /api/admin/stats - overrides Roles to admin, keeps the block's RateLimit");
    println!("  GET /api/moderate    - overrides Roles to admin or moderator");
    println!();
    println!("Test with:");
    println!("  curl http://localhost:3000/api/health");
    println!("  curl -H 'x-user-role: admin' http://localhost:3000/api/admin/stats");
    println!("  curl -H 'x-user-role: user' http://localhost:3000/api/profile");
    println!();

    use toni::ToniFactory;
    use toni_axum::AxumAdapter;

    let rt = tokio::runtime::Runtime::new().unwrap();
    rt.block_on(async {
        let mut app = ToniFactory::create(AppModule).await.unwrap();
        app.use_http_adapter(AxumAdapter::new(), ("127.0.0.1", 3000))
            .unwrap();
        app.start().await.expect("failed to start");
    });
}
