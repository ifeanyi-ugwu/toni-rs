use std::collections::HashSet;

use proc_macro2::TokenStream;
use syn::{Ident, ImplItem, ItemImpl, Result};

use crate::shared::dependency_info::DependencyInfo;
use crate::shared::metadata_info::MetadataInfo;

use super::provider::generate_provider_and_metadata;

pub fn process_impl_functions(
    impl_block: &ItemImpl,
    dependencies: &DependencyInfo,
    struct_name: &syn::Ident,
    trait_name: &Ident,
) -> Result<(Vec<TokenStream>, Vec<MetadataInfo>, HashSet<String>)> {
    let mut providers = Vec::new();
    let mut metadata = Vec::new();
    let mut unique_dependencies = HashSet::new();
    for item in &impl_block.items {
        if let ImplItem::Fn(method) = item {
            let (provider, meta) = generate_provider_and_metadata(
                method,
                &dependencies.fields,
                struct_name,
                &mut unique_dependencies,
                trait_name,
            )
            .unwrap();

            providers.push(provider);
            metadata.push(meta);
        }
    }

    Ok((providers, metadata, unique_dependencies))
}
