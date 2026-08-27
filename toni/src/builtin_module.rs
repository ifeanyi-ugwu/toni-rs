//! Built-in Global Module
//!
//! This module provides built-in providers that should be globally available
//! to all modules without requiring explicit imports.

use crate::RequestFactory;
use crate::extension::ExtensionsFactory;
use crate::traits_helpers::{ControllerFactory, ModuleMetadata, ProviderFactory};

/// Built-in global module that provides core framework functionality
///
/// Currently provides:
/// - Request: HTTP request data access for handlers
/// - Extensions: the request's extension bag, for code below the handler
pub struct BuiltinModule;

impl ModuleMetadata for BuiltinModule {
    fn identity(&self) -> crate::ModuleIdentity {
        crate::ModuleIdentity::named("ToniBuiltinModule")
    }

    fn is_global(&self) -> bool {
        true // Global module - exports are available everywhere
    }

    fn imports(&self) -> Option<Vec<Box<dyn ModuleMetadata>>> {
        None
    }

    fn controllers(&self) -> Option<Vec<Box<dyn ControllerFactory>>> {
        None
    }

    fn providers(&self) -> Option<Vec<Box<dyn ProviderFactory>>> {
        Some(vec![Box::new(RequestFactory), Box::new(ExtensionsFactory)])
    }

    fn exports(&self) -> Option<Vec<String>> {
        Some(vec![
            crate::di::token_of::<crate::request::Request>(),
            crate::di::token_of::<crate::context::Extensions>(),
        ])
    }
}
