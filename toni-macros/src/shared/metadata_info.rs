use syn::Ident;

pub struct MetadataInfo {
  pub struct_name: Ident,
  pub dependencies: Vec<(Ident, String)>,
}