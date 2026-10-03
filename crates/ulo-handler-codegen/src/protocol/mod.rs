//! The hidden `__handler` protocol between `#[routes]` and a transport's handler attribute
//! (transports DESIGN §2.1, §2.5, X1, X2).
//!
//! `#[routes]` expands first. For each handler it removes the method's enhancer and `#[meta]`
//! attributes and appends, after the transport attribute,
//!
//! ```text
//! #[::ulo::__private::__handler(<name>,
//!     controller(<impl-level enhancer attrs>),
//!     method(<method-level enhancer attrs>),
//!     meta(controller(<impl-level meta exprs>), method(<method-level meta exprs>)))]
//! ```
//!
//! The transport attribute removes it with [`take_handler_attr`] and owes three items inside the
//! impl, every `<name>` the method's identifier without `r#`:
//!
//! 1. `const __ULO_KEY_<name>: &'static str = <Tr as ::ulo::Transport>::KEY;` (X1).
//! 2. `const __ULO_CHECKS_<name>: () = { .. };`, the pairwise body-consumer assertions, `()` when
//!    the handler has fewer than two parameters. `#[routes]` reads it from a free
//!    `const _: () = <Ctrl>::__ULO_CHECKS_<name>;`, which evaluates it unconditionally, or on a
//!    generic controller from `Controller::mount`, which evaluates it when `mount` is instantiated.
//! 3. `fn __ulo_mount_<name><__UloV0, ..>(m: &mut ::ulo::Mount<'_>, shared: &::ulo::__private::Shared<(Arc<__UloV0>, ..)>)`,
//!    one type parameter per impl-level `value` entry in the order written across the impl's
//!    enhancer attributes, each bounded by its role for this transport when the entry applies to
//!    it and unbounded otherwise (X2). `#[routes]`'s `Controller::mount` builds the `Shared` once
//!    and calls every mount function with it.
//!
//! [`crate::emit::MountFn`] writes all three.

pub mod enhancers;
pub mod scope;

use proc_macro2::TokenStream;
use quote::{ToTokens, quote};
use syn::ext::IdentExt;
use syn::parse::{Parse, ParseStream};
use syn::punctuated::Punctuated;
use syn::{Attribute, Expr, Ident, Token, parenthesized};

pub use enhancers::{EnhancerAttr, Entry, Form, Role};
pub use scope::{SCOPE_WORDS, ScopeArg};

/// The tokens `__handler(..)` carries for one handler.
#[derive(Clone)]
pub struct HandlerTokens {
    pub handler: Ident,
    pub controller: Vec<EnhancerAttr>,
    pub method: Vec<EnhancerAttr>,
    pub meta: MetaTokens,
}

/// The `#[meta(..)]` expressions of one handler, at both tiers, in the order written.
#[derive(Clone, Default)]
pub struct MetaTokens {
    pub controller: Vec<Expr>,
    pub method: Vec<Expr>,
}

impl HandlerTokens {
    /// The attribute `#[routes]` appends: `#[::ulo::__private::__handler(..)]`.
    pub fn to_attribute(&self) -> TokenStream {
        let tokens = self;
        quote! {
            #[::ulo::__private::__handler(#tokens)]
        }
    }

    /// The handler's name as the generated items spell it: the identifier without `r#`.
    pub fn name(&self) -> String {
        self.handler.unraw().to_string()
    }
}

impl ToTokens for HandlerTokens {
    fn to_tokens(&self, tokens: &mut TokenStream) {
        let handler = &self.handler;
        let controller = &self.controller;
        let method = &self.method;
        let meta_controller = &self.meta.controller;
        let meta_method = &self.meta.method;
        quote! {
            #handler,
            controller(#(#controller),*),
            method(#(#method),*),
            meta(controller(#(#meta_controller),*), method(#(#meta_method),*))
        }
        .to_tokens(tokens);
    }
}

impl Parse for HandlerTokens {
    fn parse(input: ParseStream<'_>) -> syn::Result<Self> {
        let handler = input.call(Ident::parse_any)?;
        input.parse::<Token![,]>()?;
        let controller = tier(input, "controller")?;
        input.parse::<Token![,]>()?;
        let method = tier(input, "method")?;
        input.parse::<Token![,]>()?;
        let meta = meta(input)?;
        input.parse::<Option<Token![,]>>()?;
        Ok(HandlerTokens { handler, controller, method, meta })
    }
}

fn tier(input: ParseStream<'_>, name: &str) -> syn::Result<Vec<EnhancerAttr>> {
    keyword(input, name)?;
    let content;
    parenthesized!(content in input);
    Ok(Punctuated::<EnhancerAttr, Token![,]>::parse_terminated(&content)?.into_iter().collect())
}

/// `meta(controller(<exprs>), method(<exprs>))`.
fn meta(input: ParseStream<'_>) -> syn::Result<MetaTokens> {
    keyword(input, "meta")?;
    let content;
    parenthesized!(content in input);
    let controller = exprs(&content, "controller")?;
    content.parse::<Token![,]>()?;
    let method = exprs(&content, "method")?;
    content.parse::<Option<Token![,]>>()?;
    Ok(MetaTokens { controller, method })
}

fn exprs(input: ParseStream<'_>, name: &str) -> syn::Result<Vec<Expr>> {
    keyword(input, name)?;
    let content;
    parenthesized!(content in input);
    Ok(Punctuated::<Expr, Token![,]>::parse_terminated(&content)?.into_iter().collect())
}

fn keyword(input: ParseStream<'_>, name: &str) -> syn::Result<()> {
    let ident: Ident = input.parse()?;
    if ident != name {
        return Err(syn::Error::new(ident.span(), format!("expected `{name}(..)`")));
    }
    Ok(())
}

/// Removes the `__handler` attribute `#[routes]` appended to a handler and parses it. `None` when
/// the method carries none: its transport attribute sits outside a `#[routes]` impl, which the
/// transport attribute reports.
pub fn take_handler_attr(attrs: &mut Vec<Attribute>) -> syn::Result<Option<HandlerTokens>> {
    let Some(position) = attrs.iter().position(|attr| attr.path().segments.last().is_some_and(|s| s.ident == "__handler")) else {
        return Ok(None);
    };
    let attr = attrs.remove(position);
    attr.parse_args::<HandlerTokens>().map(Some)
}

/// The `#[meta(..)]` attributes in `attrs`, removed and parsed, their expressions in order.
pub fn take_meta(attrs: &mut Vec<Attribute>) -> syn::Result<Vec<Expr>> {
    let mut exprs = Vec::new();
    let mut errors = Vec::new();
    attrs.retain(|attr| {
        if !attr.path().segments.last().is_some_and(|s| s.ident == "meta") {
            return true;
        }
        match attr.parse_args_with(Punctuated::<Expr, Token![,]>::parse_terminated) {
            Ok(parsed) => exprs.extend(parsed),
            Err(e) => errors.push(e),
        }
        false
    });
    match crate::util::combine(errors) {
        Some(e) => Err(e),
        None => Ok(exprs),
    }
}
