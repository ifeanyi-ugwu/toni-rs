use crate::traits_helpers::{
    HttpErrorHandlerArc, HttpGuardEntry, HttpInterceptorEntry, HttpPipeEntry,
};

/// Resolved HTTP enhancer pipeline for a single route.
///
/// Entries are typed for `HttpContext` — the dispatcher walks them directly
/// without any runtime protocol switch.
pub struct EnhancerMetadata {
    pub guards: Vec<HttpGuardEntry>,
    pub interceptors: Vec<HttpInterceptorEntry>,
    pub pipes: Vec<HttpPipeEntry>,
    pub error_handlers: Vec<HttpErrorHandlerArc>,
}
