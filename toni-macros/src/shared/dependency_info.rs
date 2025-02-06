use std::collections::HashSet;

use syn::Ident;

pub struct DependencyInfo {
  pub fields: Vec<(Ident, Ident)>,
  pub unique_types: HashSet<String>,
}