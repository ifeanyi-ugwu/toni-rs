use proc_macro2::TokenStream;
use quote::quote;
use syn::{Error, Ident, ImplItemFn};

use crate::{
    shared::{dependency_info::DependencyInfo, metadata_info::MetadataInfo},
    utils::{
        create_struct_name::{create_provider_name_by_fn_and_struct_ident, create_struct_name},
        extracts::extract_params_from_impl_fn,
        modify_impl_function_body::modify_method_body,
        modify_return_body::modify_return_method_body,
        types::create_type_reference,
    },
};

pub fn generate_provider_and_metadata(
    implementation_fn: &ImplItemFn,
    original_struct_ident: &Ident,
    dependency_info: &mut DependencyInfo,
    trait_reference_name: &Ident,
) -> Result<(TokenStream, MetadataInfo), Error> {
    let impl_fn_params: Vec<(Ident, syn::Type)> = extract_params_from_impl_fn(implementation_fn);

    let original_struct_name = original_struct_ident.to_string();

    let function_name = &implementation_fn.sig.ident;
    let provider_name = create_struct_name(&original_struct_name, function_name)?;
    let provider_token = provider_name.to_string();

    let mut modified_block = implementation_fn.block.clone();
    modify_return_method_body(&mut modified_block);
    let injections = modify_method_body(
        &mut modified_block,
        dependency_info.fields.clone(),
        original_struct_ident.clone(),
    );

    let mut dependencies = Vec::with_capacity(injections.len());
    let mut field_definitions = Vec::with_capacity(injections.len());

    for injection in injections {
        let (provider_manager, function_name, field_name) = injection;

        let provider_name =
            create_provider_name_by_fn_and_struct_ident(&function_name, &provider_manager)?;

        if !provider_name.contains(&original_struct_name) && !dependency_info.unique_types.insert(provider_name.clone()) {
                return Err(Error::new(
                    original_struct_ident.span(),
                    format!("Conflict in dependency: {}", provider_name),
                ));
        }

        dependencies.push((field_name.clone(), provider_name));

        let provider_trait = create_type_reference("ProviderTrait", true, true, true);
        field_definitions.push(quote! { #field_name: #provider_trait });
    }

    let generated_code = generate_provider_code(
        &provider_name,
        &field_definitions,
        &modified_block,
        &provider_token,
        trait_reference_name,
        impl_fn_params,
        original_struct_name,
    );
    Ok((generated_code, MetadataInfo {
        struct_name: provider_name,
        dependencies,
    }))
}

fn generate_provider_code(
    provider_name: &Ident,
    field_defs: &[TokenStream],
    method_body: &syn::Block,
    provider_token: &str,
    trait_name: &Ident,
    impl_fn_params: Vec<(Ident, syn::Type)>,
    original_struct_name: String,
) -> TokenStream {
    let params_token_stream = impl_fn_params
        .iter()
        .map(|(name, ty)| {
            quote! {
                let #name = *iter.next().unwrap().downcast::<#ty>().unwrap();
            }
        })
        .collect::<Vec<_>>();

    quote! {
        struct #provider_name {
            #(#field_defs),*
        }

        impl ::toni_core::traits_helpers::#trait_name for #provider_name {
            #[inline]
            fn execute(&self, params: Vec<Box<dyn ::std::any::Any>>) -> Box<dyn ::std::any::Any> {
                let mut iter = params.into_iter();
                #(#params_token_stream)*
                #method_body
            }

            #[inline]
            fn get_token(&self) -> String {
                #provider_token.to_string()
            }

            #[inline]
            fn get_token_manager(&self) -> String {
                #original_struct_name.to_string()
            }
        }
    }
}
