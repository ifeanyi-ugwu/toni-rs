//! The `#[on_connect]` / `#[on_disconnect]` / `#[after_init]` connection-hook macros for
//! `#[websocket_gateway]` structs.
//!
//! Each is single-slot — one method, one bridge fn — so each is its own per-method macro rather than
//! part of the `#[subscriptions]` impl scan (which only aggregates the variable set of
//! `#[subscribe_message]` handlers). A hook macro emits the user's method unchanged plus an inherent
//! `__toni_ws_*` forwarder that shadows the `toni::__ws::WsHandlersBridge` default of the same name.
//! So a hook can live on a gateway with or without a `#[subscriptions]` impl, and declaring the same
//! hook twice is a duplicate-definition compile error rather than a silent last-wins.

use proc_macro2::TokenStream;
use quote::{format_ident, quote};
use syn::{FnArg, ImplItemFn, Result, parse2, spanned::Spanned};

/// Which `WsHandlersBridge` connection hook a macro targets.
#[derive(Clone, Copy)]
pub enum ConnHook {
    OnConnect,
    OnDisconnect,
    AfterInit,
}

impl ConnHook {
    fn bridge_method(self) -> &'static str {
        match self {
            ConnHook::OnConnect => "__toni_ws_on_connect",
            ConnHook::OnDisconnect => "__toni_ws_on_disconnect",
            ConnHook::AfterInit => "__toni_ws_after_init",
        }
    }

    fn attr_name(self) -> &'static str {
        match self {
            ConnHook::OnConnect => "on_connect",
            ConnHook::OnDisconnect => "on_disconnect",
            ConnHook::AfterInit => "after_init",
        }
    }

    /// Whether the hook receives the connecting/disconnecting `WsClient`.
    fn takes_client(self) -> bool {
        matches!(self, ConnHook::OnConnect | ConnHook::OnDisconnect)
    }
}

pub fn handle_conn_hook(hook: ConnHook, item: TokenStream) -> Result<TokenStream> {
    let method: ImplItemFn = parse2(item)?;

    if method.sig.asyncness.is_none() {
        return Err(syn::Error::new(
            method.sig.span(),
            format!(
                "#[{}] must be an `async fn` — connection hooks are uniformly async",
                hook.attr_name()
            ),
        ));
    }

    if !matches!(method.sig.inputs.first(), Some(FnArg::Receiver(_))) {
        return Err(syn::Error::new(
            method.sig.span(),
            format!("#[{}] must take `&self`", hook.attr_name()),
        ));
    }

    let user_method_name = method.sig.ident.clone();
    let bridge_method = format_ident!("{}", hook.bridge_method());

    let user_param_count = method
        .sig
        .inputs
        .iter()
        .filter(|a| matches!(a, FnArg::Typed(_)))
        .count();

    // A hook takes what it asks for, positionally: nothing, the client, or the client and the
    // execution. The context is how a hook reaches the connection's session, which is not on the
    // client — so a connect or disconnect hook that needs it declares a second parameter.
    let forward_call = match (hook.takes_client(), user_param_count) {
        (true, 0) | (false, _) => quote! { self.#user_method_name().await },
        (true, 1) => quote! { self.#user_method_name(client).await },
        (true, _) => quote! { self.#user_method_name(client, context).await },
    };

    let bridge_fn = match hook {
        ConnHook::OnConnect => quote! {
            #[doc(hidden)]
            #[allow(non_snake_case, unused_variables, clippy::all)]
            async fn #bridge_method(
                &self,
                client: &::toni::WsClient,
                context: &::toni::context::WsContext,
            ) -> ::std::result::Result<(), ::toni::WsError> {
                #forward_call
            }
        },
        ConnHook::OnDisconnect => quote! {
            #[doc(hidden)]
            #[allow(non_snake_case, unused_variables, clippy::all)]
            async fn #bridge_method(
                &self,
                client: &::toni::WsClient,
                reason: ::toni::DisconnectReason,
                context: &::toni::context::WsContext,
            ) {
                #forward_call;
            }
        },
        ConnHook::AfterInit => quote! {
            #[doc(hidden)]
            #[allow(non_snake_case, clippy::all)]
            async fn #bridge_method(&self) {
                #forward_call;
            }
        },
    };

    Ok(quote! {
        #method
        #bridge_fn
    })
}
