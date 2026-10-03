//! The scope word, written the same way wherever a scope is declared: `#[injectable(execution)]`
//! on a type, `with(execution) = ..` on a closure.

use proc_macro2::{Span, TokenStream};
use quote::quote_spanned;
use syn::ext::IdentExt;
use syn::parse::ParseStream;
use syn::{Ident, parenthesized};

pub(crate) const SCOPE_WORDS: &str = "`singleton`, `execution` or `transient`";

/// A scope written out, with the span of the word. Writing none declares `Auto`.
#[derive(Clone, Copy)]
pub(crate) enum ScopeArg {
    Singleton(Span),
    Execution(Span),
    Transient(Span),
}

impl ScopeArg {
    pub(crate) fn from_ident(ident: &Ident) -> Option<Self> {
        match ident.unraw().to_string().as_str() {
            "singleton" => Some(ScopeArg::Singleton(ident.span())),
            "execution" => Some(ScopeArg::Execution(ident.span())),
            "transient" => Some(ScopeArg::Transient(ident.span())),
            _ => None,
        }
    }

    /// The `(<scope>)` after `with`. Anything but one scope word is an error naming the three.
    pub(crate) fn parse_parenthesized(input: ParseStream<'_>) -> syn::Result<Self> {
        let content;
        let paren = parenthesized!(content in input);
        if content.is_empty() {
            return Err(syn::Error::new(paren.span.join(), format!("`with(..)` takes a scope: {SCOPE_WORDS}")));
        }
        let word = content
            .call(Ident::parse_any)
            .map_err(|e| syn::Error::new(e.span(), format!("expected a scope: {SCOPE_WORDS}")))?;
        let scope = ScopeArg::from_ident(&word)
            .ok_or_else(|| syn::Error::new(word.span(), format!("expected a scope: {SCOPE_WORDS}")))?;
        if !content.is_empty() {
            return Err(content.error("`with(..)` takes one scope"));
        }
        Ok(scope)
    }

    /// The scope marker type, spanned at the word so a scope error points at what was written.
    pub(crate) fn path(&self) -> TokenStream {
        match *self {
            ScopeArg::Singleton(span) => quote_spanned!(span=> ::ulo::scope::Singleton),
            ScopeArg::Execution(span) => quote_spanned!(span=> ::ulo::scope::PerExecution),
            ScopeArg::Transient(span) => quote_spanned!(span=> ::ulo::scope::Transient),
        }
    }

    /// The word with its span, for re-emitting an entry `__handler` carries to the transport.
    pub(crate) fn word(&self) -> Ident {
        match *self {
            ScopeArg::Singleton(span) => Ident::new("singleton", span),
            ScopeArg::Execution(span) => Ident::new("execution", span),
            ScopeArg::Transient(span) => Ident::new("transient", span),
        }
    }
}
