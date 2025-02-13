use proc_macro2::TokenStream;
use quote::quote;

use super::metadata_info::MetadataInfo;

pub fn generate_make_instances(
    structs_metadata: Vec<MetadataInfo>,
    manager_name: &String,
    self_dependency: bool,
) -> Vec<TokenStream> {
    let (without_dependency, with_dependency): (Vec<_>, Vec<_>) =
        structs_metadata.iter().partition(|instance_metadata| {
            !instance_metadata
                .dependencies
                .iter()
                .any(|d| d.1.contains(manager_name))
        });

    let ordered_structs = without_dependency.iter().chain(with_dependency.iter());

    ordered_structs
        .map(|instance_metadata| {
            let struct_name = &instance_metadata.struct_name;
            let struct_name_string = struct_name.to_string();
            let dependencies = &instance_metadata.dependencies;
            let field_injections = dependencies
                .iter()
                .map(|(field_name, dependency_key)| {
                    let error_message = format!(
                        "Missing dependency '{}' for field '{}' in '{}'",
                        dependency_key, field_name, &struct_name_string
                    );
                    let mut handle_error = quote! {
                        expect(#error_message)
                    };
                    if self_dependency {
                        handle_error = quote! {
                            unwrap_or_else( || {
                                providers.get(#dependency_key).expect(
                                    #error_message
                                )
                            })
                        };
                    }
                    quote! {
                        #field_name: dependencies
                            .get(#dependency_key)
                            .#handle_error
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
