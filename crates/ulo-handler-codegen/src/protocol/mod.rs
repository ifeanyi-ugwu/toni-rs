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
//! A method-level enhancer attribute or meta expression written inside `cfg_attr` carries one
//! `#[cfg(<predicate>)]` per enclosing predicate in front of it, `method(#[cfg(feature = "x")]
//! guards(AuthGuard))`, and whatever a transport generates from it is compiled under those gates
//! ([`EnhancerAttr::gates`], [`MetaExpr::gates`]). A handler whose transport attribute sits inside
//! `cfg_attr` receives the attribute inside a `cfg_attr` with the same predicates, so it is
//! present exactly where the transport attribute is.
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
    pub controller: Vec<MetaExpr>,
    pub method: Vec<MetaExpr>,
}

/// One `#[meta(..)]` expression.
#[derive(Clone)]
pub struct MetaExpr {
    /// The `#[cfg(..)]` attributes it is compiled under, as on [`EnhancerAttr::gates`].
    pub gates: Vec<Attribute>,
    pub expr: Expr,
}

impl Parse for MetaExpr {
    fn parse(input: ParseStream<'_>) -> syn::Result<Self> {
        let gates = input.call(Attribute::parse_outer)?;
        Ok(MetaExpr { gates, expr: input.parse()? })
    }
}

impl ToTokens for MetaExpr {
    fn to_tokens(&self, tokens: &mut TokenStream) {
        let gates = &self.gates;
        let expr = &self.expr;
        quote!(#(#gates)* #expr).to_tokens(tokens);
    }
}

impl HandlerTokens {
    /// The attribute `#[routes]` appends: `#[::ulo::__private::__handler(..)]`.
    pub fn to_attribute(&self) -> TokenStream {
        let meta = self.to_meta();
        quote!(#[#meta])
    }

    /// The attribute's contents, `::ulo::__private::__handler(..)`, for `#[routes]` to place
    /// inside a `cfg_attr`.
    pub fn to_meta(&self) -> TokenStream {
        let tokens = self;
        quote!(::ulo::__private::__handler(#tokens))
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

fn exprs(input: ParseStream<'_>, name: &str) -> syn::Result<Vec<MetaExpr>> {
    keyword(input, name)?;
    let content;
    parenthesized!(content in input);
    Ok(Punctuated::<MetaExpr, Token![,]>::parse_terminated(&content)?.into_iter().collect())
}

fn keyword(input: ParseStream<'_>, name: &str) -> syn::Result<()> {
    let ident: Ident = input.parse()?;
    if ident != name {
        return Err(syn::Error::new(ident.span(), format!("expected `{name}(..)`")));
    }
    Ok(())
}

/// Removes the `__handler` attribute `#[routes]` appended to a handler and parses it. `None` when
/// the method carries none, which the transport attribute reports with [`outside_routes`].
pub fn take_handler_attr(attrs: &mut Vec<Attribute>) -> syn::Result<Option<HandlerTokens>> {
    let Some(position) = attrs.iter().position(|attr| attr.path().segments.last().is_some_and(|s| s.ident == "__handler")) else {
        return Ok(None);
    };
    let attr = attrs.remove(position);
    attr.parse_args::<HandlerTokens>().map(Some)
}

/// The `#[meta(..)]` attributes in `attrs`, removed and parsed, their expressions in order and
/// ungated.
pub fn take_meta(attrs: &mut Vec<Attribute>) -> syn::Result<Vec<MetaExpr>> {
    let mut exprs = Vec::new();
    let mut errors = Vec::new();
    attrs.retain(|attr| {
        if !is_meta(attr) {
            return true;
        }
        match meta_exprs(attr, &[]) {
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

/// Whether `attr` is `#[meta(..)]`, by its path's last segment.
pub fn is_meta(attr: &Attribute) -> bool {
    attr.path().segments.last().is_some_and(|s| s.ident == "meta")
}

/// The expressions of one `#[meta(..)]`, each compiled under `gates`.
pub fn meta_exprs(attr: &Attribute, gates: &[Attribute]) -> syn::Result<Vec<MetaExpr>> {
    let parsed = attr.parse_args_with(Punctuated::<Expr, Token![,]>::parse_terminated)?;
    Ok(parsed.into_iter().map(|expr| MetaExpr { gates: gates.to_vec(), expr }).collect())
}

/// The error a transport attribute named `attr_name` reports when [`take_handler_attr`] finds no
/// `__handler`: it sits outside a `#[routes]` impl, or inside one on a method where `#[routes]`
/// took another attribute for the transport attribute and gated the hand-off on that one's
/// `cfg_attr`.
pub fn outside_routes(attr_name: &str) -> syn::Error {
    syn::Error::new(
        proc_macro2::Span::call_site(),
        format!(
            "#[{attr_name}] goes on a method of a `#[routes]` impl, which hands it the handler's enhancers; \
             if this method is in one, `#[routes]` took another attribute for its transport attribute: \
             a method has one, and when every attribute macro on it sits inside `cfg_attr`, the first is taken"
        ),
    )
}
