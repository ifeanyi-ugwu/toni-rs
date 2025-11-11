use std::collections::HashSet;

use proc_macro2::TokenStream;
use syn::{Expr, Ident, Type};

/// Specifies how dependencies should be resolved for a provider
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum DependencySource {
    /// Use a custom constructor method (init or new())
    /// String contains the method name
    Constructor(String),

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
    // Example: (config, ConfigService<AppConfig>, quote!{format!("ConfigService<{}>", std::any::type_name::<AppConfig>())})
    // These are fields marked with #[inject]
    pub owned_fields: Vec<(Ident, Type, Option<Expr>)>,
    // (field_name, type, default_expr)
    // These are fields NOT marked with #[inject]
    // default_expr is Some(expr) if #[default(expr)] is present, None otherwise
    pub init_method: Option<String>,
    // Optional custom constructor method name (e.g., "new")
    // If present, the macro will call struct_name::init_method(injected_deps...)
    // instead of using struct literal with owned field defaults
    pub constructor_params: Vec<(Ident, Type, TokenStream)>,
    // (param_name, param_type, lookup_token_expr)
    // Parameters of the constructor method (init or new())
    // These are automatically extracted from the method signature
    // Example: fn new(config: ConfigService<AppConfig>) -> Self
    //   → [(config, ConfigService<AppConfig>, quote!{format!("ConfigService<{}>", ...)})]
    pub unique_types: HashSet<String>,

    /// Indicates how dependencies are specified
    pub source: DependencySource,
}
