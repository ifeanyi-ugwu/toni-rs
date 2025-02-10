use proc_macro2::TokenStream;
use quote::quote;

use super::metadata_info::MetadataInfo;

pub fn generate_make_instances(structs_metadata: Vec<MetadataInfo>) -> Vec<TokenStream> {
    structs_metadata
        .iter()
        .map(|instance_metadata| {
            let struct_name = &instance_metadata.struct_name;
            let struct_name_string = struct_name.to_string();
            let dependencies = &instance_metadata.dependencies;
            let field_injections = dependencies
                .iter()
                .map(|(field_name, dependency_key)| {
                    let error_message = format!(
                        "Missing dependency '{}' for field '{}' in '{}'",
                        dependency_key,
                        field_name,
                        &struct_name_string
                    );

                    quote! {
                        #field_name: dependencies
                            .get(#dependency_key)
                            .expect(#error_message)
                            .clone()
                    }
                })
                .collect::<Vec<_>>();

            quote! {
                (
                    String::from(#struct_name_string),
                    ::std::sync::Arc::new(
                        Box::new(#struct_name {
                            #(#field_injections),*
                        })
                    )
                )
            }
        })
        .collect::<Vec<_>>()
}
