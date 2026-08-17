//! A guard attaches a value to the message's extension bag and a later enhancer
//! reads it — the enhancer-to-enhancer half of the bus.
//!
//! `extension_bus.rs` covers the other half, where the reader is the handler.

use toni::async_trait;
use toni::context::{HandlerContext, HttpContext};
use toni::traits_helpers::Guard;

#[derive(Clone, Debug, PartialEq)]
struct Principal {
    user: String,
    roles: Vec<String>,
}

/// Authenticates the request and ATTACHES the principal to the context.
struct AuthGuard;

#[async_trait]
impl Guard<HttpContext> for AuthGuard {
    async fn can_activate(&self, ctx: &HttpContext) -> bool {
        // The whole point: a guard mutating the context. A compile error
        // under the old `&HttpContext` signature.
        ctx.extensions().insert(Principal {
            user: "alice".into(),
            roles: vec!["admin".into()],
        });
        true
    }
}

/// Reads what an upstream guard attached and authorizes on it.
struct RequireAdminGuard;

#[async_trait]
impl Guard<HttpContext> for RequireAdminGuard {
    async fn can_activate(&self, ctx: &HttpContext) -> bool {
        match ctx.extensions().get::<Principal>() {
            Some(principal) => principal.roles.iter().any(|r| r == "admin"),
            None => false,
        }
    }
}

fn context() -> HttpContext {
    let (parts, ()) = http::Request::builder()
        .method("GET")
        .uri("/admin")
        .body(())
        .unwrap()
        .into_parts();
    HttpContext::from_parts(parts)
}

#[tokio::test]
async fn guard_attaches_principal_that_a_later_guard_reads() {
    let mut ctx = context();

    // Nothing is attached before AuthGuard runs.
    assert!(ctx.extensions().get::<Principal>().is_none());

    // Guard A writes the principal.
    assert!(AuthGuard.can_activate(&ctx).await);

    // It is now on the context...
    assert_eq!(
        ctx.extensions().get::<Principal>().map(|p| p.user),
        Some("alice".to_string()),
    );

    // ...and Guard B (a downstream enhancer) reads it to authorize.
    assert!(RequireAdminGuard.can_activate(&ctx).await);
}

#[tokio::test]
async fn require_admin_denies_when_no_principal_was_attached() {
    let mut ctx = context();
    // AuthGuard never ran, so the downstream guard sees nothing and denies.
    assert!(!RequireAdminGuard.can_activate(&ctx).await);
}

/// A guard that runs on EVERY transport via one blanket impl. It can only use
/// the universal `HandlerContext` surface (route metadata, extensions,
/// cancellation) — no `ctx.request()` (HTTP) or `ctx.client()` (WS), because
/// those live on the concrete context types, not the shared trait. This is
/// exactly the `impl<C: HandlerContext> Guard<C>` form the guard docs
/// describe, and it compiles.
struct UniversalGuard;

#[async_trait]
impl<C: HandlerContext + ?Sized> Guard<C> for UniversalGuard {
    async fn can_activate(&self, ctx: &C) -> bool {
        ctx.extensions().get::<Denied>().is_none()
    }
}

#[derive(Clone)]
struct Denied;

#[tokio::test]
async fn blanket_guard_runs_against_a_concrete_context() {
    let mut ctx = context();
    assert!(UniversalGuard.can_activate(&ctx).await);
}
