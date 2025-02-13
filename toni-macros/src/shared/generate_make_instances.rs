use proc_macro2::TokenStream;
use quote::quote;

use super::metadata_info::MetadataInfo;

pub fn generate_make_instances(
    structs_metadata: Vec<MetadataInfo>,
    manager_name: &String,
    self_dependency: bool,
) -> Vec<TokenStream> {
    let (independent_structs, dependent_structs): (Vec<_>, Vec<_>) = {
        let has_manager_dependency = |instance: &&MetadataInfo| {
            instance
                .dependencies
                .iter()
                .any(|(_, dep_key)| dep_key.contains(manager_name))
        };

        structs_metadata
            .iter()
            .partition(|instance| !has_manager_dependency(instance))
    };

    let ordered_structs = independent_structs
        .into_iter()
        .chain(dependent_structs.into_iter());

    ordered_structs
        .map(|instance_metadata| {
            let struct_ident  = &instance_metadata.struct_name;
            let struct_name_string = struct_ident.to_string();
            let dependencies = &instance_metadata.dependencies;
            
            let field_injections = dependencies
                .iter()
                .map(|(field_name, dependency_key)| {
                    let error_message = format!(
                        "Missing dependency '{}' for field '{}' in '{}'",
                        dependency_key, field_name, &struct_name_string
                    );
                    let error_handling = if self_dependency {
                        quote! {
                            unwrap_or_else(|| {
                                providers.get(#dependency_key).expect(#error_message)
                            })
                        }
                    } else {
                        quote! { expect(#error_message) }
                    };
                    quote! {
                        #field_name: dependencies
                            .get(#dependency_key)
                            .#error_handling
                            .clone()
                    }
                })
                .collect::<Vec<_>>();

            quote! {
                (
                    #struct_name_string.to_string(),
                    ::std::sync::Arc::new(
                        Box::new(#struct_ident {
                            #(#field_injections),*
                        })
                    )
                )
            }
        })
        .collect::<Vec<_>>()
}
