//! The enhancer attributes' grammar, shared by `#[routes]`, which parses them off the impl and its
//! methods, and by every transport attribute, which reads them back out of `__handler`.
//!
//! `#[guards(..)]`, `#[interceptors(..)]` and `#[error_handlers(..)]` each take a comma list of:
//! - `Type`: by type, resolved from the container.
//! - `key = Type`: by type, for the handlers of the transport whose `Transport::KEY` is `key`.
//! - `value = expr`: by value. On a method it is built once when the handler mounts; on the impl
//!   it is built once by `Controller::mount` and shared by every handler (transports DESIGN §2.1,
//!   X2).
//! - `with = |param: Ty, ..| expr`: by closure, scope `Auto`.
//! - `with(singleton) = ..`, `with(execution) = ..`, `with(transient) = ..`: by closure, in the
//!   scope written. Any other word in the parentheses is an error naming the three.
//! - `key(value = expr)`, `key(with = ..)` and `key(with(<scope>) = ..)`: the last three,
//!   transport-scoped.
//!
//! A scope word in a transport key's place, `execution = AuthGuard`, is an error: it names no
//! transport.

use proc_macro2::{Span, TokenStream};
use quote::{ToTokens, quote};
use syn::parse::{Parse, ParseStream};
use syn::punctuated::Punctuated;
use syn::spanned::Spanned;
use syn::{Expr, ExprClosure, Ident, Token, Type, parenthesized, token};

use crate::protocol::scope::ScopeArg;
use crate::util::check_factory_params;

/// Which role an attribute declares.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Role {
    Guard,
    Interceptor,
    ErrorHandler,
}

impl Role {
    pub fn of_attr(name: &str) -> Option<Role> {
        match name {
            "guards" => Some(Role::Guard),
            "interceptors" => Some(Role::Interceptor),
            "error_handlers" => Some(Role::ErrorHandler),
            _ => None,
        }
    }

    pub fn attr_name(self) -> &'static str {
        match self {
            Role::Guard => "guards",
            Role::Interceptor => "interceptors",
            Role::ErrorHandler => "error_handlers",
        }
    }

    /// The core's role trait for this role, `Guard`, `Interceptor` or `ErrorHandler`, as an
    /// identifier the caller places under the core's path.
    pub fn trait_ident(self, span: Span) -> Ident {
        let name = match self {
            Role::Guard => "Guard",
            Role::Interceptor => "Interceptor",
            Role::ErrorHandler => "ErrorHandler",
        };
        Ident::new(name, span)
    }

    /// The `EnhancerSpec` method registering an impl-level shared value of this role.
    pub fn arc_method(self) -> Ident {
        let name = match self {
            Role::Guard => "guard_arc",
            Role::Interceptor => "interceptor_arc",
            Role::ErrorHandler => "error_handler_arc",
        };
        Ident::new(name, Span::call_site())
    }
}

/// One attribute's entries, with its role.
#[derive(Clone)]
pub struct EnhancerAttr {
    pub role: Role,
    pub entries: Vec<Entry>,
    pub span: Span,
}

#[derive(Clone)]
pub struct Entry {
    /// The transport key, for the transport-scoped forms.
    pub transport: Option<Ident>,
    /// `value` or `with` as written, so a diagnostic can span an unscoped entry from its first
    /// token.
    pub keyword: Option<Ident>,
    pub form: Form,
}

impl Entry {
    /// The entry's tokens with their written spans, so an error built from them covers the entry.
    pub fn as_written(&self) -> TokenStream {
        let lead = self.transport.as_ref().or(self.keyword.as_ref());
        let form = match &self.form {
            Form::Type(ty) => ty.to_token_stream(),
            Form::Value(expr) => expr.to_token_stream(),
            Form::With { closure, .. } => closure.to_token_stream(),
        };
        quote!(#lead #form)
    }

    /// Whether this entry applies to a handler of the transport whose key is `key`: unscoped, or
    /// scoped to `key`.
    pub fn applies_to(&self, key: &str) -> bool {
        use syn::ext::IdentExt;
        self.transport.as_ref().is_none_or(|transport| transport.unraw() == key)
    }
}

#[derive(Clone)]
pub enum Form {
    Type(Type),
    Value(Expr),
    /// `scope` is `None` for `with = ..`, which declares `Auto`.
    With { scope: Option<ScopeArg>, closure: ExprClosure },
}

impl Form {
    pub fn span(&self) -> Span {
        match self {
            Form::Type(ty) => ty.span(),
            Form::Value(expr) => expr.span(),
            Form::With { closure, .. } => closure.span(),
        }
    }
}

impl Parse for Entry {
    fn parse(input: ParseStream<'_>) -> syn::Result<Self> {
        let assigned = input.peek2(Token![=]) && !input.peek2(Token![==]) && !input.peek2(Token![=>]);
        if !(input.peek(Ident) && (assigned || input.peek2(token::Paren))) {
            return Ok(Entry { transport: None, keyword: None, form: Form::Type(input.parse()?) });
        }
        let key: Ident = input.parse()?;
        if key == "with" {
            return Ok(Entry { transport: None, form: parse_with(input)?, keyword: Some(key) });
        }
        if key == "value" {
            if input.peek(token::Paren) {
                return Err(value_takes_no_scope(&key));
            }
            input.parse::<Token![=]>()?;
            return Ok(Entry { transport: None, form: Form::Value(input.parse()?), keyword: Some(key) });
        }
        refuse_scope_word(&key)?;
        if input.peek(Token![=]) {
            input.parse::<Token![=]>()?;
            return Ok(Entry { transport: Some(key), keyword: None, form: Form::Type(input.parse()?) });
        }
        let content;
        parenthesized!(content in input);
        let inner: Ident = content.parse()?;
        let form = if inner == "with" {
            parse_with(&content)?
        } else if inner == "value" {
            if content.peek(token::Paren) {
                return Err(value_takes_no_scope(&inner));
            }
            content.parse::<Token![=]>()?;
            Form::Value(content.parse()?)
        } else {
            return Err(syn::Error::new(
                inner.span(),
                format!(
                    "expected `{key}(value = ..)` or `{key}(with = ..)`; a type for `{key}` handlers is written `{key} = Type`"
                ),
            ));
        };
        if !content.is_empty() {
            return Err(content.error("one entry per transport-scoped form"));
        }
        Ok(Entry { transport: Some(key), keyword: Some(inner), form })
    }
}

/// What follows the `with` keyword: an optional `(<scope>)`, `=`, and the closure.
fn parse_with(input: ParseStream<'_>) -> syn::Result<Form> {
    let scope = if input.peek(token::Paren) { Some(ScopeArg::parse_parenthesized(input)?) } else { None };
    input.parse::<Token![=]>()?;
    Ok(Form::With { scope, closure: parse_closure(input)? })
}

fn value_takes_no_scope(keyword: &Ident) -> syn::Error {
    syn::Error::new(
        keyword.span(),
        "a `value` is built once and shared, so it takes no scope; write `value = ..`, \
         or `<transport>(value = ..)` for one transport's handlers",
    )
}

/// `execution = AuthGuard` names no transport: refused here, since on the impl the key assertion
/// would read it as a misspelled transport key.
fn refuse_scope_word(key: &Ident) -> syn::Result<()> {
    if ScopeArg::from_ident(key).is_none() {
        return Ok(());
    }
    Err(syn::Error::new(
        key.span(),
        format!(
            "`{key}` is a scope, not a transport key; a type declares its scope on the type, \
             `#[injectable({key})]`, and a closure is written `with({key}) = ..`"
        ),
    ))
}

fn parse_closure(input: ParseStream<'_>) -> syn::Result<ExprClosure> {
    match input.parse::<Expr>()? {
        Expr::Closure(closure) => {
            check_factory_params(&closure)?;
            Ok(closure)
        }
        other => Err(syn::Error::new_spanned(
            other,
            "`with` takes a closure whose parameters are injection points, as in `with = |u: Ext<CurrentUser>| RoleGuard::require(u)`",
        )),
    }
}

impl ToTokens for Entry {
    fn to_tokens(&self, tokens: &mut TokenStream) {
        let inner = match &self.form {
            Form::Type(ty) => quote!(#ty),
            Form::Value(expr) => quote!(value = #expr),
            Form::With { scope: None, closure } => quote!(with = #closure),
            Form::With { scope: Some(scope), closure } => {
                let word = scope.word();
                quote!(with(#word) = #closure)
            }
        };
        match (&self.transport, &self.form) {
            (None, _) => inner.to_tokens(tokens),
            (Some(key), Form::Type(ty)) => quote!(#key = #ty).to_tokens(tokens),
            (Some(key), _) => quote!(#key(#inner)).to_tokens(tokens),
        }
    }
}

impl EnhancerAttr {
    /// `#[guards(..)]` and its kin as written on the impl or a method.
    pub fn from_attr(role: Role, attr: &syn::Attribute) -> syn::Result<Self> {
        let entries = attr.parse_args_with(Punctuated::<Entry, Token![,]>::parse_terminated)?;
        Ok(EnhancerAttr { role, entries: entries.into_iter().collect(), span: attr.path().span() })
    }
}

/// `guards(..)` as `__handler` carries it.
impl Parse for EnhancerAttr {
    fn parse(input: ParseStream<'_>) -> syn::Result<Self> {
        let name: Ident = input.parse()?;
        let role = Role::of_attr(&name.to_string())
            .ok_or_else(|| syn::Error::new(name.span(), "expected `guards`, `interceptors` or `error_handlers`"))?;
        let content;
        parenthesized!(content in input);
        let entries = Punctuated::<Entry, Token![,]>::parse_terminated(&content)?;
        Ok(EnhancerAttr { role, entries: entries.into_iter().collect(), span: name.span() })
    }
}

impl ToTokens for EnhancerAttr {
    fn to_tokens(&self, tokens: &mut TokenStream) {
        let name = Ident::new(self.role.attr_name(), self.span);
        let entries = &self.entries;
        quote!(#name(#(#entries),*)).to_tokens(tokens);
    }
}
