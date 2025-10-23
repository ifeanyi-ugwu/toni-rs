use proc_macro2::TokenStream;
use syn::{Ident, ImplItem, ItemImpl, Result};

use crate::{
    controller_macro::controller::generate_controller_and_metadata,
    enhancer::enhancer::get_enhancers_attr,
    markers_params::{
        get_marker_params::get_marker_params,
        remove_marker_controller_fn::remove_marker_in_controller_fn_args,
    },
    shared::{dependency_info::DependencyInfo, metadata_info::MetadataInfo},
    utils::controller_utils::find_macro_attribute,
};

pub fn process_impl_functions(
    impl_block: &mut ItemImpl,
    dependencies: &mut DependencyInfo,
    struct_name: &syn::Ident,
    trait_name: &Ident,
    prefix_path: &str,
) -> Result<(Vec<TokenStream>, Vec<MetadataInfo>)> {
    let mut controllers = Vec::new();
    let mut metadata = Vec::new();
    for item in &mut impl_block.items {
        if let ImplItem::Fn(method) = item {
            if let Some(attr) = find_macro_attribute(&method.attrs, "http_method".to_owned()) {
                let enhancers_attr = get_enhancers_attr(&method.attrs)?;
                //println!("enhancers_attr: {:?}", enhancers_attr);

                let marker_params = get_marker_params(method)?;

                let (controller, meta) = generate_controller_and_metadata(
                    method,
                    struct_name,
                    dependencies,
                    trait_name,
                    &prefix_path.to_string(),
                    attr,
                    enhancers_attr,
                    marker_params,
                )?;

                controllers.push(controller);
                metadata.push(meta);

                remove_marker_in_controller_fn_args(method);
            }
        }
    }
    Ok((controllers, metadata))
}
