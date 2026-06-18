//! POC for the phase-2 clean architecture: auto-populate the typed role registry by detecting
//! enhancer trait impls at the provider factory, with no marker.
//!
//! The mechanism is inherent-method-beats-trait-method specialization (stable): a probe type has
//! an inherent method that exists only when `T: Guard<HttpContext>`, shadowing a blanket trait
//! method that returns the no-op. At a site where `T` is concrete (the factory's `build()`), the
//! call resolves to the inherent method for guards and the fallback for everything else — so the
//! coercion `Arc<T> as Arc<dyn Guard<HttpContext>>` is emitted exactly when it is valid, and the
//! marker disappears.
//!
//! This proves: (1) a value probe yields the coerced role for a guard and `None` for a non-guard;
//! (2) a type-level probe (no instance, for request-scoped registration decisions) discriminates
//! the same way; (3) the coerced trait object actually runs.

use std::marker::PhantomData;
use std::sync::Arc;

use toni::async_trait;
use toni::context::HttpContext;
use toni::traits_helpers::Guard;

// ---- the probe: lives in `toni` in the real thing; defined locally for the POC --------------

/// Value probe: `Arc<T>` -> `Option<Arc<dyn Guard<HttpContext>>>`.
struct GuardProbe<T>(Arc<T>);

// Specialized inherent method — present only when `T: Guard<HttpContext>`. Inherent methods take
// resolution priority over trait methods, so this shadows the fallback whenever the bound holds.
impl<T: Guard<HttpContext> + 'static> GuardProbe<T> {
    fn http_guard_role(&self) -> Option<Arc<dyn Guard<HttpContext>>> {
        Some(self.0.clone() as Arc<dyn Guard<HttpContext>>)
    }
}

// Fallback — blanket for every `T`. Used when the inherent method above doesn't apply.
trait GuardProbeFallback {
    fn http_guard_role(&self) -> Option<Arc<dyn Guard<HttpContext>>>;
}
impl<T> GuardProbeFallback for GuardProbe<T> {
    fn http_guard_role(&self) -> Option<Arc<dyn Guard<HttpContext>>> {
        None
    }
}

/// Type-level probe: `T` -> `bool`, no instance needed. The request/transient registration path
/// needs this to decide whether to register a per-request factory before any instance exists.
struct GuardTypeProbe<T>(PhantomData<T>);

impl<T: Guard<HttpContext> + 'static> GuardTypeProbe<T> {
    fn is_http_guard(&self) -> bool {
        true
    }
}

trait GuardTypeProbeFallback {
    fn is_http_guard(&self) -> bool;
}
impl<T> GuardTypeProbeFallback for GuardTypeProbe<T> {
    fn is_http_guard(&self) -> bool {
        false
    }
}

// ---- subjects ------------------------------------------------------------------------------

#[derive(Clone)]
struct RealGuard {
    allow: bool,
}

#[async_trait]
impl Guard<HttpContext> for RealGuard {
    async fn can_activate(&self, _ctx: &HttpContext) -> bool {
        self.allow
    }
}

// A normal provider-shaped type that is NOT a guard.
#[derive(Clone)]
struct NotAGuard {
    _label: &'static str,
}

// ---- proofs --------------------------------------------------------------------------------

#[test]
fn value_probe_discriminates_guard_from_non_guard() {
    let guard = Arc::new(RealGuard { allow: true });
    let role = GuardProbe(guard).http_guard_role();
    assert!(
        role.is_some(),
        "RealGuard implements Guard<HttpContext> and must be detected + coerced"
    );

    let not_guard = Arc::new(NotAGuard { _label: "config" });
    let none = GuardProbe(not_guard).http_guard_role();
    assert!(
        none.is_none(),
        "NotAGuard does not implement Guard<HttpContext> and must yield None"
    );
}

#[test]
fn type_level_probe_discriminates_without_an_instance() {
    assert!(GuardTypeProbe::<RealGuard>(PhantomData).is_http_guard());
    assert!(!GuardTypeProbe::<NotAGuard>(PhantomData).is_http_guard());
}

#[tokio::test]
async fn coerced_trait_object_runs() {
    let guard = Arc::new(RealGuard { allow: false });
    let role: Arc<dyn Guard<HttpContext>> =
        GuardProbe(guard).http_guard_role().expect("guard detected");

    let parts = http::Request::builder().body(()).unwrap().into_parts().0;
    let ctx = HttpContext::from_parts(parts);

    assert!(
        !role.can_activate(&ctx).await,
        "the coerced trait object must run and reflect the instance's state"
    );
}
