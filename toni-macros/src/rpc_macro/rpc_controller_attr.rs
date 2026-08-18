//! `#[rpc_controller]` — the struct-attribute form.
//!
//! Placed on the struct, exactly like `#[injectable]`: `#[inject]` fields are dependencies and
//! construction/lifecycle reach the impl through the `toni::__construct` / `toni::__lifecycle`
//! bridges. An RPC controller is a provider with a role, so this emits the provider wiring (carrying
//! the rpc-controller role) plus two impls: `RpcControllerTrait` on the struct, whose sole
//! `handle_message` delegates to the `RpcHandlersBridge`, and `RpcControllerSource` on a generated
//! companion, which answers the token, the patterns and the enhancer tokens without an instance and
//! hands one over when a call arrives. The pattern handlers live in a sibling `#[patterns] impl`,
//! which the struct attribute never sees.
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

/// The `RpcControllerSource` companion generated beside the controller struct.
pub fn rpc_source_ident(struct_name: &Ident) -> Ident {
    Ident::new(
        &format!("{}RpcControllerSource", struct_name),
        struct_name.span(),
    )
}

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

/// The two impls a controller needs: `RpcControllerTrait` on the struct, and `RpcControllerSource`
/// on a generated companion. Both route through `Self::__toni_rpc_*`, which the `#[patterns]` impl
/// shadows with inherent fns; without that impl, the `RpcHandlersBridge` defaults answer.
fn generate_rpc_trait_impl(struct_name: &Ident) -> TokenStream {
    let struct_token = struct_name.to_string();
    let source_name = rpc_source_ident(struct_name);

    quote! {
        #[::toni::async_trait]
        impl ::toni::rpc::RpcControllerTrait for #struct_name {
            async fn handle_message(
                &self,
                ctx: &::toni::context::RpcContext,
            ) -> ::toni::http_helpers::ExecutionResult<
                ::std::option::Option<::toni::rpc::RpcData>,
                ::toni::rpc::RpcError,
            > {
                use ::toni::__rpc::RpcHandlersBridge as _;
                <Self>::__toni_rpc_handle_message(self, ctx).await
            }
        }

        pub struct #source_name {
            instance: ::std::sync::Arc<Box<dyn ::toni::rpc::RpcControllerTrait>>,
        }

        #[::toni::async_trait]
        impl ::toni::rpc::RpcControllerSource for #source_name {
            fn get_token(&self) -> String {
                #struct_token.to_string()
            }

            fn get_patterns(&self) -> Vec<String> {
                use ::toni::__rpc::RpcHandlersBridge as _;
                <#struct_name>::__toni_rpc_patterns()
            }

            fn enhancers(&self) -> ::toni::rpc::RpcEnhancers {
                use ::toni::__rpc::RpcHandlersBridge as _;
                <#struct_name>::__toni_rpc_enhancers()
            }

            async fn instance(
                &self,
                _ctx: &::toni::context::RpcContext,
            ) -> ::std::sync::Arc<Box<dyn ::toni::rpc::RpcControllerTrait>> {
                self.instance.clone()
            }
        }
    }
}
