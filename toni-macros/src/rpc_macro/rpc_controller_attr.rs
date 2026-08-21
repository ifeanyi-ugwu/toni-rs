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
    add_clone_and_inject_fields, generate_rpc_controller_system,
};
use crate::shared::scope_parser::{ControllerScope, RpcControllerArgs};
use crate::utils::extracts::extract_struct_dependencies;

/// The `RpcControllerSource` companion generated beside the controller struct.
pub fn rpc_source_ident(struct_name: &Ident) -> Ident {
    Ident::new(
        &format!("{}RpcControllerSource", struct_name),
        struct_name.span(),
    )
}

pub fn handle_rpc_controller(attr: TokenStream, item: TokenStream) -> Result<TokenStream> {
    let struct_def = parse2::<ItemStruct>(item)?;
    let args = parse2::<RpcControllerArgs>(attr)?;

    if args.struct_def.is_some() {
        return Err(syn::Error::new_spanned(
            &struct_def.ident,
            "the inline-struct form `#[rpc_controller(pub struct …)]` has been removed; place \
             `#[rpc_controller]` on the struct and `#[patterns]` on its impl",
        ));
    }

    let struct_name = struct_def.ident.clone();

    let emitted_struct = add_clone_and_inject_fields(&struct_def);
    let dependencies = extract_struct_dependencies(&struct_def)?;
    let controller_system = generate_rpc_controller_system(
        &struct_name,
        &dependencies,
        matches!(args.scope, ControllerScope::Request),
    );
    let trait_impl = generate_rpc_trait_impl(&struct_name);

    Ok(quote! {
        #[allow(dead_code)]
        #emitted_struct

        #controller_system

        #trait_impl
    })
}

/// The two impls a controller needs: `RpcControllerTrait` on the struct, and `RpcControllerSource`
/// on a generated companion. Both route through `Self::__toni_rpc_*`, which the `#[patterns]` impl
/// shadows with inherent fns; without that impl, the `RpcHandlersBridge` defaults answer.
///
/// The companion is the fork between a controller built once and a controller built per call. Which
/// variant it is settled at startup, by the factory.
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

        enum #source_name {
            /// Built at startup and shared by every call.
            Singleton(::std::sync::Arc<Box<dyn ::toni::rpc::RpcControllerTrait>>),
            /// The controller's own provider, resolved inside the call being served.
            PerCall(::std::sync::Arc<Box<dyn ::toni::traits_helpers::Provider>>),
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

            fn metadata(&self) -> ::std::sync::Arc<::toni::context::Metadata> {
                use ::toni::__rpc::RpcHandlersBridge as _;
                ::std::sync::Arc::new(<#struct_name>::__toni_rpc_metadata())
            }

            fn handler_metadata(
                &self,
            ) -> Vec<(String, ::std::sync::Arc<::toni::context::Metadata>)> {
                use ::toni::__rpc::RpcHandlersBridge as _;
                <#struct_name>::__toni_rpc_handler_metadata()
                    .into_iter()
                    .map(|(__p, __m)| (__p, ::std::sync::Arc::new(__m)))
                    .collect()
            }

            async fn instance(
                &self,
                ctx: &::toni::context::RpcContext,
            ) -> ::std::sync::Arc<Box<dyn ::toni::rpc::RpcControllerTrait>> {
                match self {
                    Self::Singleton(__instance) => __instance.clone(),
                    Self::PerCall(__provider) => {
                        // The provider caches in the execution, so a controller asked for twice in
                        // one call is built once; init/bootstrap fire on the instance the call is
                        // served by, as they do for a request-scoped HTTP controller.
                        let __any = __provider
                            .execute(vec![], ::toni::ProviderContext::Rpc(ctx.clone()))
                            .await;
                        let __concrete = *__any.downcast::<#struct_name>().unwrap_or_else(|_| panic!(
                            "RPC controller '{}' resolved to a different type",
                            #struct_token
                        ));
                        {
                            use ::toni::__lifecycle::LifecycleBridge as _;
                            let _ = #struct_name::__toni_lc_on_init(&__concrete).await;
                            let _ = #struct_name::__toni_lc_on_bootstrap(&__concrete).await;
                        }
                        ::std::sync::Arc::new(
                            Box::new(__concrete) as Box<dyn ::toni::rpc::RpcControllerTrait>
                        )
                    }
                }
            }
        }
    }
}
