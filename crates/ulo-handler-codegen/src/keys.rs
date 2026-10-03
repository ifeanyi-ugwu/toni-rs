//! Transport keys (transports DESIGN §2.1, X1): the constant each transport attribute emits per
//! handler, and the assertions `#[routes]` emits over every controller-level scoped key.
//!
//! A free `const _` after the impl is evaluated unconditionally; inside the impl a constant needs
//! a name, and a named associated constant is evaluated only where it is read, so a misspelled
//! key would compile unnoticed. Const evaluation runs after every attribute has expanded, so the
//! `__ULO_KEY_*` constants exist by then. A generic controller is the case a free constant cannot
//! name: there `#[routes]` emits a named associated constant and `Controller::mount` reads it,
//! which evaluates it when `mount` is instantiated.
//!
//! A handler behind `#[cfg]` has no `__ULO_KEY_<name>` or `__ULO_CHECKS_<name>` where its `cfg`
//! fails, so every read of either carries the handler's gates ([`crate::cfg::presence_gates`]).
//! The key assertion is a block: its first `let` reads the ungated handlers' keys, and each
//! further `let`, one per gated handler, carries that handler's gates. A handler's key counts
//! only in a build that compiles the handler, and a scoped key whose handlers are all compiled
//! out fails the assertion in that build.

use proc_macro2::{Span, TokenStream};
use quote::{quote, quote_spanned};
use syn::ext::IdentExt;
use syn::{Attribute, Generics, Ident, LitStr, Type};

use crate::paths::Paths;
use crate::protocol::EnhancerAttr;

/// `__ULO_KEY_<name>`.
pub fn key_const_ident(name: &str, span: Span) -> Ident {
    Ident::new(&format!("__ULO_KEY_{name}"), span)
}

/// `__ULO_CHECKS_<name>`.
pub fn checks_const_ident(name: &str, span: Span) -> Ident {
    Ident::new(&format!("__ULO_CHECKS_{name}"), span)
}

/// `const __ULO_KEY_<name>: &'static str = <Marker as ::ulo::Transport>::KEY;`, hidden, with
/// `non_upper_case_globals` allowed for the method name inside it.
///
/// `dead_code` is allowed too: the constant is read only when the impl carries a scoped key.
pub fn key_const(name: &str, span: Span, paths: &Paths) -> TokenStream {
    let ident = key_const_ident(name, span);
    let core = &paths.core;
    let marker = &paths.marker;
    quote_spanned! {span=>
        #[doc(hidden)]
        #[allow(non_upper_case_globals, dead_code)]
        const #ident: &'static str = <#marker as #core::Transport>::KEY;
    }
}

/// Every distinct transport key written at the controller tier, in the order first written, each
/// with the span of its first occurrence.
pub fn scoped_keys(controller: &[EnhancerAttr]) -> Vec<Ident> {
    let mut keys: Vec<Ident> = Vec::new();
    for transport in controller.iter().flat_map(|attr| &attr.entries).filter_map(|entry| entry.transport.as_ref()) {
        if !keys.iter().any(|key| key.unraw() == transport.unraw()) {
            keys.push(transport.clone());
        }
    }
    keys
}

/// A handler as the assertions read it.
pub struct GatedHandler<'a> {
    pub name: &'a Ident,
    /// The attributes that decide whether the handler is compiled, from
    /// [`crate::cfg::presence_gates`]; empty for a handler compiled in every configuration.
    pub gates: &'a [Attribute],
}

/// What `#[routes]` emits for the X1 key assertions and for the per-handler checks constants.
pub struct Assertions {
    /// Free items after the impl: per scoped key, spanned on the key,
    ///
    /// ```text
    /// const _: () = {
    ///     let __ulo_found = ::ulo::__private::key_in("htpp", &[<Ctrl>::__ULO_KEY_get, ..]);
    ///     #[cfg(feature = "rpc")]
    ///     let __ulo_found = __ulo_found || ::ulo::__private::key_in("htpp", &[<Ctrl>::__ULO_KEY_get_rpc]);
    ///     ::core::assert!(__ulo_found, "..");
    /// };
    /// ```
    ///
    /// the ungated handlers' keys in the first statement and one statement per gated handler; and
    /// `const _: () = <Ctrl>::__ULO_CHECKS_<name>;` per handler, under its gates. Empty for a
    /// generic controller.
    pub free: TokenStream,
    /// Associated items `#[routes]` adds to the impl: for a generic controller, the named
    /// `const __ULO_KEYS_CHECK_<key>: () = { .. };` per scoped key, the same block over `Self`.
    /// Empty otherwise.
    pub associated: TokenStream,
    /// Statements at the head of `Controller::mount`: for a generic controller,
    /// `let () = Self::__ULO_KEYS_CHECK_<key>;` per key and `let () = Self::__ULO_CHECKS_<name>;`
    /// per handler, under its gates. Empty otherwise.
    pub in_mount: TokenStream,
}

/// The assertions for `keys` over `handlers`, the impl's handlers. The message names the key and
/// every handler, the gated ones apart: "`htpp` is not the key of any handler's transport in this
/// impl (handlers: get; behind `#[cfg]`: get_rpc)".
///
/// A controller with any generic parameter, a lifetime included, takes the generic form: a free
/// constant cannot name its type without the parameters in scope.
pub fn assertions(self_ty: &Type, generics: &Generics, keys: &[Ident], handlers: &[GatedHandler<'_>]) -> Assertions {
    let names: Vec<String> = handlers.iter().map(|handler| handler.name.unraw().to_string()).collect();
    let listed = handler_list(handlers, &names);
    let generic = !generics.params.is_empty();
    let owner = if generic { quote!(Self) } else { quote!(<#self_ty>) };
    let found = Ident::new("__ulo_found", Span::mixed_site());

    let mut free = TokenStream::new();
    let mut associated = TokenStream::new();
    let mut in_mount = TokenStream::new();
    for key in keys {
        let text = key.unraw().to_string();
        let literal = LitStr::new(&text, key.span());
        let message = format!("`{text}` is not the key of any handler's transport in this impl ({listed})");
        let mut ungated = Vec::new();
        let mut gated = Vec::new();
        for (handler, name) in handlers.iter().zip(&names) {
            let key_const = key_const_ident(name, key.span());
            let read = quote_spanned!(key.span()=> #owner::#key_const);
            if handler.gates.is_empty() {
                ungated.push(read);
            } else {
                let gates = handler.gates;
                gated.push(quote_spanned! {key.span()=>
                    #(#gates)*
                    let #found = #found || ::ulo::__private::key_in(#literal, &[#read]);
                });
            }
        }
        let check = quote_spanned! {key.span()=>
            {
                let #found = ::ulo::__private::key_in(#literal, &[#(#ungated),*]);
                #(#gated)*
                ::core::assert!(#found, #message);
            }
        };
        if generic {
            let ident = Ident::new(&format!("__ULO_KEYS_CHECK_{text}"), key.span());
            associated.extend(quote_spanned! {key.span()=>
                #[doc(hidden)]
                #[allow(non_upper_case_globals)]
                const #ident: () = #check;
            });
            in_mount.extend(quote_spanned! {key.span()=>
                let () = Self::#ident;
            });
        } else {
            free.extend(quote_spanned! {key.span()=>
                const _: () = #check;
            });
        }
    }
    for (handler, name) in handlers.iter().zip(&names) {
        let span = handler.name.span();
        let checks = checks_const_ident(name, span);
        let gates = handler.gates;
        if generic {
            in_mount.extend(quote_spanned! {span=>
                #(#gates)*
                let () = Self::#checks;
            });
        } else {
            free.extend(quote_spanned! {span=>
                #(#gates)*
                const _: () = #owner::#checks;
            });
        }
    }
    Assertions { free, associated, in_mount }
}

/// "handlers: get, get_rpc", or "no handlers"; gated handlers are listed after "behind `#[cfg]`:",
/// since a build that fails the assertion may have compiled them out.
fn handler_list(handlers: &[GatedHandler<'_>], names: &[String]) -> String {
    let mut ungated = Vec::new();
    let mut gated = Vec::new();
    for (handler, name) in handlers.iter().zip(names) {
        if handler.gates.is_empty() {
            ungated.push(name.as_str());
        } else {
            gated.push(name.as_str());
        }
    }
    match (ungated.is_empty(), gated.is_empty()) {
        (true, true) => "no handlers".to_owned(),
        (false, true) => format!("handlers: {}", ungated.join(", ")),
        (true, false) => format!("handlers behind `#[cfg]`: {}", gated.join(", ")),
        (false, false) => format!("handlers: {}; behind `#[cfg]`: {}", ungated.join(", "), gated.join(", ")),
    }
}
