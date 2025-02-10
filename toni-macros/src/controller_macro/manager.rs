use std::collections::HashSet;

use proc_macro2::TokenStream;
use quote::quote;
use syn::Ident;

use crate::shared::{
    generate_make_instances::generate_make_instances, metadata_info::MetadataInfo,
};

pub fn generate_manager(
    struct_name: &Ident,
    controllers_metadata: Vec<MetadataInfo>,
    unique_dependencies: HashSet<String>,
) -> TokenStream {
    let manager_struct_name = Ident::new(&format!("{}Manager", struct_name), struct_name.span());
    let manager_name = struct_name.to_string();
    let dependencies_name = unique_dependencies.iter().map(|dependency| {
        quote! { #dependency.to_string() }
    });
    let controllers_instances = generate_make_instances(controllers_metadata);
    quote! {
        pub struct #manager_struct_name;

        impl ::tonirs_core::traits_helpers::Controller for #manager_struct_name {
            fn get_all_controllers(&self, dependencies: &::rustc_hash::FxHashMap<String, ::std::sync::Arc<Box<dyn ::tonirs_core::traits_helpers::ProviderTrait>>>) -> ::rustc_hash::FxHashMap<String, ::std::sync::Arc<Box<dyn ::tonirs_core::traits_helpers::ControllerTrait>>> {                
                let mut controllers = ::rustc_hash::FxHashMap::default();
                #(
                    let (key, value): (String, ::std::sync::Arc<Box<dyn ::tonirs_core::traits_helpers::ControllerTrait>>) = #controllers_instances;
                    controllers.insert(key, value);
                )*
                controllers
            }
            fn get_name(&self) -> String {
                #manager_name.to_string()
            }
            fn get_token(&self) -> String {
                #manager_name.to_string()
            }
            fn get_dependencies(&self) -> Vec<String> {
                vec![#(#dependencies_name),*]
            }
        }
    }
}
