use std::sync::Arc;

use crate::traits_helpers::{ErrorHandler, GuardEntry, InterceptorEntry, PipeEntry};

pub struct EnhancerMetadata {
    pub guards: Vec<GuardEntry>,
    pub interceptors: Vec<InterceptorEntry>,
    pub pipes: Vec<PipeEntry>,
    pub error_handlers: Vec<Arc<dyn ErrorHandler>>,
}
