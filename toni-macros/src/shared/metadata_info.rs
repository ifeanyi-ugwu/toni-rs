use proc_macro2::TokenStream;
use syn::Ident;

#[derive(Debug)]
pub struct MetadataInfo {
    pub struct_name: Ident,
    pub dependencies: Vec<(Ident, TokenStream)>,
}
