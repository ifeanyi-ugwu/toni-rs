//! The `#[on_module_init]` / `#[on_application_bootstrap]` / `#[on_module_destroy]` /
//! `#[before_application_shutdown]` / `#[on_application_shutdown]` hook macros for `#[injectable]`
//! providers.
//!
//! The struct macro generates the provider from the struct and can't see these methods. Each macro
//! emits the user's method unchanged plus an inherent `__ulo_lc_*` forwarder that shadows the
//! blanket `ulo::__lifecycle::LifecycleBridge` no-op of the same name, and an inherent `__ULO_LC_*`
//! flag shadowing the blanket `false` in `LifecycleFlags`. The struct macro's `Provider` impl
//! always calls the `__ulo_lc_*` methods, so the user hook runs when present and the no-op
//! otherwise; `#[injectable]` reads the flags to refuse a hook on a scope that never fires it.
//!
//! Hooks are `async fn(&self)`. `on_module_init`/`on_application_bootstrap` return `ulo::di::InitResult`; the three
//! shutdown/destroy hooks return `()`, and `before_application_shutdown`/`on_application_shutdown` receive
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
            Hook::OnInit => "__ulo_lc_on_init",
            Hook::OnBootstrap => "__ulo_lc_on_bootstrap",
            Hook::OnDestroy => "__ulo_lc_on_destroy",
            Hook::BeforeShutdown => "__ulo_lc_before_shutdown",
            Hook::OnShutdown => "__ulo_lc_on_shutdown",
        }
    }

    /// Inherent flag this hook sets, read by `#[injectable]`'s scope check.
    pub(crate) fn flag(self) -> &'static str {
        match self {
            Hook::OnInit => "__ULO_LC_ON_INIT",
            Hook::OnBootstrap => "__ULO_LC_ON_BOOTSTRAP",
            Hook::OnDestroy => "__ULO_LC_ON_DESTROY",
            Hook::BeforeShutdown => "__ULO_LC_BEFORE_SHUTDOWN",
            Hook::OnShutdown => "__ULO_LC_ON_SHUTDOWN",
        }
    }

    pub(crate) const ALL: [Hook; 5] = [
        Hook::OnInit,
        Hook::OnBootstrap,
        Hook::OnDestroy,
        Hook::BeforeShutdown,
        Hook::OnShutdown,
    ];

    fn takes_signal(self) -> bool {
        matches!(self, Hook::BeforeShutdown | Hook::OnShutdown)
    }

    fn returns_init_result(self) -> bool {
        matches!(self, Hook::OnInit | Hook::OnBootstrap)
    }

    pub(crate) fn attr_name(self) -> &'static str {
        match self {
            Hook::OnInit => "on_module_init",
            Hook::OnBootstrap => "on_application_bootstrap",
            Hook::OnDestroy => "on_module_destroy",
            Hook::BeforeShutdown => "before_application_shutdown",
            Hook::OnShutdown => "on_application_shutdown",
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
    let flag = format_ident!("{}", hook.flag());

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
            async fn #bridge_method(&self) -> ::ulo::di::InitResult {
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

        #[doc(hidden)]
        const #flag: bool = true;
    })
}
