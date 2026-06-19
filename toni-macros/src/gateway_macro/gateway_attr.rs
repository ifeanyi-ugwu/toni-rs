//! `#[websocket_gateway("/path", namespace = "...", port = N)]` — the struct-attribute form.
//!
//! Placed on the struct, exactly like `#[injectable]`: `#[inject]` fields are dependencies and
//! construction/lifecycle reach the impl through the `toni::__construct` / `toni::__lifecycle`
//! bridges. A gateway is a provider with a role, so this emits the provider wiring (carrying the
//! gateway role) plus `impl GatewayTrait`, baking path/namespace/port from the attribute and
//! delegating the behavior methods to the `WsHandlersBridge`. The message handlers live in a sibling
//! `#[subscriptions] impl`; the connection hooks (`#[on_connect]` / `#[on_disconnect]` /
//! `#[after_init]`) are their own per-method macros. The struct attribute sees none of them.
//!
//! A gateway with no `#[subscriptions]` impl and no hooks is valid: every behavior method resolves to
//! the bridge default, so it accepts connections (and can broadcast) with no message routing.

use proc_macro2::TokenStream;
use quote::quote;
use syn::{Error, Ident, ItemStruct, LitInt, LitStr, Result, parse2};

use crate::provider_macro::instance_injection::{
    EnhancerTraits, add_clone_and_inject_fields, generate_provider_from_struct_with_traits,
};
use crate::shared::scope_parser::ProviderScope;

struct GatewayArgs {
    path: String,
    namespace: Option<String>,
    port: Option<u16>,
    /// Set when the removed inline-struct form (`pub struct …` in the attribute) is detected, so the
    /// caller can emit a migration error.
    saw_inline_struct: bool,
}

impl syn::parse::Parse for GatewayArgs {
    fn parse(input: syn::parse::ParseStream) -> Result<Self> {
        let mut path = None;
        let mut namespace = None;
        let mut port = None;
        let mut saw_inline_struct = false;

        while !input.is_empty() {
            if input.peek(syn::Token![pub]) || input.peek(syn::Token![struct]) {
                saw_inline_struct = true;
                break;
            }

            if input.peek(syn::LitStr) && path.is_none() {
                path = Some(input.parse::<LitStr>()?.value());
                if !input.is_empty() && input.peek(syn::Token![,]) {
                    input.parse::<syn::Token![,]>()?;
                }
                continue;
            }

            if input.peek(syn::Ident) {
                let ident: Ident = input.parse()?;
                if ident == "namespace" {
                    input.parse::<syn::Token![=]>()?;
                    namespace = Some(input.parse::<LitStr>()?.value());
                    if !input.is_empty() && input.peek(syn::Token![,]) {
                        input.parse::<syn::Token![,]>()?;
                    }
                    continue;
                }
                if ident == "port" {
                    input.parse::<syn::Token![=]>()?;
                    let lit = input.parse::<LitInt>()?;
                    port = Some(lit.base10_parse::<u16>().map_err(|e| {
                        Error::new(lit.span(), format!("port must be a valid u16: {}", e))
                    })?);
                    if !input.is_empty() && input.peek(syn::Token![,]) {
                        input.parse::<syn::Token![,]>()?;
                    }
                    continue;
                }
                return Err(Error::new(ident.span(), "Unknown argument"));
            }

            return Err(input.error("Expected path string, namespace, or port"));
        }

        Ok(GatewayArgs {
            path: path.unwrap_or_else(|| "/".to_string()),
            namespace,
            port,
            saw_inline_struct,
        })
    }
}

pub fn handle_websocket_gateway(attr: TokenStream, item: TokenStream) -> Result<TokenStream> {
    let struct_def = parse2::<ItemStruct>(item)?;
    let args = parse2::<GatewayArgs>(attr)?;

    if args.saw_inline_struct {
        return Err(syn::Error::new_spanned(
            &struct_def.ident,
            "the inline-struct form `#[websocket_gateway(\"/p\", pub struct …)]` has been removed; \
             place `#[websocket_gateway(\"/p\")]` on the struct and `#[subscriptions]` on its impl",
        ));
    }

    let struct_name = struct_def.ident.clone();

    let emitted_struct = add_clone_and_inject_fields(&struct_def);
    let provider_system = generate_provider_from_struct_with_traits(
        &struct_def,
        ProviderScope::Singleton,
        None,
        EnhancerTraits {
            is_gateway: true,
            ..Default::default()
        },
    )?;
    let gateway_trait_impl = generate_gateway_trait_impl(
        &struct_name,
        &args.path,
        args.namespace.as_deref(),
        args.port,
    );

    Ok(quote! {
        #[allow(dead_code)]
        #emitted_struct

        #provider_system

        #gateway_trait_impl
    })
}

/// `impl GatewayTrait` for the gateway struct. Identity (token), path, namespace, and port are baked
/// from the attribute; the behavior methods delegate to `Self::__toni_ws_*`, which the
/// `#[subscriptions]` impl shadows with inherent fns. Without that impl, the `WsHandlersBridge`
/// defaults answer — no routing, connections allowed.
fn generate_gateway_trait_impl(
    struct_name: &Ident,
    path: &str,
    namespace: Option<&str>,
    port: Option<u16>,
) -> TokenStream {
    let struct_token = struct_name.to_string();

    let namespace_impl = namespace.map(|ns| {
        quote! {
            fn get_namespace(&self) -> Option<String> {
                Some(#ns.to_string())
            }
        }
    });

    let port_impl = port.map(|p| {
        quote! {
            fn get_port(&self) -> Option<u16> {
                Some(#p)
            }
        }
    });

    quote! {
        #[::toni::async_trait]
        impl ::toni::GatewayTrait for #struct_name {
            fn get_token(&self) -> String {
                #struct_token.to_string()
            }

            fn get_path(&self) -> String {
                #path.to_string()
            }

            #namespace_impl

            #port_impl

            async fn after_init(&self) {
                use ::toni::__ws::WsHandlersBridge as _;
                <Self>::__toni_ws_after_init(self).await
            }

            async fn on_connect(
                &self,
                client: &::toni::WsClient,
                context: &::toni::context::WsContext,
            ) -> Result<(), ::toni::WsError> {
                use ::toni::__ws::WsHandlersBridge as _;
                <Self>::__toni_ws_on_connect(self, client, context).await
            }

            async fn on_disconnect(
                &self,
                client: &::toni::WsClient,
                reason: ::toni::DisconnectReason,
            ) {
                use ::toni::__ws::WsHandlersBridge as _;
                <Self>::__toni_ws_on_disconnect(self, client, reason).await
            }

            async fn handle_event(
                &self,
                client: ::toni::WsClient,
                message: ::toni::WsMessage,
                event: &str,
            ) -> ::toni::http_helpers::ExecutionResult<::toni::WsHandlerOutput, ::toni::WsError> {
                use ::toni::__ws::WsHandlersBridge as _;
                <Self>::__toni_ws_handle_event(self, client, message, event).await
            }

            fn enhancers(&self) -> ::toni::GatewayEnhancers {
                use ::toni::__ws::WsHandlersBridge as _;
                <Self>::__toni_ws_enhancers(self)
            }
        }
    }
}
