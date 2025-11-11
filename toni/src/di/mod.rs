//! Dependency Injection utilities and types
//!
//! This module provides utilities for working with the Toni DI system,
//! including type-safe tokens for identifying providers.

pub mod token;

pub use token::{APP_GUARD, APP_INTERCEPTOR, APP_MIDDLEWARE, APP_PIPE, Token};
