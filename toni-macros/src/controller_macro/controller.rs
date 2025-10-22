use std::collections::HashMap;

use proc_macro2::TokenStream;
use quote::quote;
use syn::{Attribute, Error, Ident, ImplItemFn, LitStr, Result, spanned::Spanned};

use crate::{
    enhancer::enhancer::create_enchancers_token_stream,
    markers_params::{
        extracts_marker_params::{
            extract_body_from_param, extract_path_param_from_param, extract_query_from_param,
        },
        get_marker_params::MarkerParam,
    },
    shared::{dependency_info::DependencyInfo, metadata_info::MetadataInfo},
    utils::{
        controller_utils::{attr_to_string, create_extract_body_dto_token_stream},
        create_struct_name::{create_provider_name_by_fn_and_struct_ident, create_struct_name},
        modify_impl_function_body::modify_method_body,
        modify_return_body::modify_return_method_body,
        types::create_type_reference,
    },
};

pub fn generate_controller_and_metadata(
    implementation_fn: &ImplItemFn,
    original_struct_name: &Ident,
    dependency_info: &mut DependencyInfo,
    trait_reference_name: &Ident,
    route_prefix: &String,
    http_method_attr: &Attribute,
    enhancers_attr: HashMap<&Ident, &Attribute>,
    marker_params: Vec<MarkerParam>,
) -> Result<(TokenStream, MetadataInfo)> {
    let http_method = attr_to_string(http_method_attr)
        .map_err(|_| Error::new(http_method_attr.span(), "Invalid attribute format"))?;

    let route_path = http_method_attr
        .parse_args::<LitStr>()
        .map_err(|_| Error::new(http_method_attr.span(), "Invalid attribute format"))?
        .value();

    let enhancers = create_enchancers_token_stream(enhancers_attr)?;

    let function_name = &implementation_fn.sig.ident;
    let controller_name = create_struct_name("Controller", function_name)?;
    let controller_token = controller_name.to_string();

    let mut modified_block = implementation_fn.block.clone();
    modify_return_method_body(&mut modified_block);
    let injections = modify_method_body(
        &mut modified_block,
        dependency_info.fields.clone(),
        original_struct_name.clone(),
    );

    let mut dependencies = Vec::with_capacity(injections.len());
    let mut field_definitions = Vec::with_capacity(injections.len());
    for injection in injections {
        let (provider_manager, function_name, field_name) = injection;

        let provider_name =
            create_provider_name_by_fn_and_struct_ident(&function_name, &provider_manager)?;

        if !dependency_info.unique_types.contains(&provider_name) {
            dependency_info.unique_types.insert(provider_name.clone());
        }
        if !dependencies.contains(&(field_name.clone(), provider_name.clone())) {
            dependencies.push((field_name.clone(), provider_name));
            let provider_trait = create_type_reference("ProviderTrait", true, true, true);
            field_definitions.push(quote! { #field_name: #provider_trait });
        }
    }

    let full_route_path = format!("{}{}", route_prefix, route_path);
    let mut marker_params_token_stream = Vec::new();
    let mut body_dto_token_stream = None;
    if !marker_params.is_empty() {
        marker_params_token_stream = marker_params
            .iter()
            .map(|marker_param| {
                if marker_param.marker_name == "body" {
                    let dto_type_ident = marker_param.type_ident.clone();
                    body_dto_token_stream =
                        Some(create_extract_body_dto_token_stream(&dto_type_ident)?);
                    return extract_body_from_param(marker_param);
                }
                if marker_param.marker_name == "query" {
                    return extract_query_from_param(marker_param);
                }
                if marker_param.marker_name == "param" {
                    return extract_path_param_from_param(marker_param);
                }
                Ok(quote! {})
            })
            .collect::<Result<Vec<_>>>()?;
    }

    let generated_code = generate_controller_code(
        &controller_name,
        &field_definitions,
        &modified_block,
        &controller_token,
        full_route_path,
        &http_method,
        trait_reference_name,
        enhancers,
        marker_params_token_stream,
        body_dto_token_stream,
    );
    Ok((
        generated_code,
        MetadataInfo {
            struct_name: controller_name,
            dependencies,
        },
    ))
}

fn generate_controller_code(
    controller_name: &Ident,
    field_defs: &[TokenStream],
    method_body: &syn::Block,
    controller_token: &str,
    full_route_path: String,
    http_method: &str,
    trait_name: &Ident,
    enhancers: HashMap<String, Vec<TokenStream>>,
    marker_params_token_stream: Vec<TokenStream>,
    body_dto_token_stream: Option<TokenStream>,
) -> TokenStream {
    let binding = Vec::new();
    let use_guards = enhancers.get("use_guard").unwrap_or(&binding);
    let interceptors = enhancers.get("interceptor").unwrap_or(&binding);
    let pipes = enhancers.get("pipe").unwrap_or(&binding);
    let body_dto_stream = if let Some(token_stream) = body_dto_token_stream {
        token_stream
    } else {
        quote! { None }
    };
    quote! {
        struct #controller_name {
            #(#field_defs),*
        }
        #[::async_trait::async_trait]
        impl ::toni::traits_helpers::#trait_name for #controller_name {
            #[inline]
            async fn execute(
                &self,
                req: ::toni::http_helpers::HttpRequest
            ) -> Box<dyn ::toni::http_helpers::IntoResponse<Response = ::toni::http_helpers::HttpResponse> + Send> {
                #(#marker_params_token_stream)*
                #method_body
            }

            #[inline]
            fn get_token(&self) -> String {
                #controller_token.to_string()
            }

            #[inline]
            fn get_path(&self) -> String {
                #full_route_path.to_string()
            }

            #[inline]
            fn get_method(&self) -> ::toni::http_helpers::HttpMethod {
                ::toni::http_helpers::HttpMethod::from_string(#http_method).unwrap()
            }

            #[inline]
            fn get_guards(&self) -> Vec<::std::sync::Arc<dyn ::toni::traits_helpers::Guard>> {
                vec![#(#use_guards),*]
            }

            #[inline]
            fn get_pipes(&self) -> Vec<::std::sync::Arc<dyn ::toni::traits_helpers::Pipe>> {
                vec![#(#pipes),*]
            }

            #[inline]
            fn get_interceptors(&self) -> Vec<::std::sync::Arc<dyn ::toni::traits_helpers::Interceptor>> {
                vec![#(#interceptors),*]
            }

            #[inline]
            fn get_body_dto(&self, req: &::toni::http_helpers::HttpRequest) -> Option<Box<dyn ::toni::traits_helpers::validate::Validatable>> {
                #body_dto_stream
            }
        }
    }
}
