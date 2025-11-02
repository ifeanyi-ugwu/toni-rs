# Integration Tests

This crate contains integration tests for the Toni framework. It tests the interactions between different components (toni, toni-config, toni-axum, etc.) to ensure they work together correctly.

## Purpose

This crate exists to:

- Keep core crates lean by avoiding heavy dev-dependencies in production crates
- Test cross-crate interactions in a realistic environment
- Provide end-to-end tests with actual HTTP servers
- Prevent circular dev-dependencies (e.g., toni depending on toni-axum for tests)

## Test Organization

Tests are organized by functionality:

### DI System Tests (`tests/*`)

- **attribute_syntax.rs** - Tests for `#[injectable]` attribute syntax
- **custom_init.rs** - Custom initialization methods (`init = "method_name"`)
- **instance_injection.rs** - Basic instance injection
- **owned_fields.rs** - Providers with `#[inject]` and `#[default]` fields
- **scope_bubbling.rs** - Scope elevation warnings
- **scope_validation.rs** - Provider-to-provider scope validation rules
- **scopes.rs** - Singleton/Request/Transient scope compilation
- **simple_provider.rs** - Simple provider injection
- **transient_scope.rs** - Transient scope behavior

### Module System Tests (`tests/*`)

- **global_modules.rs** - Global module functionality (`global: true`)

### E2E HTTP Tests (`tests/*`)

- **async_controllers.rs** - Async controller methods
- **config_injection.rs** - ConfigService injection with real HTTP server
- **controller_scopes.rs** - Controller scope behavior (Singleton vs Request)
- **extensions_and_from_request.rs** - Extensions and `from_request` pattern
- **request_provider.rs** - Built-in Request provider

## Running Tests

```bash
# Run all integration tests
cargo test -p integration-tests

# Run a specific test
cargo test -p integration-tests --test global_modules

# Run with output
cargo test -p integration-tests -- --nocapture
```

## Note

This crate is marked with `publish = false` and is not intended to be published to crates.io. It exists solely for testing purposes within the workspace.
