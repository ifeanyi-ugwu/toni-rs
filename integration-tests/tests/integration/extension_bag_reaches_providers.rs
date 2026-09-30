//! Data a middleware puts in the request extension bag reaches an execution-scoped
//! provider and the controller that injects it, without either extracting it by
//! hand.
//!
//! The bag is the seam between a middleware, which runs before DI has anything
//! request-shaped to resolve, and a provider built per request. What the
//! provider reads is what the middleware wrote on that same request — the
//! claim that still passes when scope is wrong, since a singleton returns
//! whichever request populated it first.

use std::sync::Arc;
use ulo::http::{Body, Request};
use ulo::{UloFactory, controller, get, injectable, module, new, routes};
// ===== 1. Define types to store in extensions =====

#[derive(Clone, Debug)]
pub struct UserId(String);

#[derive(Clone, Debug)]
pub struct RequestId(String);

// ===== 2. Execution-scoped provider built from the request =====

#[injectable(scope = "execution")]
pub struct RequestContext {
    user_id: String,
    request_id: String,
    is_authenticated: bool,
}

impl RequestContext {
    /// Built per request from the injected `Request`, an execution-scoped injectable.
    #[new]
    fn new(req: Request) -> Self {
        let user_id = req
            .extensions()
            .get::<UserId>()
            .map(|u| u.0.clone())
            .unwrap_or_else(|| "anonymous".to_string());

        let request_id = req
            .extensions()
            .get::<RequestId>()
            .map(|r| r.0.clone())
            .unwrap_or_else(|| "no-request-id".to_string());

        let is_authenticated = user_id != "anonymous";

        Self {
            user_id,
            request_id,
            is_authenticated,
        }
    }

    pub fn require_auth(&self) -> Result<&str, &'static str> {
        if self.is_authenticated {
            Ok(&self.user_id)
        } else {
            Err("Unauthenticated")
        }
    }

    pub fn get_user_id(&self) -> &str {
        &self.user_id
    }

    pub fn get_request_id(&self) -> &str {
        &self.request_id
    }
}

// ===== 3. Singleton service (business logic) =====

#[injectable]
pub struct UserService {}

impl UserService {
    pub fn get_user_data(&self, user_id: &str) -> String {
        // Pure business logic - no HTTP coupling!
        format!("Data for user: {}", user_id)
    }
}

// ===== 4. Controller using request context =====

#[controller("/users")]
pub struct UserController {
    #[inject]
    context: Arc<RequestContext>, // Execution-scoped context
    #[inject]
    user_service: Arc<UserService>, // Singleton service
}

#[routes]
impl UserController {
    #[get("/me")]
    fn get_current_user(&self) -> Body {
        // No manual extraction! Context is already populated
        let user_id = self.context.get_user_id();
        let request_id = self.context.get_request_id();

        let data = self.user_service.get_user_data(user_id);

        Body::text(format!(
            "Request ID: {}\nUser: {}\nData: {}",
            request_id, user_id, data
        ))
    }

    #[get("/protected")]
    fn protected_route(&self) -> Body {
        // Easy auth check
        match self.context.require_auth() {
            Ok(user_id) => Body::text(format!("Protected data for user: {}", user_id)),
            Err(msg) => Body::text(msg.to_string()),
        }
    }
}

// ===== 5. Module definition =====

#[module(
    providers: [RequestContext, UserService],
    controllers: [UserController],
)]
impl TestModule {}

// ===== 6. Tests =====

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_request_context_construction() {
        let (mut req, ()) = http::Request::builder()
            .method("GET")
            .uri("/test")
            .body(())
            .unwrap()
            .into_parts();

        req.extensions.insert(UserId("alice".to_string()));
        req.extensions.insert(RequestId("req-123".to_string()));

        // Build the injected Request from the parts, then run the constructor.
        let context = RequestContext::new(Request::from_parts(&req));

        assert_eq!(context.get_user_id(), "alice");
        assert_eq!(context.get_request_id(), "req-123");
        assert!(context.is_authenticated);
        assert!(context.require_auth().is_ok());
    }

    #[test]
    fn test_request_context_anonymous() {
        let (req, ()) = http::Request::builder()
            .method("GET")
            .uri("/test")
            .body(())
            .unwrap()
            .into_parts();

        let context = RequestContext::new(Request::from_parts(&req));

        assert_eq!(context.get_user_id(), "anonymous");
        assert!(!context.is_authenticated);
        assert!(context.require_auth().is_err());
    }

    #[test]
    fn test_user_service() {
        // Test singleton service can be tested independently
        let service = UserService {};
        let data = service.get_user_data("bob");
        assert_eq!(data, "Data for user: bob");
    }

    #[tokio::test]
    async fn test_di_resolves() {
        // Verify the module wires correctly: UserService (singleton) must resolve,
        // and its business logic must be callable without an HTTP server.
        let app = UloFactory::create(TestModule).await.unwrap();

        let service = app
            .get::<UserService>()
            .await
            .expect("UserService should resolve as singleton");
        assert_eq!(service.get_user_data("alice"), "Data for user: alice");
    }
}
