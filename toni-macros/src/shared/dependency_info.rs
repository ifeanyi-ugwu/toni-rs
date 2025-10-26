use std::collections::HashSet;

use syn::{Ident, Type};

pub struct DependencyInfo {
    pub fields: Vec<(Ident, Type, String)>,
    // (field_name, full_type, lookup_token)
    // Example: (config, ConfigService<AppConfig>, "ConfigService")
    pub unique_types: HashSet<String>,
}
