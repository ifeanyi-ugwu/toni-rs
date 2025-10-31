use std::collections::HashSet;

use syn::{Expr, Ident, Type};

pub struct DependencyInfo {
    pub fields: Vec<(Ident, Type, String)>,
    // (field_name, full_type, lookup_token)
    // Example: (config, ConfigService<AppConfig>, "ConfigService")
    // These are fields marked with #[inject]

    pub owned_fields: Vec<(Ident, Type, Option<Expr>)>,
    // (field_name, type, default_expr)
    // These are fields NOT marked with #[inject]
    // default_expr is Some(expr) if #[default(expr)] is present, None otherwise

    pub init_method: Option<String>,
    // Optional custom constructor method name (e.g., "new")
    // If present, the macro will call struct_name::init_method(injected_deps...)
    // instead of using struct literal with owned field defaults

    pub unique_types: HashSet<String>,
}
