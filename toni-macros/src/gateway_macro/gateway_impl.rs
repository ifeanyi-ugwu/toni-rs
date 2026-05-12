use proc_macro2::TokenStream;
use quote::quote;
use syn::{Attribute, Error, Ident, ItemImpl, ItemStruct, LitInt, LitStr, Result, parse2};

use crate::controller_macro::controller_struct::{extract_constructor_params, has_new_method};
use crate::enhancer::enhancer::{
    create_enhancer_infos, get_enhancers_attr, has_enhancer_attribute,
};
use crate::provider_macro::instance_injection::generate_instance_provider_system;
use crate::shared::attr_is;
use crate::shared::dependency_info::DependencySource;
use crate::shared::scope_parser::ProviderScope;
use crate::utils::extracts::extract_struct_dependencies;

/// Parse WebSocket gateway arguments.
/// Supports:
/// - `#[websocket_gateway] impl Foo { ... }` — struct defined separately (preferred)
/// - `#[websocket_gateway("/path")] impl Foo { ... }` — with path
/// - `#[websocket_gateway("/path", pub struct Foo { ... })]` — inline struct (legacy)
struct GatewayArgs {
    path: String,
    namespace: Option<String>,
    port: Option<u16>,
    /// `None` when the struct is defined above the impl.
    struct_def: Option<ItemStruct>,
}

impl syn::parse::Parse for GatewayArgs {
    fn parse(input: syn::parse::ParseStream) -> Result<Self> {
        let mut path = None;
        let mut namespace = None;
        let mut port = None;
        let mut struct_def = None;

        while !input.is_empty() {
            if input.peek(syn::Token![pub]) || input.peek(syn::Token![struct]) {
                struct_def = Some(input.parse::<ItemStruct>()?);
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

            return Err(input.error("Expected path string, namespace, port, or struct definition"));
        }

        Ok(GatewayArgs {
            path: path.unwrap_or_else(|| "/".to_string()),
            namespace,
            port,
            struct_def,
        })
    }
}

pub fn handle_websocket_gateway(attr: TokenStream, item: TokenStream) -> Result<TokenStream> {
    let args = parse2::<GatewayArgs>(attr)?;
    let impl_block = parse2::<ItemImpl>(item)?;

    let struct_def = args.struct_def;
    let path = args.path;
    let namespace = args.namespace;
    let port = args.port;

    let mut dependencies = match &struct_def {
        Some(s) => extract_struct_dependencies(s)?,
        None => crate::shared::dependency_info::DependencyInfo {
            fields: vec![],
            owned_fields: vec![],
            init_method: None,
            constructor_params: vec![],
            unique_types: std::collections::HashSet::new(),
            source: DependencySource::None,
        },
    };

    if has_new_method(&impl_block) {
        let params = extract_constructor_params(&impl_block, "new")?;
        dependencies.init_method = Some("new".to_string());
        dependencies.constructor_params = params;
        dependencies.source = DependencySource::Constructor("new".to_string());
    } else if struct_def.is_none() {
        return Err(syn::Error::new_spanned(
            &impl_block.self_ty,
            "add a `fn new(...) -> Self` constructor to declare this gateway's dependencies, \
             or move the struct definition into the macro attribute",
        ));
    }

    generate_gateway_impl(
        struct_def.as_ref(),
        &impl_block,
        &dependencies,
        &path,
        namespace.as_deref(),
        port,
    )
}

fn generate_gateway_impl(
    struct_def: Option<&ItemStruct>,
    impl_block: &ItemImpl,
    dependencies: &crate::shared::dependency_info::DependencyInfo,
    path: &str,
    namespace: Option<&str>,
    port: Option<u16>,
) -> Result<TokenStream> {
    let struct_name = match struct_def {
        Some(s) => s.ident.clone(),
        None => crate::utils::extracts::extract_impl_self_ident(impl_block)?,
    };
    let struct_name = &struct_name;
    let struct_token = struct_name.to_string();

    let mut message_handlers = Vec::new();
    let mut on_connect_method = None;
    let mut on_disconnect_method = None;
    let mut after_init_method = None;

    for item in &impl_block.items {
        if let syn::ImplItem::Fn(method) = item {
            if let Some(event_name) = extract_subscribe_message_event(&method.attrs) {
                message_handlers.push((event_name, method.clone()));
            } else if has_attribute(&method.attrs, "on_connect") {
                on_connect_method = Some(method.clone());
            } else if has_attribute(&method.attrs, "on_disconnect") {
                on_disconnect_method = Some(method.clone());
            } else if has_attribute(&method.attrs, "after_init") {
                after_init_method = Some(method.clone());
            }
        }
    }

    // User errors are preserved past the macro boundary as
    // `ExecutionResult::Err` carrying the transport's `WsError`. `Into::into`
    // calls the `From<E: Error> for WsError` blanket so domain errors
    // implementing `toni::Error` lift automatically.
    let match_arms: Vec<_> = message_handlers
        .iter()
        .map(|(event, method)| {
            let method_name = &method.sig.ident;

            quote! {
                #event => {
                    match self.#method_name(client, message).await {
                        Ok(__output) => ::toni::http_helpers::ExecutionResult::Ok(__output),
                        Err(__err) => ::toni::http_helpers::ExecutionResult::Err(
                            ::std::convert::Into::<toni::WsError>::into(__err),
                        ),
                    }
                }
            }
        })
        .collect();

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

    // Note: Clone derive is handled by generate_instance_provider_system()
    let on_connect_impl = on_connect_method.as_ref().map(|method| {
        let method_name = &method.sig.ident;
        quote! {
            async fn on_connect(
                &self,
                client: &toni::WsClient,
                _context: &toni::context::WsContext,
            ) -> Result<(), toni::WsError> {
                self.#method_name(client).await
            }
        }
    });

    let on_disconnect_impl = on_disconnect_method.as_ref().map(|method| {
        let method_name = &method.sig.ident;
        quote! {
            async fn on_disconnect(
                &self,
                client: &toni::WsClient,
                _reason: toni::DisconnectReason,
            ) {
                self.#method_name(client).await;
            }
        }
    });

    let after_init_impl = after_init_method.as_ref().map(|method| {
        let method_name = &method.sig.ident;
        quote! {
            async fn after_init(&self) {
                self.#method_name().await;
            }
        }
    });

    // Extract gateway-level enhancer attrs from the impl block
    let gateway_enhancers_attr = get_enhancers_attr(&impl_block.attrs)?;
    let enhancer_infos =
        create_enhancer_infos(gateway_enhancers_attr, std::collections::HashMap::new())?;

    let binding = Vec::new();
    let guard_tokens: Vec<_> = enhancer_infos
        .get("guards")
        .unwrap_or(&binding)
        .iter()
        .filter(|info| !info.token_expr.is_empty())
        .map(|info| &info.token_expr)
        .collect();
    let interceptor_tokens: Vec<_> = enhancer_infos
        .get("interceptors")
        .unwrap_or(&binding)
        .iter()
        .filter(|info| !info.token_expr.is_empty())
        .map(|info| &info.token_expr)
        .collect();
    let pipe_tokens: Vec<_> = enhancer_infos
        .get("pipes")
        .unwrap_or(&binding)
        .iter()
        .filter(|info| !info.token_expr.is_empty())
        .map(|info| &info.token_expr)
        .collect();
    let error_handler_tokens: Vec<_> = enhancer_infos
        .get("error_handlers")
        .unwrap_or(&binding)
        .iter()
        .filter(|info| !info.token_expr.is_empty())
        .map(|info| &info.token_expr)
        .collect();

    let guard_tokens_impl = if !guard_tokens.is_empty() {
        quote! {
            fn get_guard_tokens(&self) -> Vec<String> {
                vec![#(#guard_tokens),*]
            }
        }
    } else {
        quote! {}
    };
    let interceptor_tokens_impl = if !interceptor_tokens.is_empty() {
        quote! {
            fn get_interceptor_tokens(&self) -> Vec<String> {
                vec![#(#interceptor_tokens),*]
            }
        }
    } else {
        quote! {}
    };
    let pipe_tokens_impl = if !pipe_tokens.is_empty() {
        quote! {
            fn get_pipe_tokens(&self) -> Vec<String> {
                vec![#(#pipe_tokens),*]
            }
        }
    } else {
        quote! {}
    };
    let error_handler_tokens_impl = if !error_handler_tokens.is_empty() {
        quote! {
            fn get_error_handler_tokens(&self) -> Vec<String> {
                vec![#(#error_handler_tokens),*]
            }
        }
    } else {
        quote! {}
    };

    // Extract per-handler enhancer tokens from each #[subscribe_message] method.
    // Stored as (event, guard_tokens, interceptor_tokens, pipe_tokens, error_handler_tokens).
    let mut handler_enhancer_entries: Vec<(
        String,
        Vec<TokenStream>,
        Vec<TokenStream>,
        Vec<TokenStream>,
        Vec<TokenStream>,
    )> = Vec::new();

    for (event, method) in &message_handlers {
        let method_enhancers_attr = get_enhancers_attr(&method.attrs)?;
        if method_enhancers_attr.is_empty() {
            continue;
        }
        let handler_infos =
            create_enhancer_infos(method_enhancers_attr, std::collections::HashMap::new())?;
        let binding = Vec::new();
        let hg: Vec<TokenStream> = handler_infos
            .get("guards")
            .unwrap_or(&binding)
            .iter()
            .filter(|i| !i.token_expr.is_empty())
            .map(|i| i.token_expr.clone())
            .collect();
        let hi: Vec<TokenStream> = handler_infos
            .get("interceptors")
            .unwrap_or(&binding)
            .iter()
            .filter(|i| !i.token_expr.is_empty())
            .map(|i| i.token_expr.clone())
            .collect();
        let hp: Vec<TokenStream> = handler_infos
            .get("pipes")
            .unwrap_or(&binding)
            .iter()
            .filter(|i| !i.token_expr.is_empty())
            .map(|i| i.token_expr.clone())
            .collect();
        let he: Vec<TokenStream> = handler_infos
            .get("error_handlers")
            .unwrap_or(&binding)
            .iter()
            .filter(|i| !i.token_expr.is_empty())
            .map(|i| i.token_expr.clone())
            .collect();
        if !hg.is_empty() || !hi.is_empty() || !hp.is_empty() || !he.is_empty() {
            handler_enhancer_entries.push((event.clone(), hg, hi, hp, he));
        }
    }

    let handler_events_impl = if !handler_enhancer_entries.is_empty() {
        let events: Vec<&str> = handler_enhancer_entries
            .iter()
            .map(|(e, _, _, _, _)| e.as_str())
            .collect();
        quote! {
            fn get_handler_events(&self) -> Vec<String> {
                vec![#(#events.to_string()),*]
            }
        }
    } else {
        quote! {}
    };

    let handler_guard_tokens_impl = {
        let arms: Vec<_> = handler_enhancer_entries
            .iter()
            .filter(|(_, g, _, _, _)| !g.is_empty())
            .map(|(event, guards, _, _, _)| quote! { #event => vec![#(#guards),*], })
            .collect();
        if !arms.is_empty() {
            quote! {
                fn get_handler_guard_tokens(&self, event: &str) -> Vec<String> {
                    match event {
                        #(#arms)*
                        _ => vec![],
                    }
                }
            }
        } else {
            quote! {}
        }
    };

    let handler_interceptor_tokens_impl = {
        let arms: Vec<_> = handler_enhancer_entries
            .iter()
            .filter(|(_, _, i, _, _)| !i.is_empty())
            .map(|(event, _, interceptors, _, _)| {
                quote! { #event => vec![#(#interceptors),*], }
            })
            .collect();
        if !arms.is_empty() {
            quote! {
                fn get_handler_interceptor_tokens(&self, event: &str) -> Vec<String> {
                    match event {
                        #(#arms)*
                        _ => vec![],
                    }
                }
            }
        } else {
            quote! {}
        }
    };

    let handler_pipe_tokens_impl = {
        let arms: Vec<_> = handler_enhancer_entries
            .iter()
            .filter(|(_, _, _, p, _)| !p.is_empty())
            .map(|(event, _, _, pipes, _)| quote! { #event => vec![#(#pipes),*], })
            .collect();
        if !arms.is_empty() {
            quote! {
                fn get_handler_pipe_tokens(&self, event: &str) -> Vec<String> {
                    match event {
                        #(#arms)*
                        _ => vec![],
                    }
                }
            }
        } else {
            quote! {}
        }
    };

    let handler_error_handler_tokens_impl = {
        let arms: Vec<_> = handler_enhancer_entries
            .iter()
            .filter(|(_, _, _, _, e)| !e.is_empty())
            .map(|(event, _, _, _, handlers)| {
                quote! { #event => vec![#(#handlers),*], }
            })
            .collect();
        if !arms.is_empty() {
            quote! {
                fn get_handler_error_handler_tokens(&self, event: &str) -> Vec<String> {
                    match event {
                        #(#arms)*
                        _ => vec![],
                    }
                }
            }
        } else {
            quote! {}
        }
    };

    // Clean impl block: strip gateway marker attrs and all enhancer attrs from methods and block
    let mut impl_def = impl_block.clone();
    impl_def.attrs.retain(|attr| !has_enhancer_attribute(attr));
    for item in impl_def.items.iter_mut() {
        if let syn::ImplItem::Fn(method) = item {
            method.attrs.retain(|attr| {
                !attr_is(attr, "subscribe_message")
                    && !attr_is(attr, "on_connect")
                    && !attr_is(attr, "on_disconnect")
                    && !attr_is(attr, "after_init")
                    && !has_enhancer_attribute(attr)
            });
        }
    }

    let provider_system = generate_instance_provider_system(
        struct_def,
        &impl_def,
        dependencies,
        ProviderScope::Singleton,
        true,  // is_gateway
        false, // is_rpc_controller
        false, // is_grpc_service
    )?;

    let gateway_trait_impl = quote! {
        #[toni::async_trait]
        impl toni::GatewayTrait for #struct_name {
            fn get_token(&self) -> String {
                #struct_token.to_string()
            }

            fn get_path(&self) -> String {
                #path.to_string()
            }

            #namespace_impl

            #port_impl

            #after_init_impl

            #on_connect_impl

            #on_disconnect_impl

            #guard_tokens_impl

            #interceptor_tokens_impl

            #pipe_tokens_impl

            #error_handler_tokens_impl

            #handler_events_impl

            #handler_guard_tokens_impl

            #handler_interceptor_tokens_impl

            #handler_pipe_tokens_impl

            #handler_error_handler_tokens_impl

            async fn handle_event(
                &self,
                client: toni::WsClient,
                message: toni::WsMessage,
                event: &str,
            ) -> ::toni::http_helpers::ExecutionResult<toni::WsHandlerOutput, toni::WsError> {
                match event {
                    #(#match_arms)*
                    _ => ::toni::http_helpers::ExecutionResult::Err(
                        toni::WsError::EventNotFound(
                            format!("Unknown event: {}", event),
                        ),
                    ),
                }
            }
        }
    };

    Ok(quote! {
        #provider_system

        #gateway_trait_impl
    })
}

fn extract_subscribe_message_event(attrs: &[Attribute]) -> Option<String> {
    for attr in attrs {
        if attr_is(attr, "subscribe_message") {
            if let Ok(lit) = attr.parse_args::<LitStr>() {
                return Some(lit.value());
            }
        }
    }
    None
}

fn has_attribute(attrs: &[Attribute], name: &str) -> bool {
    attrs.iter().any(|attr| attr_is(attr, name))
}
