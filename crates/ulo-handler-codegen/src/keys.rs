//! Transport keys (transports DESIGN §2.1, X1): the constant each transport attribute emits per
//! handler, and the assertions `#[routes]` emits over every controller-level scoped key.
//!
//! A free `const _` after the impl is evaluated unconditionally; inside the impl a constant needs
//! a name, and a named associated constant is evaluated only where it is read, so a misspelled
//! key would compile unnoticed. Const evaluation runs after every attribute has expanded, so the
//! `__ULO_KEY_*` constants exist by then. A generic controller is the case a free constant cannot
//! name: there `#[routes]` emits a named associated constant and `Controller::mount` reads it,
//! which evaluates it when `mount` is instantiated.

use proc_macro2::{Span, TokenStream};
use quote::{quote, quote_spanned};
use syn::ext::IdentExt;
use syn::{Generics, Ident, LitStr, Type};

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

/// What `#[routes]` emits for the X1 key assertions and for the per-handler checks constants.
pub struct Assertions {
    /// Free items after the impl: `const _: () = assert!(::ulo::__private::key_in("htpp", &[Ctrl::__ULO_KEY_get, ..]), "..");`
    /// per scoped key, spanned on the key, and `const _: () = <Ctrl>::__ULO_CHECKS_<name>;` per
    /// handler. Empty for a generic controller.
    pub free: TokenStream,
    /// Associated items `#[routes]` adds to the impl: for a generic controller, the named
    /// `const __ULO_KEYS_CHECK_<key>: () = assert!(..);` per scoped key. Empty otherwise.
    pub associated: TokenStream,
    /// Statements at the head of `Controller::mount`: for a generic controller,
    /// `let () = Self::__ULO_KEYS_CHECK_<key>;` and `let () = Self::__ULO_CHECKS_<name>;`.
    /// Empty otherwise.
    pub in_mount: TokenStream,
}

/// The assertions for `keys` over `handlers`, the handler identifiers of the impl. The message
/// names the key and every handler: "`htpp` is not the key of any handler's transport in this
/// impl (handlers: get, get_rpc)".
///
/// A controller with any generic parameter, a lifetime included, takes the generic form: a free
/// constant cannot name its type without the parameters in scope.
pub fn assertions(self_ty: &Type, generics: &Generics, keys: &[Ident], handlers: &[Ident]) -> Assertions {
    let names: Vec<String> = handlers.iter().map(|handler| handler.unraw().to_string()).collect();
    let listed = if names.is_empty() { "no handlers".to_owned() } else { format!("handlers: {}", names.join(", ")) };
    let generic = !generics.params.is_empty();
    let owner = if generic { quote!(Self) } else { quote!(<#self_ty>) };

    let mut free = TokenStream::new();
    let mut associated = TokenStream::new();
    let mut in_mount = TokenStream::new();
    for key in keys {
        let text = key.unraw().to_string();
        let literal = LitStr::new(&text, key.span());
        let message = format!("`{text}` is not the key of any handler's transport in this impl ({listed})");
        let reads: Vec<TokenStream> = names
            .iter()
            .map(|name| {
                let key_const = key_const_ident(name, key.span());
                quote_spanned!(key.span()=> #owner::#key_const)
            })
            .collect();
        let check = quote_spanned! {key.span()=>
            ::core::assert!(::ulo::__private::key_in(#literal, &[#(#reads),*]), #message)
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
        let checks = checks_const_ident(name, handler.span());
        if generic {
            in_mount.extend(quote_spanned! {handler.span()=>
                let () = Self::#checks;
            });
        } else {
            free.extend(quote_spanned! {handler.span()=>
                const _: () = #owner::#checks;
            });
        }
    }
    Assertions { free, associated, in_mount }
}
