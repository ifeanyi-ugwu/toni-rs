//! Provider-to-provider scope validation tests
//!
//! Unlike controllers (which auto-elevate), providers follow NestJS's scope rules:
//! - Singleton providers CANNOT inject Request-scoped providers (data leakage prevention)
//! - Request providers CAN inject Singleton, Request, or Transient providers
//! - Transient providers CAN inject anything (they're the narrowest scope)
//!
//! These tests verify that the runtime validation correctly catches scope violations.

#![allow(dead_code, unused_variables)]

use toni::{provider_struct, HttpAdapter};
use toni_macros::module;

// ============================================================================
// Test 1: Valid scope hierarchies (should work)
// ============================================================================

#[provider_struct(pub struct ValidSingletonProvider {})]
impl ValidSingletonProvider {
    pub fn get_id(&self) -> String {
        "singleton".to_string()
    }
}

#[provider_struct(pub struct AnotherSingletonProvider {
    dep: ValidSingletonProvider
})]
impl AnotherSingletonProvider {
    pub fn get_id(&self) -> String {
        format!("another-{}", self.dep.get_id())
    }
}

#[provider_struct(scope = "request", pub struct ValidRequestProvider {})]
impl ValidRequestProvider {
    pub fn get_id(&self) -> String {
        "request".to_string()
    }
}

#[provider_struct(scope = "request", pub struct RequestWithSingletonDep {
    dep: ValidSingletonProvider
})]
impl RequestWithSingletonDep {
    pub fn get_id(&self) -> String {
        format!("request-{}", self.dep.get_id())
    }
}

#[provider_struct(scope = "transient", pub struct TransientProvider {})]
impl TransientProvider {
    pub fn get_id(&self) -> String {
        "transient".to_string()
    }
}

#[provider_struct(scope = "transient", pub struct TransientWithAnyDeps {
    singleton: ValidSingletonProvider,
    request: ValidRequestProvider,
})]
impl TransientWithAnyDeps {
    pub fn get_id(&self) -> String {
        format!("{}-{}", self.singleton.get_id(), self.request.get_id())
    }
}

#[module(
    providers: [
        ValidSingletonProvider,
        AnotherSingletonProvider,
        ValidRequestProvider,
        RequestWithSingletonDep,
        TransientProvider,
        TransientWithAnyDeps
    ]
)]
impl ValidScopesModule {}

#[tokio::test]
async fn test_valid_scope_hierarchies() {
    use toni::toni_factory::ToniFactory;
    use toni_axum::AxumAdapter;

    // This test should pass without panicking
    let adapter = AxumAdapter::new();
    let factory = ToniFactory::new();
    let _app = factory
        .create(ValidScopesModule::module_definition(), adapter)
        .await;

    // If we get here without panicking, the test passed
}

// ============================================================================
// Test 2: Invalid - Singleton injecting Request (should panic at startup)
// ============================================================================

#[cfg(test)]
mod invalid_singleton_with_request {
    use super::*;

    #[provider_struct(scope = "request", pub struct RequestScopedService {})]
    impl RequestScopedService {
        pub fn get_id(&self) -> String {
            "request-service".to_string()
        }
    }

    // This should panic during module initialization
    #[provider_struct(pub struct InvalidSingletonProvider {
        request_dep: RequestScopedService
    })]
    impl InvalidSingletonProvider {
        pub fn get_id(&self) -> String {
            self.request_dep.get_id()
        }
    }

    #[module(
        providers: [RequestScopedService, InvalidSingletonProvider]
    )]
    impl InvalidModule {}

    #[tokio::test]
    #[should_panic(expected = "Scope validation error")]
    async fn test_singleton_cannot_inject_request() {
        use toni::toni_factory::ToniFactory;
        use toni_axum::AxumAdapter;

        // This should panic with a scope validation error during module initialization
        let adapter = AxumAdapter::new();
        let factory = ToniFactory::new();
        let _app = factory
            .create(InvalidModule::module_definition(), adapter)
            .await;
    }
}

// ============================================================================
// Test 3: Valid - Singleton injecting Transient (allowed per NestJS behavior)
// ============================================================================

#[provider_struct(scope = "transient", pub struct TransientService {})]
impl TransientService {
    pub fn get_id(&self) -> String {
        "transient-service".to_string()
    }
}

// Singleton CAN inject Transient - this is valid per NestJS
#[provider_struct(pub struct ValidSingletonWithTransient {
    transient_dep: TransientService
})]
impl ValidSingletonWithTransient {
    pub fn get_id(&self) -> String {
        self.transient_dep.get_id()
    }
}

#[module(
    providers: [TransientService, ValidSingletonWithTransient]
)]
impl ValidModule2 {}

#[tokio::test]
async fn test_singleton_can_inject_transient() {
    use toni::toni_factory::ToniFactory;
    use toni_axum::AxumAdapter;

    // This should NOT panic - Singleton + Transient is allowed
    let adapter = AxumAdapter::new();
    let factory = ToniFactory::new();
    let _app = factory
        .create(ValidModule2::module_definition(), adapter)
        .await;

    // If we get here without panicking, the test passed
}

// ============================================================================
// Test 4: Valid - Request injecting Transient (allowed per NestJS behavior)
// ============================================================================

#[provider_struct(scope = "transient", pub struct TransientService2 {})]
impl TransientService2 {
    pub fn get_id(&self) -> String {
        "transient-service-2".to_string()
    }
}

// Request CAN inject Transient - this is valid per NestJS
#[provider_struct(scope = "request", pub struct ValidRequestWithTransient {
    transient_dep: TransientService2
})]
impl ValidRequestWithTransient {
    pub fn get_id(&self) -> String {
        self.transient_dep.get_id()
    }
}

#[module(
    providers: [TransientService2, ValidRequestWithTransient]
)]
impl ValidModule3 {}

#[tokio::test]
async fn test_request_can_inject_transient() {
    use toni::toni_factory::ToniFactory;
    use toni_axum::AxumAdapter;

    // This should NOT panic - Request + Transient is allowed
    let adapter = AxumAdapter::new();
    let factory = ToniFactory::new();
    let _app = factory
        .create(ValidModule3::module_definition(), adapter)
        .await;

    // If we get here without panicking, the test passed
}

// ============================================================================
// Test 5: Complex valid hierarchy (multiple levels)
// ============================================================================

#[provider_struct(pub struct BaseService {})]
impl BaseService {
    pub fn get_value(&self) -> i32 {
        42
    }
}

#[provider_struct(pub struct MiddleService {
    base: BaseService
})]
impl MiddleService {
    pub fn get_value(&self) -> i32 {
        self.base.get_value() * 2
    }
}

#[provider_struct(scope = "request", pub struct TopService {
    middle: MiddleService,
    base: BaseService
})]
impl TopService {
    pub fn get_value(&self) -> i32 {
        self.middle.get_value() + self.base.get_value()
    }
}

#[module(
    providers: [BaseService, MiddleService, TopService]
)]
impl ComplexValidModule {}

#[tokio::test]
async fn test_complex_valid_hierarchy() {
    use toni::toni_factory::ToniFactory;
    use toni_axum::AxumAdapter;

    // This should work: Singleton -> Singleton -> Request is valid
    let adapter = AxumAdapter::new();
    let factory = ToniFactory::new();
    let _app = factory
        .create(ComplexValidModule::module_definition(), adapter)
        .await;

    // If we get here without panicking, the test passed
}

// ============================================================================
// Test 6: Invalid - Explicit singleton with Request dependency (should panic)
// ============================================================================

#[cfg(test)]
mod explicit_singleton_violation {
    use super::*;

    #[provider_struct(scope = "request", pub struct ExplicitRequestService {})]
    impl ExplicitRequestService {
        pub fn get_id(&self) -> String {
            "explicit-request".to_string()
        }
    }

    // User EXPLICITLY set scope = "singleton", but it has Request dependency
    #[provider_struct(scope = "singleton", pub struct ExplicitSingletonWithRequest {
        request_dep: ExplicitRequestService
    })]
    impl ExplicitSingletonWithRequest {
        pub fn get_id(&self) -> String {
            self.request_dep.get_id()
        }
    }

    #[module(
        providers: [ExplicitRequestService, ExplicitSingletonWithRequest]
    )]
    impl ExplicitModule {}

    #[tokio::test]
    #[should_panic(expected = "Scope validation error")]
    async fn test_explicit_singleton_with_request_fails() {
        use toni::toni_factory::ToniFactory;
        use toni_axum::AxumAdapter;

        // Should panic even though user explicitly set singleton
        let adapter = AxumAdapter::new();
        let factory = ToniFactory::new();
        let _app = factory
            .create(ExplicitModule::module_definition(), adapter)
            .await;
    }
}
