use std::collections::HashSet;

use proc_macro2::TokenStream;
use syn::{Ident, ImplItem, ItemImpl, Result};

use crate::shared::dependency_info::DependencyInfo;
use crate::shared::metadata_info::MetadataInfo;

use crate::controller::controller::generate_controller_and_metadata;
use crate::utils::controller_utils::find_http_method_attribute;

pub fn process_impl_functions(
    impl_block: &ItemImpl,
    dependencies: &DependencyInfo,
    struct_name: &syn::Ident,
    trait_name: &Ident,
    prefix_path: &str,
) -> Result<(Vec<TokenStream>, Vec<MetadataInfo>, HashSet<String>)> {
    let mut controllers = Vec::new();
    let mut metadata = Vec::new();
    let mut unique_dependencies = HashSet::new();
    for item in &impl_block.items {
        if let ImplItem::Fn(method) = item {
            if let Some(attr) = find_http_method_attribute(&method.attrs) {
                let (controller, meta) = generate_controller_and_metadata(
                    method,
                    &dependencies.fields,
                    struct_name,
                    &mut unique_dependencies,
                    trait_name,
                    &prefix_path.to_string(),
                    attr,
                )
                .unwrap();

                controllers.push(controller);
                metadata.push(meta);
            }
        }
    }

    Ok((controllers, metadata, unique_dependencies))
}
