//! Controller Instance Injection Implementation
//!
//! Architecture:
//! 1. User struct with REAL fields (unchanged)
//! 2. Controller wrapper per handler method that lazily creates instances with dependency resolution
//! 3. Manager that returns controller wrappers
//!
//! Example transformation:
//! ```rust,ignore
//! // User code:
//! #[controller_struct(pub struct AppController { service: AppService })]
//! #[controller("/api")]
//! impl AppController {
//!     #[get("/info")]
//!     fn get_info(&self, req: HttpRequest) -> ToniBody {
//!         self.service.get_app_info()
//!     }
//! }
//!
//! // Generated:
//! #[derive(Clone)]
//! pub struct AppController {
//!     service: AppService
//! }
//!
//! impl AppController {
//!     fn get_info(&self, req: HttpRequest) -> ToniBody {
//!         self.service.get_app_info()
//!     }
//! }
//!
//! struct GetInfoController {
//!     dependencies: FxHashMap<String, Arc<Box<dyn ProviderTrait>>>
//! }
//!
//! impl ControllerTrait for GetInfoController {
//!     async fn handle(&self, req: HttpRequest) -> HttpResponse {
//!         // Resolve dependencies
//!         let service: AppService = *self.dependencies.get("AppService")
//!             .unwrap().execute(vec![]).await.downcast().unwrap();
//!
//!         // Create controller instance with real fields
//!         let controller = AppController { service };
//!
//!         // Call user's method on real struct
//!         controller.get_info(req)
//!     }
//! }
//! ```

use proc_macro2::TokenStream;
use quote::quote;
use std::collections::HashMap;
use syn::{
    Attribute, Error, Ident, ImplItemFn, ItemImpl, ItemStruct, LitStr, Result, spanned::Spanned,
};

use crate::{
    enhancer::enhancer::create_enchancers_token_stream,
    markers_params::{
        extracts_marker_params::{
            extract_body_from_param, extract_path_param_from_param, extract_query_from_param,
        },
        get_marker_params::MarkerParam,
    },
    shared::{dependency_info::DependencyInfo, metadata_info::MetadataInfo},
    utils::controller_utils::{attr_to_string, create_extract_body_dto_token_stream},
};

pub fn generate_instance_controller_system(
    struct_attrs: &ItemStruct,
    impl_block: &ItemImpl,
    dependencies: &DependencyInfo,
    route_prefix: &str,
    scope: crate::shared::scope_parser::ControllerScope,
    was_explicit: bool,
) -> Result<TokenStream> {
    let struct_name = &struct_attrs.ident;

    // Add Clone derive to struct (required for creating instances)
    let struct_with_clone = add_clone_derive(struct_attrs);
    let impl_def = impl_block.clone();

    // Generate controller wrappers
    let (singleton_wrappers, singleton_metadata) =
        generate_controller_wrappers(impl_block, struct_name, dependencies, route_prefix, crate::shared::scope_parser::ControllerScope::Singleton)?;

    // OPTIMIZATION: Only generate Request wrappers if controller has dependencies
    // No dependencies = no possibility of elevation, so Request wrappers would be wasted
    let (request_wrappers, request_metadata) = if dependencies.fields.is_empty() {
        // No dependencies - elevation is impossible, don't generate Request wrappers
        (vec![], vec![])
    } else {
        // Has dependencies - generate Request wrappers for potential elevation
        generate_controller_wrappers(impl_block, struct_name, dependencies, route_prefix, crate::shared::scope_parser::ControllerScope::Request)?
    };

    let manager = generate_manager(
        struct_name,
        singleton_metadata,
        request_metadata,
        dependencies,
        scope,
        was_explicit,
    );

    Ok(quote! {
        #[allow(dead_code)]
        #struct_with_clone

        #[allow(dead_code)]
        #impl_def

        // Generate Singleton wrappers (always)
        #(#singleton_wrappers)*

        // Generate Request wrappers (only if controller has dependencies)
        #(#request_wrappers)*

        #manager
    })
}

fn add_clone_derive(struct_attrs: &ItemStruct) -> ItemStruct {
    let mut struct_def = struct_attrs.clone();

    let has_clone = struct_def.attrs.iter().any(|attr| {
        if attr.path().is_ident("derive") {
            // Would need to parse derive contents properly
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

fn generate_controller_wrappers(
    impl_block: &ItemImpl,
    struct_name: &Ident,
    dependencies: &DependencyInfo,
    route_prefix: &str,
    scope: crate::shared::scope_parser::ControllerScope,
) -> Result<(Vec<TokenStream>, Vec<MetadataInfo>)> {
    let mut wrappers = Vec::new();
    let mut metadata_list = Vec::new();

    for item in &impl_block.items {
        if let syn::ImplItem::Fn(method) = item {
            if let Some(http_method_attr) = find_http_method_attr(&method.attrs) {
                let enhancers_attr = get_enhancers_attr(&method.attrs)?;
                let marker_params = get_marker_params(method)?;

                let (wrapper, metadata) = generate_controller_wrapper(
                    method,
                    struct_name,
                    dependencies,
                    route_prefix,
                    http_method_attr,
                    enhancers_attr,
                    marker_params,
                    scope,
                )?;

                wrappers.push(wrapper);
                metadata_list.push(metadata);
            }
        }
    }

    Ok((wrappers, metadata_list))
}

fn find_http_method_attr(attrs: &[Attribute]) -> Option<&Attribute> {
    attrs.iter().find(|attr| {
        attr.path().is_ident("get")
            || attr.path().is_ident("post")
            || attr.path().is_ident("put")
            || attr.path().is_ident("delete")
            || attr.path().is_ident("patch")
            || attr.path().is_ident("head")
            || attr.path().is_ident("options")
    })
}

fn get_enhancers_attr(attrs: &[syn::Attribute]) -> Result<HashMap<&Ident, &Attribute>> {
    use crate::enhancer::enhancer::get_enhancers_attr as get_enhancers;
    get_enhancers(attrs)
}

fn get_marker_params(method: &ImplItemFn) -> Result<Vec<MarkerParam>> {
    use crate::markers_params::get_marker_params::get_marker_params as get_params;
    get_params(method)
}

fn generate_controller_wrapper(
    method: &ImplItemFn,
    struct_name: &Ident,
    dependencies: &DependencyInfo,
    route_prefix: &str,
    http_method_attr: &Attribute,
    enhancers_attr: HashMap<&Ident, &Attribute>,
    marker_params: Vec<MarkerParam>,
    scope: crate::shared::scope_parser::ControllerScope,
) -> Result<(TokenStream, MetadataInfo)> {
    let http_method = attr_to_string(http_method_attr)
        .map_err(|_| Error::new(http_method_attr.span(), "Invalid attribute format"))?;

    let route_path = http_method_attr
        .parse_args::<LitStr>()
        .map_err(|_| Error::new(http_method_attr.span(), "Invalid attribute format"))?
        .value();

    let full_route_path = format!("{}{}", route_prefix, route_path);

    let method_name = &method.sig.ident;
    // Include struct name to avoid collisions between controllers with same method names
    // Also include scope suffix to allow both Singleton and Request wrappers
    let scope_suffix = match scope {
        crate::shared::scope_parser::ControllerScope::Singleton => "",
        crate::shared::scope_parser::ControllerScope::Request => "Request",
    };
    let controller_name = Ident::new(
        &format!(
            "{}{}Controller{}",
            struct_name,
            capitalize_first(method_name.to_string()),
            scope_suffix
        ),
        method_name.span(),
    );
    let controller_token = controller_name.to_string();

    let (field_resolutions, field_names) = generate_field_resolutions(dependencies);

    let struct_instantiation = quote! {
        let controller = #struct_name {
            #(#field_names),*
        };
    };

    let method_call = generate_method_call(method, &marker_params)?;
    let enhancers = create_enchancers_token_stream(enhancers_attr)?;

    let (marker_params_extraction, body_dto_token_stream) =
        generate_marker_params_extraction(&marker_params)?;

    let wrapper = generate_controller_wrapper_code(
        &controller_name,
        &controller_token,
        &full_route_path,
        &http_method,
        &field_resolutions,
        &struct_instantiation,
        &method_call,
        &enhancers,
        &marker_params_extraction,
        &body_dto_token_stream,
        scope,
        struct_name, // Pass struct_name for downcast in singleton wrapper
    );

    let controller_dependencies: Vec<(Ident, String)> = dependencies
        .fields
        .iter()
        .map(|(field_name, _full_type, lookup_token)| {
            let dep_field_name = Ident::new(&format!("{}_dep", field_name), field_name.span());
            (dep_field_name, lookup_token.clone())
        })
        .collect();

    Ok((
        wrapper,
        MetadataInfo {
            struct_name: controller_name,
            dependencies: controller_dependencies,
        },
    ))
}

fn generate_field_resolutions(dependencies: &DependencyInfo) -> (Vec<TokenStream>, Vec<Ident>) {
    use std::collections::HashMap;

    let mut resolutions = Vec::new();
    let mut field_names = Vec::new();

    // Group fields by their lookup token (type)
    let mut type_to_fields: HashMap<String, Vec<(Ident, syn::Type)>> = HashMap::new();
    for (field_name, full_type, lookup_token) in &dependencies.fields {
        type_to_fields
            .entry(lookup_token.clone())
            .or_insert_with(Vec::new)
            .push((field_name.clone(), full_type.clone()));
    }

    // For each unique type, check scope and generate appropriate resolution
    for (lookup_token, fields) in type_to_fields.iter() {
        // Use the first field's type (all fields of same token have same type)
        let (_, full_type) = &fields[0];
        let error_msg = format!("Missing dependency '{}'", lookup_token);

        if fields.len() == 1 {
            // Only one field - no need for runtime check, just resolve directly
            let field_name = &fields[0].0;
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
        } else {
            // Multiple fields of same type - need runtime scope check
            // If Transient: resolve separately for each field (no deduplication)
            // If Request/Singleton: resolve once and clone for multiple fields (deduplication)
            let scope_check_var_name = format!(
                "is_transient_{}",
                lookup_token.to_lowercase().replace("::", "_")
            );
            let scope_check_var = Ident::new(&scope_check_var_name, fields[0].0.span());

            // Generate scope check
            let scope_check = quote! {
                let #scope_check_var = {
                    let provider = self.dependencies
                        .get(#lookup_token)
                        .expect(#error_msg);
                    provider.get_scope() == ::toni::ProviderScope::Transient
                };
            };
            resolutions.push(scope_check);

            let temp_var_name = format!(
                "resolved_{}",
                lookup_token.to_lowercase().replace("::", "_")
            );
            let temp_var = Ident::new(&temp_var_name, fields[0].0.span());

            // Declare all field variables outside the if/else for proper scoping
            let field_declarations: Vec<_> = fields
                .iter()
                .map(|(field_name, _)| {
                    quote! {
                        let #field_name: #full_type;
                    }
                })
                .collect();

            // Generate runtime branch based on scope
            let transient_assignments: Vec<_> = fields
                .iter()
                .map(|(field_name, _)| {
                    quote! {
                        #field_name = {
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
                    }
                })
                .collect();

            let request_resolution = quote! {
                let #temp_var: #full_type = {
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

            let request_assignments: Vec<_> = fields
                .iter()
                .map(|(field_name, _)| {
                    quote! {
                        #field_name = #temp_var.clone();
                    }
                })
                .collect();

            // Declare fields first, then assign based on scope
            resolutions.extend(field_declarations);

            // Generate if/else based on scope
            let scope_branch = quote! {
                if #scope_check_var {
                    // Transient: resolve each field separately
                    #(#transient_assignments)*
                } else {
                    // Request/Singleton: resolve once and clone
                    #request_resolution
                    #(#request_assignments)*
                }
            };

            resolutions.push(scope_branch);

            // Add field names
            for (field_name, _) in fields {
                field_names.push(field_name.clone());
            }
        }
    }

    (resolutions, field_names)
}

fn generate_method_call(method: &ImplItemFn, marker_params: &[MarkerParam]) -> Result<TokenStream> {
    let method_name = &method.sig.ident;

    let mut call_args = vec![quote! { req }];

    for marker_param in marker_params {
        let param_name = &marker_param.param_name;
        call_args.push(quote! { #param_name });
    }

    Ok(quote! {
        controller.#method_name(#(#call_args),*)
    })
}

fn generate_marker_params_extraction(
    marker_params: &[MarkerParam],
) -> Result<(Vec<TokenStream>, Option<TokenStream>)> {
    let mut extractions = Vec::new();
    let mut body_dto_token_stream = None;

    for marker_param in marker_params {
        match marker_param.marker_name.as_str() {
            "body" => {
                let dto_type_ident = &marker_param.type_ident;
                body_dto_token_stream = Some(create_extract_body_dto_token_stream(dto_type_ident)?);
                extractions.push(extract_body_from_param(marker_param)?);
            }
            "query" => {
                extractions.push(extract_query_from_param(marker_param)?);
            }
            "param" => {
                extractions.push(extract_path_param_from_param(marker_param)?);
            }
            _ => {}
        }
    }

    Ok((extractions, body_dto_token_stream))
}

fn generate_controller_wrapper_code(
    controller_name: &Ident,
    controller_token: &str,
    full_route_path: &str,
    http_method: &str,
    field_resolutions: &[TokenStream],
    struct_instantiation: &TokenStream,
    method_call: &TokenStream,
    enhancers: &HashMap<String, Vec<TokenStream>>,
    marker_params_extraction: &[TokenStream],
    body_dto_token_stream: &Option<TokenStream>,
    scope: crate::shared::scope_parser::ControllerScope,
    struct_name: &Ident,
) -> TokenStream {
    use crate::shared::scope_parser::ControllerScope;

    match scope {
        ControllerScope::Singleton => generate_singleton_controller_wrapper(
            controller_name,
            controller_token,
            full_route_path,
            http_method,
            method_call,
            enhancers,
            marker_params_extraction,
            body_dto_token_stream,
            struct_name, // Pass struct name for downcast
        ),
        ControllerScope::Request => generate_request_controller_wrapper(
            controller_name,
            controller_token,
            full_route_path,
            http_method,
            field_resolutions,
            struct_instantiation,
            method_call,
            enhancers,
            marker_params_extraction,
            body_dto_token_stream,
        ),
    }
}

// Singleton controller (stores Arc<ControllerInstance> created at startup)
fn generate_singleton_controller_wrapper(
    controller_name: &Ident,
    controller_token: &str,
    full_route_path: &str,
    http_method: &str,
    method_call: &TokenStream,
    enhancers: &HashMap<String, Vec<TokenStream>>,
    marker_params_extraction: &[TokenStream],
    body_dto_token_stream: &Option<TokenStream>,
    struct_name: &Ident, // Need this for downcast type
) -> TokenStream {
    let binding = Vec::new();
    let use_guards = enhancers.get("use_guard").unwrap_or(&binding);
    let interceptors = enhancers.get("interceptor").unwrap_or(&binding);
    let pipes = enhancers.get("pipe").unwrap_or(&binding);

    let body_dto_stream = if let Some(token_stream) = body_dto_token_stream {
        token_stream.clone()
    } else {
        quote! { None }
    };

    quote! {
        struct #controller_name {
            // Singleton: Store the pre-created controller instance!
            instance: ::std::sync::Arc<dyn ::std::any::Any + Send + Sync>,
        }

        #[::toni::async_trait]
        impl ::toni::traits_helpers::ControllerTrait for #controller_name {
            async fn execute(
                &self,
                req: ::toni::http_helpers::HttpRequest,
            ) -> Box<dyn ::toni::http_helpers::IntoResponse<Response = ::toni::http_helpers::HttpResponse> + Send> {
                // NO dependency resolution here!
                // NO controller instantiation here!
                // Just extract parameters and call the handler on the pre-existing instance

                #(#marker_params_extraction)*

                // Downcast the Arc<dyn Any> to the actual controller type
                let controller = self.instance
                    .downcast_ref::<#struct_name>()
                    .expect("Failed to downcast controller instance");

                let result = #method_call;
                Box::new(result)
            }

            fn get_method(&self) -> ::toni::http_helpers::HttpMethod {
                ::toni::http_helpers::HttpMethod::from_string(#http_method).unwrap()
            }

            fn get_path(&self) -> String {
                #full_route_path.to_string()
            }

            fn get_token(&self) -> String {
                #controller_token.to_string()
            }

            fn get_guards(&self) -> Vec<::std::sync::Arc<dyn ::toni::traits_helpers::Guard>> {
                vec![#(#use_guards),*]
            }

            fn get_interceptors(&self) -> Vec<::std::sync::Arc<dyn ::toni::traits_helpers::Interceptor>> {
                vec![#(#interceptors),*]
            }

            fn get_pipes(&self) -> Vec<::std::sync::Arc<dyn ::toni::traits_helpers::Pipe>> {
                vec![#(#pipes),*]
            }

            fn get_body_dto(&self, _req: &::toni::http_helpers::HttpRequest) -> Option<Box<dyn ::toni::traits_helpers::validate::Validatable>> {
                #body_dto_stream
            }
        }
    }
}

// Request-scoped controller (creates instance per request)
fn generate_request_controller_wrapper(
    controller_name: &Ident,
    controller_token: &str,
    full_route_path: &str,
    http_method: &str,
    field_resolutions: &[TokenStream],
    struct_instantiation: &TokenStream,
    method_call: &TokenStream,
    enhancers: &HashMap<String, Vec<TokenStream>>,
    marker_params_extraction: &[TokenStream],
    body_dto_token_stream: &Option<TokenStream>,
) -> TokenStream {
    let binding = Vec::new();
    let use_guards = enhancers.get("use_guard").unwrap_or(&binding);
    let interceptors = enhancers.get("interceptor").unwrap_or(&binding);
    let pipes = enhancers.get("pipe").unwrap_or(&binding);

    let body_dto_stream = if let Some(token_stream) = body_dto_token_stream {
        token_stream.clone()
    } else {
        quote! { None }
    };

    quote! {
        struct #controller_name {
            dependencies: ::toni::FxHashMap<
                String,
                ::std::sync::Arc<Box<dyn ::toni::traits_helpers::ProviderTrait>>
            >,
        }

        #[::toni::async_trait]
        impl ::toni::traits_helpers::ControllerTrait for #controller_name {
            async fn execute(
                &self,
                req: ::toni::http_helpers::HttpRequest,
            ) -> Box<dyn ::toni::http_helpers::IntoResponse<Response = ::toni::http_helpers::HttpResponse> + Send> {
                #(#field_resolutions)*
                #(#marker_params_extraction)*
                #struct_instantiation

                let result = #method_call;
                Box::new(result)
            }

            fn get_method(&self) -> ::toni::http_helpers::HttpMethod {
                ::toni::http_helpers::HttpMethod::from_string(#http_method).unwrap()
            }

            fn get_path(&self) -> String {
                #full_route_path.to_string()
            }

            fn get_token(&self) -> String {
                #controller_token.to_string()
            }

            fn get_guards(&self) -> Vec<::std::sync::Arc<dyn ::toni::traits_helpers::Guard>> {
                vec![#(#use_guards),*]
            }

            fn get_interceptors(&self) -> Vec<::std::sync::Arc<dyn ::toni::traits_helpers::Interceptor>> {
                vec![#(#interceptors),*]
            }

            fn get_pipes(&self) -> Vec<::std::sync::Arc<dyn ::toni::traits_helpers::Pipe>> {
                vec![#(#pipes),*]
            }

            fn get_body_dto(&self, _req: &::toni::http_helpers::HttpRequest) -> Option<Box<dyn ::toni::traits_helpers::validate::Validatable>> {
                #body_dto_stream
            }
        }
    }
}

fn generate_manager(
    struct_name: &Ident,
    singleton_metadata: Vec<MetadataInfo>,
    request_metadata: Vec<MetadataInfo>,
    dependencies: &DependencyInfo,
    scope: crate::shared::scope_parser::ControllerScope,
    was_explicit: bool,
) -> TokenStream {
    use crate::shared::scope_parser::ControllerScope;

    match scope {
        ControllerScope::Singleton => {
            generate_singleton_manager(
                struct_name,
                singleton_metadata,
                request_metadata,
                dependencies,
                was_explicit,
            )
        }
        ControllerScope::Request => {
            // Request-scoped controllers don't need elevation logic
            generate_request_manager(struct_name, request_metadata, dependencies)
        }
    }
}

// Singleton manager - creates controller instance AT STARTUP
// OR elevates to Request scope if dependencies require it
fn generate_singleton_manager(
    struct_name: &Ident,
    singleton_metadata: Vec<MetadataInfo>,
    request_metadata: Vec<MetadataInfo>,
    dependencies: &DependencyInfo,
    was_explicit: bool,
) -> TokenStream {
    let manager_name = Ident::new(&format!("{}Manager", struct_name), struct_name.span());
    let struct_token = struct_name.to_string();

    let dependency_tokens: Vec<String> = dependencies
        .fields
        .iter()
        .map(|(_, _full_type, lookup_token)| lookup_token.clone())
        .collect();

    let unique_tokens: Vec<_> = dependency_tokens
        .iter()
        .map(|token| quote! { #token.to_string() })
        .collect();

    // Generate field resolutions AT STARTUP
    let field_resolutions = dependencies
        .fields
        .iter()
        .map(|(field_name, full_type, lookup_token)| {
            let error_msg = format!("Missing dependency '{}'", lookup_token);
            quote! {
                let #field_name: #full_type = {
                    let provider = dependencies
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
            }
        })
        .collect::<Vec<_>>();

    let field_names: Vec<_> = dependencies
        .fields
        .iter()
        .map(|(field_name, _, _)| field_name.clone())
        .collect();

    // Create controller wrappers with the shared Arc'd instance (for Singleton mode)
    let controller_wrapper_creations: Vec<_> = singleton_metadata
        .iter()
        .map(|metadata| {
            let controller_name = &metadata.struct_name;
            let controller_token = controller_name.to_string();

            quote! {
                controllers.insert(
                    #controller_token.to_string(),
                    ::std::sync::Arc::new(
                        Box::new(#controller_name {
                            instance: controller_instance.clone(),
                        }) as Box<dyn ::toni::traits_helpers::ControllerTrait>
                    )
                );
            }
        })
        .collect();

    // Generate scope checking code to determine if we need to elevate to Request scope
    let scope_check_code = if dependencies.fields.is_empty() {
        // No dependencies - definitely Singleton
        quote! { let needs_elevation = false; }
    } else {
        let dep_checks: Vec<_> = dependencies
            .fields
            .iter()
            .map(|(_, _, lookup_token)| {
                quote! {
                    if let Some(provider) = dependencies.get(#lookup_token) {
                        if matches!(provider.get_scope(), ::toni::ProviderScope::Request) {
                            request_deps.push(#lookup_token);
                        }
                    }
                }
            })
            .collect();

        quote! {
            // Check if any dependency is Request-scoped
            let mut request_deps: Vec<&str> = Vec::new();
            #(#dep_checks)*
            let needs_elevation = !request_deps.is_empty();
        }
    };

    // Generate warning messages based on strategy
    let warning_code = if was_explicit {
        // Case 3: User explicitly set singleton, but we need to elevate
        quote! {
            if needs_elevation {
                eprintln!("⚠️  WARNING: Controller '{}' explicitly declared as 'singleton'", #struct_token);
                eprintln!("    but depends on Request-scoped provider(s): {:?}", request_deps);
                eprintln!("    The controller will be Request-scoped. Change to:");
                eprintln!("    #[controller_struct(scope = \"request\", pub struct {} {{ ... }})]", #struct_token);
            }
        }
    } else {
        // Case 1: Default scope (implicit singleton), elevating to request
        quote! {
            if needs_elevation {
                eprintln!("⚠️  INFO: Controller '{}' automatically elevated to Request scope", #struct_token);
                eprintln!("    due to Request-scoped provider(s): {:?}", request_deps);
                eprintln!("    To silence this message, explicitly set:");
                eprintln!("    #[controller_struct(scope = \"request\", pub struct {} {{ ... }})]", #struct_token);
            }
        }
    };

    // Generate controller instances for Request-scoped (used if elevation happens)
    let request_controller_instances: Vec<_> = request_metadata
        .iter()
        .map(|metadata| {
            let controller_name = &metadata.struct_name;
            let controller_token = controller_name.to_string();

            quote! {
                (
                    #controller_token.to_string(),
                    ::std::sync::Arc::new(
                        Box::new(#controller_name {
                            dependencies: dependencies.clone(),
                        }) as Box<dyn ::toni::traits_helpers::ControllerTrait>
                    )
                )
            }
        })
        .collect();

    quote! {
        pub struct #manager_name;

        #[::toni::async_trait]
        impl ::toni::traits_helpers::Controller for #manager_name {
            async fn get_all_controllers(
                &self,
                dependencies: &::toni::FxHashMap<
                    String,
                    ::std::sync::Arc<Box<dyn ::toni::traits_helpers::ProviderTrait>>
                >,
            ) -> ::toni::FxHashMap<
                String,
                ::std::sync::Arc<Box<dyn ::toni::traits_helpers::ControllerTrait>>
            > {
                let mut controllers = ::toni::FxHashMap::default();

                // CHECK IF ELEVATION TO REQUEST SCOPE IS NEEDED
                #scope_check_code

                // EMIT WARNINGS BASED ON STRATEGY
                #warning_code

                // BRANCH: Use Request-scoped logic if elevation needed, otherwise Singleton
                if needs_elevation {
                    // ELEVATED TO REQUEST SCOPE - use Request-scoped wrappers
                    #(
                        let (key, value): (String, ::std::sync::Arc<Box<dyn ::toni::traits_helpers::ControllerTrait>>) = #request_controller_instances;
                        controllers.insert(key, value);
                    )*
                } else {
                    // TRUE SINGLETON - create instance once at startup
                    // RESOLVE DEPENDENCIES AT STARTUP
                    #(#field_resolutions)*

                    // CREATE CONTROLLER INSTANCE AT STARTUP
                    let controller_instance: ::std::sync::Arc<dyn ::std::any::Any + Send + Sync> = ::std::sync::Arc::new(#struct_name {
                        #(#field_names),*
                    });

                    // CREATE ALL HANDLER WRAPPERS THAT SHARE THE SAME ARC
                    #(#controller_wrapper_creations)*
                }

                controllers
            }

            fn get_name(&self) -> String {
                #struct_token.to_string()
            }

            fn get_token(&self) -> String {
                #struct_token.to_string()
            }

            fn get_dependencies(&self) -> Vec<String> {
                vec![#(#unique_tokens),*]
            }
        }
    }
}

// Request manager - stores dependencies, creates instance per request
fn generate_request_manager(
    struct_name: &Ident,
    metadata_list: Vec<MetadataInfo>,
    dependencies: &DependencyInfo,
) -> TokenStream {
    let manager_name = Ident::new(&format!("{}Manager", struct_name), struct_name.span());
    let struct_token = struct_name.to_string();

    let dependency_tokens: Vec<String> = dependencies
        .fields
        .iter()
        .map(|(_, _full_type, lookup_token)| lookup_token.clone())
        .collect();

    let unique_tokens: Vec<_> = dependency_tokens
        .iter()
        .map(|token| quote! { #token.to_string() })
        .collect();

    let controller_instances: Vec<_> = metadata_list
        .iter()
        .map(|metadata| {
            let controller_name = &metadata.struct_name;
            let controller_token = controller_name.to_string();

            quote! {
                (
                    #controller_token.to_string(),
                    ::std::sync::Arc::new(
                        Box::new(#controller_name {
                            dependencies: dependencies.clone(),
                        }) as Box<dyn ::toni::traits_helpers::ControllerTrait>
                    )
                )
            }
        })
        .collect();

    quote! {
        pub struct #manager_name;

        #[::toni::async_trait]
        impl ::toni::traits_helpers::Controller for #manager_name {
            async fn get_all_controllers(
                &self,
                dependencies: &::toni::FxHashMap<
                    String,
                    ::std::sync::Arc<Box<dyn ::toni::traits_helpers::ProviderTrait>>
                >,
            ) -> ::toni::FxHashMap<
                String,
                ::std::sync::Arc<Box<dyn ::toni::traits_helpers::ControllerTrait>>
            > {
                let mut controllers = ::toni::FxHashMap::default();

                #(
                    let (key, value): (String, ::std::sync::Arc<Box<dyn ::toni::traits_helpers::ControllerTrait>>) = #controller_instances;
                    controllers.insert(key, value);
                )*

                controllers
            }

            fn get_name(&self) -> String {
                #struct_token.to_string()
            }

            fn get_token(&self) -> String {
                #struct_token.to_string()
            }

            fn get_dependencies(&self) -> Vec<String> {
                vec![#(#unique_tokens),*]
            }
        }
    }
}

fn capitalize_first(s: String) -> String {
    s.split('_')
        .map(|word| {
            let mut chars = word.chars();
            match chars.next() {
                None => String::new(),
                Some(first) => first.to_uppercase().collect::<String>() + chars.as_str(),
            }
        })
        .collect()
}
