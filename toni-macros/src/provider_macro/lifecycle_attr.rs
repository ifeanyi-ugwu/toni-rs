//! The `#[on_init]` / `#[on_bootstrap]` / `#[on_destroy]` / `#[before_shutdown]` / `#[on_shutdown]`
//! hook macros for `#[injectable]` providers.
//!
//! The derive generates the provider from the struct and can't see these methods. Each macro emits
//! the user's method unchanged plus an inherent `__toni_lc_*` forwarder that shadows the blanket
//! `toni::__lifecycle::LifecycleBridge` no-op of the same name. The derive's `Provider` impl always
//! calls the `__toni_lc_*` methods, so the user hook runs when present and the no-op otherwise.
//!
//! Hooks are `async fn(&self)`. `on_init`/`on_bootstrap` return `toni::InitResult`; the three
//! shutdown/destroy hooks return `()`, and `before_shutdown`/`on_shutdown` receive
//! `signal: Option<String>`.

use proc_macro2::TokenStream;
use quote::{format_ident, quote};
use syn::{FnArg, ImplItemFn, Result, parse2, spanned::Spanned};

/// Which `Provider`/`LifecycleBridge` hook a macro targets.
#[derive(Clone, Copy)]
pub enum Hook {
    OnInit,
    OnBootstrap,
    OnDestroy,
    BeforeShutdown,
    OnShutdown,
}

impl Hook {
    /// Inherent bridge method this hook forwards into.
    fn bridge_method(self) -> &'static str {
        match self {
            Hook::OnInit => "__toni_lc_on_init",
            Hook::OnBootstrap => "__toni_lc_on_bootstrap",
            Hook::OnDestroy => "__toni_lc_on_destroy",
            Hook::BeforeShutdown => "__toni_lc_before_shutdown",
            Hook::OnShutdown => "__toni_lc_on_shutdown",
        }
    }

    fn takes_signal(self) -> bool {
        matches!(self, Hook::BeforeShutdown | Hook::OnShutdown)
    }

    fn returns_init_result(self) -> bool {
        matches!(self, Hook::OnInit | Hook::OnBootstrap)
    }

    fn attr_name(self) -> &'static str {
        match self {
            Hook::OnInit => "on_init",
            Hook::OnBootstrap => "on_bootstrap",
            Hook::OnDestroy => "on_destroy",
            Hook::BeforeShutdown => "before_shutdown",
            Hook::OnShutdown => "on_shutdown",
        }
    }
}

pub fn handle_hook(hook: Hook, item: TokenStream) -> Result<TokenStream> {
    let method: ImplItemFn = parse2(item)?;

    if method.sig.asyncness.is_none() {
        return Err(syn::Error::new(
            method.sig.span(),
            format!(
                "#[{}] must be an `async fn` — lifecycle hooks are uniformly async",
                hook.attr_name()
            ),
        ));
    }

    // The user method must take `&self`; the forwarder calls it on the instance.
    let takes_self = matches!(method.sig.inputs.first(), Some(FnArg::Receiver(_)));
    if !takes_self {
        return Err(syn::Error::new(
            method.sig.span(),
            format!("#[{}] must take `&self`", hook.attr_name()),
        ));
    }

    let user_method_name = method.sig.ident.clone();
    let bridge_method = format_ident!("{}", hook.bridge_method());

    // Whether the user wrote the optional `signal` parameter (shutdown hooks). A user signature
    // may include it or omit it; forward accordingly.
    let user_param_count = method
        .sig
        .inputs
        .iter()
        .filter(|a| matches!(a, FnArg::Typed(_)))
        .count();

    let forward_call = if hook.takes_signal() {
        if user_param_count >= 1 {
            quote! { self.#user_method_name(__signal).await }
        } else {
            quote! { self.#user_method_name().await }
        }
    } else {
        quote! { self.#user_method_name().await }
    };

    let bridge_fn = if hook.returns_init_result() {
        quote! {
            #[doc(hidden)]
            async fn #bridge_method(&self) -> ::toni::InitResult {
                #forward_call
            }
        }
    } else if hook.takes_signal() {
        quote! {
            #[doc(hidden)]
            #[allow(unused_variables)]
            async fn #bridge_method(&self, __signal: ::std::option::Option<::std::string::String>) {
                #forward_call;
            }
        }
    } else {
        quote! {
            #[doc(hidden)]
            async fn #bridge_method(&self) {
                #forward_call;
            }
        }
    };

    Ok(quote! {
        #method
        #bridge_fn
    })
}
