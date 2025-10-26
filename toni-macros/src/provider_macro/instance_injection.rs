//! Instance Injection Implementation (New Pattern)
//!
//! Architecture:
//! 1. User struct with REAL fields (unchanged)
//! 2. Provider wrapper that lazily creates instances with dependency resolution
//! 3. Manager that returns the wrapper (not the final instance)
//!
//! Example transformation:
//! ```rust,ignore
//! // User code:
//! #[provider_struct(pub struct AppService { config: ConfigService<AppConfig> })]
//! impl AppService {
//!     pub fn method(&self) -> String {
//!         self.config.get().app_name
//!     }
//! }
//!
//! // Generated:
//! #[derive(Clone)]
//! pub struct AppService {
//!     config: ConfigService<AppConfig>
//! }
//!
//! struct AppServiceProvider {
//!     dependencies: FxHashMap<String, Arc<Box<dyn ProviderTrait>>>
//! }
//!
//! impl ProviderTrait for AppServiceProvider {
//!     async fn execute(&self, _: Vec<...>) -> Box<dyn Any> {
//!         // Resolve dependencies (can await here!)
//!         let config = *self.dependencies.get("ConfigService")
//!             .unwrap().execute(vec![]).await.downcast().unwrap();
//!
//!         // Create instance with real fields
//!         Box::new(AppService { config })
//!     }
//! }
//! ```

use proc_macro2::TokenStream;
use quote::quote;
use syn::{Ident, ItemImpl, ItemStruct, Result};

use crate::shared::dependency_info::DependencyInfo;

pub fn generate_instance_provider_system(
    struct_attrs: &ItemStruct,
    impl_block: &ItemImpl,
    dependencies: &DependencyInfo,
) -> Result<TokenStream> {
    let struct_name = &struct_attrs.ident;

    let struct_with_clone = add_clone_derive(struct_attrs);

    let impl_def = impl_block.clone();

    let provider_wrapper = generate_provider_wrapper(struct_name, dependencies);

    let manager = generate_manager(struct_name, dependencies);

    Ok(quote! {
        #[allow(dead_code)]
        #struct_with_clone

        #[allow(dead_code)]
        #impl_def

        #provider_wrapper
        #manager
    })
}

fn add_clone_derive(struct_attrs: &ItemStruct) -> ItemStruct {
    let mut struct_def = struct_attrs.clone();

    let has_clone = struct_def.attrs.iter().any(|attr| {
        if attr.path().is_ident("derive") {
            //TODO: Would probably need to parse derive contents properly
            false
        } else {
            false
        }
    });

    if !has_clone {
        let clone_derive: syn::Attribute = syn::parse_quote! {
            #[derive(Clone)]
        };
        struct_def.attrs.push(clone_derive);
    }

    struct_def
}

fn generate_provider_wrapper(struct_name: &Ident, dependencies: &DependencyInfo) -> TokenStream {
    let provider_name = Ident::new(&format!("{}Provider", struct_name), struct_name.span());
    let struct_token = struct_name.to_string();

    let (field_resolutions, field_names) = generate_field_resolutions(dependencies);

    quote! {
        struct #provider_name {
            dependencies: ::toni::FxHashMap<
                String,
                ::std::sync::Arc<Box<dyn ::toni::traits_helpers::ProviderTrait>>
            >,
        }

        #[::toni::async_trait]
        impl ::toni::traits_helpers::ProviderTrait for #provider_name {
            async fn execute(
                &self,
                _params: Vec<Box<dyn ::std::any::Any + Send>>
            ) -> Box<dyn ::std::any::Any + Send> {
                #(#field_resolutions)*

                let instance = #struct_name {
                    #(#field_names),*
                };

                Box::new(instance)
            }

            fn get_token(&self) -> String {
                #struct_token.to_string()
            }

            fn get_token_manager(&self) -> String {
                #struct_token.to_string()
            }
        }
    }
}

fn generate_field_resolutions(dependencies: &DependencyInfo) -> (Vec<TokenStream>, Vec<Ident>) {
    let mut resolutions = Vec::new();
    let mut field_names = Vec::new();

    for (field_name, full_type, lookup_token) in &dependencies.fields {
        let error_msg = format!(
            "Missing dependency '{}' for field '{}'",
            lookup_token, field_name
        );

        let resolution = quote! {
            let #field_name: #full_type = {
                let provider = self.dependencies
                    .get(#lookup_token)
                    .expect(#error_msg);

                let any_box = provider.execute(vec![]).await;

                *any_box.downcast::<#full_type>()
                    .unwrap_or_else(|_| panic!(
                        "Failed to downcast '{}' to {}",
                        #lookup_token,
                        stringify!(#full_type)
                    ))
            };
        };

        resolutions.push(resolution);
        field_names.push(field_name.clone());
    }

    (resolutions, field_names)
}

fn generate_manager(struct_name: &Ident, dependencies: &DependencyInfo) -> TokenStream {
    let manager_name = Ident::new(&format!("{}Manager", struct_name), struct_name.span());
    let provider_name = Ident::new(&format!("{}Provider", struct_name), struct_name.span());
    let struct_token = struct_name.to_string();

    let dependency_tokens: Vec<_> = dependencies
        .fields
        .iter()
        .map(|(_, _full_type, lookup_token)| {
            quote! { #lookup_token.to_string() }
        })
        .collect();

    quote! {
        pub struct #manager_name;

        #[::toni::async_trait]
        impl ::toni::traits_helpers::Provider for #manager_name {
            async fn get_all_providers(
                &self,
                dependencies: &::toni::FxHashMap<
                    String,
                    ::std::sync::Arc<Box<dyn ::toni::traits_helpers::ProviderTrait>>
                >,
            ) -> ::toni::FxHashMap<
                String,
                ::std::sync::Arc<Box<dyn ::toni::traits_helpers::ProviderTrait>>
            > {
                let mut providers = ::toni::FxHashMap::default();

                // Create provider wrapper (not the final instance!)
                // The wrapper will create the instance lazily when execute() is called
                let provider_wrapper = #provider_name {
                    dependencies: dependencies.clone(),
                };

                providers.insert(
                    #struct_token.to_string(),
                    ::std::sync::Arc::new(Box::new(provider_wrapper) as Box<dyn ::toni::traits_helpers::ProviderTrait>)
                );

                providers
            }

            fn get_name(&self) -> String {
                #struct_token.to_string()
            }

            fn get_token(&self) -> String {
                #struct_token.to_string()
            }

            fn get_dependencies(&self) -> Vec<String> {
                vec![#(#dependency_tokens),*]
            }
        }
    }
}
