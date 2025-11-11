//! Singleton Instance Injection Implementation
//!
//! Architecture:
//! 1. User struct with REAL fields (unchanged)
//! 2. Provider wrapper that stores Arc<Instance> (created once at startup)
//! 3. Manager that creates the instance eagerly with dependency resolution
//!
//! Example transformation (Singleton):
//! ```rust,ignore
//! // User code:
//! #[injectable(pub struct AppService { config: ConfigService<AppConfig> })]
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
//!     instance: Arc<AppService>  // Created once at startup!
//! }
//!
//! impl ProviderTrait for AppServiceProvider {
//!     async fn execute(&self, _: Vec<...>) -> Box<dyn Any> {
//!         // Just clone the Arc - zero allocation!
//!         Box::new(self.instance.clone())
//!     }
//!
//!     fn get_scope(&self) -> ProviderScope {
//!         ProviderScope::Singleton
//!     }
//! }
//!
//! // Manager creates instance at startup:
//! impl Provider for AppServiceManager {
//!     async fn get_all_providers(&self, dependencies: &FxHashMap<...>) -> FxHashMap<...> {
//!         // Resolve dependencies (await naturally)
//!         let config = dependencies.get("ConfigService").execute(vec![]).await...;
//!
//!         // Create instance ONCE
//!         let instance = Arc::new(AppService { config });
//!
//!         // Wrap in provider
//!         let provider = AppServiceProvider { instance };
//!         providers.insert("AppService", Arc::new(Box::new(provider)));
//!     }
//! }
//! ```

use proc_macro2::TokenStream;
use quote::quote;
use syn::{Ident, ItemImpl, ItemStruct, Result};

use crate::shared::{dependency_info::DependencyInfo, scope_parser::ProviderScope};

pub fn generate_instance_provider_system(
    struct_attrs: &ItemStruct,
    impl_block: &ItemImpl,
    dependencies: &DependencyInfo,
    scope: ProviderScope,
) -> Result<TokenStream> {
    let struct_name = &struct_attrs.ident;

    let struct_with_clone = add_clone_derive(struct_attrs);

    let impl_def = impl_block.clone();

    let provider_wrapper = generate_provider_wrapper(struct_name, dependencies, scope);

    let manager = generate_manager(struct_name, dependencies, scope);

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
        // Add both Clone and Injectable derives
        // Injectable registers #[inject] and #[default] as valid attributes
        let derives: syn::Attribute = syn::parse_quote! {
            #[derive(Clone, ::toni::Injectable)]
        };
        struct_def.attrs.push(derives);
    } else {
        // Just add Injectable
        let injectable_derive: syn::Attribute = syn::parse_quote! {
            #[derive(::toni::Injectable)]
        };
        struct_def.attrs.push(injectable_derive);
    }

    struct_def
}

fn generate_provider_wrapper(
    struct_name: &Ident,
    dependencies: &DependencyInfo,
    scope: ProviderScope,
) -> TokenStream {
    match scope {
        ProviderScope::Singleton => generate_singleton_provider(struct_name),
        ProviderScope::Request => generate_request_provider(struct_name, dependencies),
        ProviderScope::Transient => generate_transient_provider(struct_name, dependencies),
    }
}

fn generate_singleton_provider(struct_name: &Ident) -> TokenStream {
    let provider_name = Ident::new(&format!("{}Provider", struct_name), struct_name.span());
    let struct_token = struct_name.to_string();

    quote! {
        struct #provider_name {
            instance: ::std::sync::Arc<#struct_name>,
        }

        #[::toni::async_trait]
        impl ::toni::traits_helpers::ProviderTrait for #provider_name {
            async fn execute(
                &self,
                _params: Vec<Box<dyn ::std::any::Any + Send>>,
                _req: Option<&::toni::http_helpers::HttpRequest>,
            ) -> Box<dyn ::std::any::Any + Send> {
                Box::new((*self.instance).clone())
            }

            fn get_token(&self) -> String {
                #struct_token.to_string()
            }

            fn get_token_manager(&self) -> String {
                #struct_token.to_string()
            }

            fn get_scope(&self) -> ::toni::ProviderScope {
                ::toni::ProviderScope::Singleton
            }
        }
    }
}

fn generate_request_provider(struct_name: &Ident, dependencies: &DependencyInfo) -> TokenStream {
    let provider_name = Ident::new(&format!("{}Provider", struct_name), struct_name.span());
    let struct_token = struct_name.to_string();

    let (field_resolutions, field_names) = generate_field_resolutions(dependencies);

    // Check if this uses from_request pattern
    let is_from_request = dependencies
        .init_method
        .as_ref()
        .map(|m| m == "from_request")
        .unwrap_or(false);

    // Generate struct instantiation code (either custom init or struct literal)
    let struct_instantiation = if let Some(init_fn) = &dependencies.init_method {
        let init_ident = syn::Ident::new(init_fn, struct_name.span());

        if is_from_request {
            // Special case: from_request gets HttpRequest as first parameter
            if field_names.is_empty() {
                // No dependencies, just HttpRequest
                quote! {
                    #struct_name::#init_ident(
                        _req.expect("from_request requires HttpRequest")
                    )
                }
            } else {
                // Has dependencies + HttpRequest
                quote! {
                    #struct_name::#init_ident(
                        _req.expect("from_request requires HttpRequest"),
                        #(#field_names),*
                    )
                }
            }
        } else {
            // Normal custom init
            quote! {
                #struct_name::#init_ident(#(#field_names),*)
            }
        }
    } else {
        let owned_field_inits: Vec<_> = dependencies
            .owned_fields
            .iter()
            .map(|(field_name, field_type, default_expr)| {
                if let Some(expr) = default_expr {
                    quote! { #field_name: #expr }
                } else {
                    quote! { #field_name: <#field_type>::default() }
                }
            })
            .collect();

        quote! {
            #struct_name {
                #(#field_names,)*
                #(#owned_field_inits),*
            }
        }
    };

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
                _params: Vec<Box<dyn ::std::any::Any + Send>>,
                _req: Option<&::toni::http_helpers::HttpRequest>,
            ) -> Box<dyn ::std::any::Any + Send> {
                // Resolve dependencies per request
                #(#field_resolutions)*

                // Create new instance per request
                let instance = #struct_instantiation;

                Box::new(instance)
            }

            fn get_token(&self) -> String {
                #struct_token.to_string()
            }

            fn get_token_manager(&self) -> String {
                #struct_token.to_string()
            }

            fn get_scope(&self) -> ::toni::ProviderScope {
                ::toni::ProviderScope::Request
            }
        }
    }
}

fn generate_transient_provider(struct_name: &Ident, dependencies: &DependencyInfo) -> TokenStream {
    let provider_name = Ident::new(&format!("{}Provider", struct_name), struct_name.span());
    let struct_token = struct_name.to_string();

    let (field_resolutions, field_names) = generate_field_resolutions(dependencies);

    // Generate struct instantiation code (either custom init or struct literal)
    let struct_instantiation = if let Some(init_fn) = &dependencies.init_method {
        let init_ident = syn::Ident::new(init_fn, struct_name.span());
        quote! {
            #struct_name::#init_ident(#(#field_names),*)
        }
    } else {
        let owned_field_inits: Vec<_> = dependencies
            .owned_fields
            .iter()
            .map(|(field_name, field_type, default_expr)| {
                if let Some(expr) = default_expr {
                    quote! { #field_name: #expr }
                } else {
                    quote! { #field_name: <#field_type>::default() }
                }
            })
            .collect();

        quote! {
            #struct_name {
                #(#field_names,)*
                #(#owned_field_inits),*
            }
        }
    };

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
                _params: Vec<Box<dyn ::std::any::Any + Send>>,
                _req: Option<&::toni::http_helpers::HttpRequest>,
            ) -> Box<dyn ::std::any::Any + Send> {
                // Resolve dependencies every time
                #(#field_resolutions)*

                // Create new instance every time
                let instance = #struct_instantiation;

                Box::new(instance)
            }

            fn get_token(&self) -> String {
                #struct_token.to_string()
            }

            fn get_token_manager(&self) -> String {
                #struct_token.to_string()
            }

            fn get_scope(&self) -> ::toni::ProviderScope {
                ::toni::ProviderScope::Transient
            }
        }
    }
}

/// Generate field resolutions for Request/Transient providers (uses self.dependencies)
fn generate_field_resolutions(dependencies: &DependencyInfo) -> (Vec<TokenStream>, Vec<Ident>) {
    let mut resolutions = Vec::new();
    let mut field_names = Vec::new();

    for (field_name, full_type, lookup_token_expr) in &dependencies.fields {
        let field_name_str = field_name.to_string();

        let resolution = quote! {
            let #field_name: #full_type = {
                let __lookup_token = #lookup_token_expr;
                let provider = self.dependencies
                    .get(&__lookup_token)
                    .unwrap_or_else(|| panic!(
                        "Missing dependency '{}' for field '{}'",
                        __lookup_token, #field_name_str
                    ));

                let any_box = provider.execute(vec![], _req).await;

                *any_box.downcast::<#full_type>()
                    .unwrap_or_else(|_| panic!(
                        "Failed to downcast '{}' to {}",
                        __lookup_token,
                        stringify!(#full_type)
                    ))
            };
        };

        resolutions.push(resolution);
        field_names.push(field_name.clone());
    }

    (resolutions, field_names)
}

/// Generate field resolutions for Singleton manager (uses dependencies parameter)
fn generate_manager_field_resolutions(
    dependencies: &DependencyInfo,
) -> (Vec<TokenStream>, Vec<Ident>) {
    let mut resolutions = Vec::new();
    let mut field_names = Vec::new();

    for (field_name, full_type, lookup_token_expr) in &dependencies.fields {
        let field_name_str = field_name.to_string();

        let resolution = quote! {
            let #field_name: #full_type = {
                let __lookup_token = #lookup_token_expr;
                let provider = dependencies
                    .get(&__lookup_token)
                    .unwrap_or_else(|| panic!(
                        "Missing dependency '{}' for field '{}'",
                        __lookup_token, #field_name_str
                    ));

                let any_box = provider.execute(vec![], None).await;

                *any_box.downcast::<#full_type>()
                    .unwrap_or_else(|_| panic!(
                        "Failed to downcast '{}' to {}",
                        __lookup_token,
                        stringify!(#full_type)
                    ))
            };
        };

        resolutions.push(resolution);
        field_names.push(field_name.clone());
    }

    (resolutions, field_names)
}

fn generate_manager(
    struct_name: &Ident,
    dependencies: &DependencyInfo,
    scope: ProviderScope,
) -> TokenStream {
    match scope {
        ProviderScope::Singleton => generate_singleton_manager(struct_name, dependencies),
        ProviderScope::Request => generate_request_manager(struct_name, dependencies),
        ProviderScope::Transient => generate_transient_manager(struct_name, dependencies),
    }
}

fn generate_singleton_manager(struct_name: &Ident, dependencies: &DependencyInfo) -> TokenStream {
    let manager_name = Ident::new(&format!("{}Manager", struct_name), struct_name.span());
    let provider_name = Ident::new(&format!("{}Provider", struct_name), struct_name.span());
    let struct_token = struct_name.to_string();

    let (field_resolutions, field_names) = generate_manager_field_resolutions(dependencies);

    // Generate struct instantiation code (either custom init or struct literal)
    let struct_instantiation = if let Some(init_fn) = &dependencies.init_method {
        // Custom init method: MyService::new(dep1, dep2, ...)
        let init_ident = syn::Ident::new(init_fn, struct_name.span());
        quote! {
            #struct_name::#init_ident(#(#field_names),*)
        }
    } else {
        // Standard struct literal: MyService { dep1, dep2, field3: default, ... }
        let owned_field_inits: Vec<_> = dependencies
            .owned_fields
            .iter()
            .map(|(field_name, field_type, default_expr)| {
                if let Some(expr) = default_expr {
                    // User provided #[default(...)]
                    quote! { #field_name: #expr }
                } else {
                    // Fall back to Default trait
                    quote! { #field_name: <#field_type>::default() }
                }
            })
            .collect();

        quote! {
            #struct_name {
                #(#field_names,)*
                #(#owned_field_inits),*
            }
        }
    };

    let dependency_tokens: Vec<_> = dependencies
        .fields
        .iter()
        .map(|(_, _full_type, lookup_token_expr)| lookup_token_expr)
        .collect();

    // Generate scope validation code (Singleton cannot inject Request)
    let scope_validation = if !dependencies.fields.is_empty() {
        let dep_checks: Vec<_> = dependencies
            .fields
            .iter()
            .map(|(field_name, _full_type, lookup_token_expr)| {
                let field_str = field_name.to_string();
                quote! {
                    {
                        let __lookup_token = #lookup_token_expr;
                        if let Some(provider) = dependencies.get(&__lookup_token) {
                            let dep_scope = provider.get_scope();
                            if matches!(dep_scope, ::toni::ProviderScope::Request) {
                                panic!(
                                    "\n❌ Scope validation error in provider '{}':\n\
                                     \n\
                                     Singleton-scoped providers cannot inject Request-scoped providers.\n\
                                     Field '{}' depends on '{}' which has Request scope.\n\
                                     \n\
                                     This restriction prevents data leakage across requests. Singleton providers\n\
                                     live for the entire application lifetime and would capture stale request data.\n\
                                     \n\
                                     Solutions:\n\
                                     1. Change '{}' to Request scope: #[injectable(scope = \"request\")]\n\
                                     2. Change '{}' to Singleton scope (if appropriate for your use case)\n\
                                     3. Pass request-specific data as method parameters instead of injecting\n\
                                     4. Extract data in controller (which has HttpRequest access) and pass it down\n\
                                     \n",
                                    #struct_token,
                                    #field_str,
                                    __lookup_token,
                                    #struct_token,
                                    __lookup_token
                                );
                            }
                        }
                    }
                }
            })
            .collect();

        quote! {
            // Validate scope compatibility (runtime check at startup)
            #(#dep_checks)*
        }
    } else {
        quote! {}
    };

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

                #scope_validation

                // Resolve all dependencies at startup
                #(#field_resolutions)*

                // Create the instance ONCE at startup
                let instance = ::std::sync::Arc::new({
                    #struct_instantiation
                });

                // Wrap in singleton provider
                let provider_wrapper = #provider_name { instance };

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

fn generate_request_manager(struct_name: &Ident, dependencies: &DependencyInfo) -> TokenStream {
    let manager_name = Ident::new(&format!("{}Manager", struct_name), struct_name.span());
    let provider_name = Ident::new(&format!("{}Provider", struct_name), struct_name.span());
    let struct_token = struct_name.to_string();

    let dependency_tokens: Vec<_> = dependencies
        .fields
        .iter()
        .map(|(_, _full_type, lookup_token_expr)| lookup_token_expr)
        .collect();

    // No scope validation for Request providers - they can inject anything
    // (Singleton, Request, or Transient - all are valid)

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

                // Request scope: just store dependencies, instance created per request
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

fn generate_transient_manager(struct_name: &Ident, dependencies: &DependencyInfo) -> TokenStream {
    let manager_name = Ident::new(&format!("{}Manager", struct_name), struct_name.span());
    let provider_name = Ident::new(&format!("{}Provider", struct_name), struct_name.span());
    let struct_token = struct_name.to_string();

    let dependency_tokens: Vec<_> = dependencies
        .fields
        .iter()
        .map(|(_, _full_type, lookup_token_expr)| lookup_token_expr)
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

                // Transient scope: just store dependencies, instance created every time
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
