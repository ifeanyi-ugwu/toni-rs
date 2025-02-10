use proc_macro2::TokenStream;
use quote::quote;
use syn::{Attribute, Error, Ident, ImplItemFn, LitStr, spanned::Spanned};

use crate::{
    shared::{dependency_info::DependencyInfo, metadata_info::MetadataInfo},
    utils::{
        controller_utils::attr_to_string,
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
    attribute: &Attribute,
) -> Result<(TokenStream, MetadataInfo), Error> {
    let http_method = attr_to_string(attribute)
        .map_err(|_| Error::new(attribute.span(), "Invalid attribute format"))?;

    let route_path = attribute
        .parse_args::<LitStr>()
        .map_err(|_| Error::new(attribute.span(), "Invalid attribute format"))?
        .value();

    let function_name = &implementation_fn.sig.ident;
    let controller_name = create_struct_name("Controller", function_name);
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
            create_provider_name_by_fn_and_struct_ident(&function_name, &provider_manager);

        if !dependency_info.unique_types.insert(provider_name.clone()) {
            return Err(Error::new(
                attribute.span(),
                format!("Conflict in dependency: {}", provider_name),
            ));
        }

        dependencies.push((field_name.clone(), provider_name));

        let provider_trait = create_type_reference("ProviderTrait", true, true, true);
        field_definitions.push(quote! { #field_name: #provider_trait });
    }

    let generated_code = generate_controller_code(
        &controller_name,
        &field_definitions,
        &modified_block,
        &controller_token,
        &route_prefix,
        &route_path,
        &http_method,
        &trait_reference_name,
    );
    Ok((generated_code, MetadataInfo {
        struct_name: controller_name,
        dependencies,
    }))
}

fn generate_controller_code(
    controller_name: &Ident,
    field_defs: &[TokenStream],
    method_body: &syn::Block,
    controller_token: &str,
    route_prefix: &str,
    route_path: &str,
    http_method: &str,
    trait_name: &Ident,
) -> TokenStream {
    let full_route_path = format!("{}{}", route_prefix, route_path);

    quote! {
        struct #controller_name {
            #(#field_defs),*
        }

        impl ::tonirs_core::traits_helpers::#trait_name for #controller_name {
            #[inline]
            fn execute(
                &self,
                req: ::tonirs_core::http_helpers::HttpRequest
            ) -> Box<dyn ::tonirs_core::http_helpers::IntoResponse<Response = ::tonirs_core::http_helpers::HttpResponse>> {
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
            fn get_method(&self) -> ::tonirs_core::http_helpers::HttpMethod {
                ::tonirs_core::http_helpers::HttpMethod::from_str(#http_method).unwrap()
            }
        }
    }
}
