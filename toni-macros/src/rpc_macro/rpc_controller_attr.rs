//! `#[rpc_controller]` — the struct-attribute form.
//!
//! Placed on the struct, exactly like `#[injectable]`: `#[inject]` fields are dependencies and
//! construction/lifecycle reach the impl through the `toni::__construct` / `toni::__lifecycle`
//! bridges. An RPC controller is a provider with a role, so this emits the provider wiring (carrying
//! the rpc-controller role) plus `impl RpcControllerTrait`, with `get_token` baked from the struct
//! name and `get_patterns` / `handle_message` / `enhancers` delegating to the `RpcHandlersBridge`.
//! The pattern handlers live in a sibling `#[patterns] impl`, which the struct attribute never sees.
//!
//! A controller with no `#[patterns]` impl is valid: it registers as a provider but routes nothing
//! (the bridge defaults answer — empty pattern list, `PatternNotFound`, no enhancers).

use proc_macro2::TokenStream;
use quote::quote;
use syn::{Ident, ItemStruct, Result, parse2};

use crate::provider_macro::instance_injection::{
    EnhancerTraits, add_clone_and_inject_fields, generate_provider_from_struct_with_traits,
};
use crate::shared::scope_parser::ProviderScope;

pub fn handle_rpc_controller(attr: TokenStream, item: TokenStream) -> Result<TokenStream> {
    let struct_def = parse2::<ItemStruct>(item)?;

    if !attr.is_empty() {
        return Err(syn::Error::new_spanned(
            &struct_def.ident,
            "the inline-struct form `#[rpc_controller(pub struct …)]` has been removed; place \
             `#[rpc_controller]` on the struct and `#[patterns]` on its impl",
        ));
    }

    let struct_name = struct_def.ident.clone();

    let emitted_struct = add_clone_and_inject_fields(&struct_def);
    let provider_system = generate_provider_from_struct_with_traits(
        &struct_def,
        ProviderScope::Singleton,
        None,
        EnhancerTraits {
            is_rpc_controller: true,
            ..Default::default()
        },
    )?;
    let trait_impl = generate_rpc_trait_impl(&struct_name);

    Ok(quote! {
        #[allow(dead_code)]
        #emitted_struct

        #provider_system

        #trait_impl
    })
}

/// `impl RpcControllerTrait` for the controller struct. `get_token` is the struct name; the pattern
/// list, message routing, and enhancers delegate to `Self::__toni_rpc_*`, which the `#[patterns]`
/// impl shadows with inherent fns. Without that impl, the `RpcHandlersBridge` defaults answer.
fn generate_rpc_trait_impl(struct_name: &Ident) -> TokenStream {
    let struct_token = struct_name.to_string();

    quote! {
        #[::toni::async_trait]
        impl ::toni::rpc::RpcControllerTrait for #struct_name {
            fn get_token(&self) -> String {
                #struct_token.to_string()
            }

            fn get_patterns(&self) -> Vec<String> {
                use ::toni::__rpc::RpcHandlersBridge as _;
                <Self>::__toni_rpc_get_patterns(self)
            }

            async fn handle_message(
                &self,
                ctx: &mut ::toni::context::RpcContext,
            ) -> ::toni::http_helpers::ExecutionResult<
                ::std::option::Option<::toni::rpc::RpcData>,
                ::toni::rpc::RpcError,
            > {
                use ::toni::__rpc::RpcHandlersBridge as _;
                <Self>::__toni_rpc_handle_message(self, ctx).await
            }

            fn enhancers(&self) -> ::toni::rpc::RpcEnhancers {
                use ::toni::__rpc::RpcHandlersBridge as _;
                <Self>::__toni_rpc_enhancers(self)
            }
        }
    }
}
