use std::collections::HashSet;

use proc_macro2::TokenStream;
use syn::{Expr, Ident, Type};

/// Specifies how dependencies should be resolved for a provider
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum DependencySource {
    /// Use explicit #[inject] and #[default] annotations
    Annotations,

    /// No annotations - all fields use Default trait
    DefaultFallback,

    /// Empty struct - no dependencies
    None,
}

pub struct DependencyInfo {
    pub fields: Vec<(Ident, Type, TokenStream)>,
    // (field_name, full_type, lookup_token_expr)
    // Example: (config, ConfigService<AppConfig>, quote!{::ulo::di::token_of::<ConfigService<AppConfig>>()})
    // These are fields marked with #[inject]
    pub owned_fields: Vec<(Ident, Type, Option<Expr>)>,
    // (field_name, type, default_expr)
    // These are fields NOT marked with #[inject]
    // default_expr is Some(expr) if #[default(expr)] is present, None otherwise
    pub unique_types: HashSet<String>,

    /// Indicates how dependencies are specified
    pub source: DependencySource,
}
