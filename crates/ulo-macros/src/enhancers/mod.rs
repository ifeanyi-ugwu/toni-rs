//! The enhancer attributes and the hidden protocol that carries them to a transport (§7).
//!
//! Grammar of `#[guards(..)]`, `#[interceptors(..)]` and `#[error_handlers(..)]`, a comma list of:
//! - `Type`: by type, resolved from the container; `spec.guard::<Type>()`.
//! - `key = Type`: by type, applied to handlers of the transport whose scope key is `key`
//!   (`http`, `rpc`, `grpc`, `ws`, as each transport crate declares).
//! - `value = expr`: by value, built once per handler it applies to and shared by that
//!   handler's calls; `spec.guard_value(expr)`.
//! - `with = |site: Ty, ..| expr`: by closure, built per execution. The closure's body is
//!   wrapped as `async move { body }` and handed to `spec.guard_with(..)`, whose parameters are
//!   sites. A closure that is already `async`, or whose body is already an `async` block, is
//!   handed over as written.
//! - `key(value = expr)` and `key(with = ..)`: the last two, transport-scoped.
//!
//! On a method, a transport-scoped entry for another transport than the handler's applies to
//! nothing and is an error.
//!
//! Every entry is registered through a local fn named after the handler whose bound is the role
//! for the handler's transport, so a missing role reads "`AuthGuard` is not a guard for `Rpc`",
//! points at the entry, and notes "required by a bound in `get_rpc`" at the handler.

use proc_macro2::{Span, TokenStream};
use quote::{ToTokens, quote, quote_spanned};
use syn::ext::IdentExt;
use syn::parse::{Parse, ParseStream};
use syn::punctuated::Punctuated;
use syn::spanned::Spanned;
use syn::{Expr, ExprClosure, Ident, LitStr, ReturnType, Token, Type, parenthesized, token};

use crate::shared::{check_factory_params, ulo, ulo_at};

/// Which role an attribute declares.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum Role {
    Guard,
    Interceptor,
    ErrorHandler,
}

impl Role {
    pub(crate) fn of_attr(name: &str) -> Option<Role> {
        match name {
            "guards" => Some(Role::Guard),
            "interceptors" => Some(Role::Interceptor),
            "error_handlers" => Some(Role::ErrorHandler),
            _ => None,
        }
    }

    pub(crate) fn attr_name(self) -> &'static str {
        match self {
            Role::Guard => "guards",
            Role::Interceptor => "interceptors",
            Role::ErrorHandler => "error_handlers",
        }
    }

    /// The role trait's path, spanned at `span`: the handler, where the "required by a bound"
    /// note points.
    fn role_trait(self, span: Span) -> TokenStream {
        let ulo = ulo_at(span);
        match self {
            Role::Guard => quote_spanned!(span=> #ulo::Guard),
            Role::Interceptor => quote_spanned!(span=> #ulo::Interceptor),
            Role::ErrorHandler => quote_spanned!(span=> #ulo::ErrorHandler),
        }
    }

    /// The `EnhancerSpec` methods for the by-type, by-value and by-closure forms.
    fn spec_methods(self) -> (Ident, Ident, Ident) {
        let (by_type, by_value, by_closure) = match self {
            Role::Guard => ("guard", "guard_value", "guard_with"),
            Role::Interceptor => ("interceptor", "interceptor_value", "interceptor_with"),
            Role::ErrorHandler => ("error_handler", "error_handler_value", "error_handler_with"),
        };
        let ident = |name: &str| Ident::new(name, Span::call_site());
        (ident(by_type), ident(by_value), ident(by_closure))
    }
}

/// One attribute's entries, with its role.
#[derive(Clone)]
pub(crate) struct EnhancerAttr {
    pub(crate) role: Role,
    pub(crate) entries: Vec<Entry>,
    pub(crate) span: Span,
}

#[derive(Clone)]
pub(crate) struct Entry {
    /// The transport scope key, for the transport-scoped forms.
    pub(crate) scope: Option<Ident>,
    pub(crate) form: Form,
}

#[derive(Clone)]
pub(crate) enum Form {
    Type(Type),
    Value(Expr),
    With(ExprClosure),
}

impl Form {
    fn span(&self) -> Span {
        match self {
            Form::Type(ty) => ty.span(),
            Form::Value(expr) => expr.span(),
            Form::With(closure) => closure.span(),
        }
    }
}

impl Parse for Entry {
    fn parse(input: ParseStream<'_>) -> syn::Result<Self> {
        if input.peek(Ident) && input.peek2(Token![=]) && !input.peek2(Token![==]) && !input.peek2(Token![=>]) {
            let key: Ident = input.parse()?;
            input.parse::<Token![=]>()?;
            return Ok(if key == "value" {
                Entry { scope: None, form: Form::Value(input.parse()?) }
            } else if key == "with" {
                Entry { scope: None, form: Form::With(parse_closure(input)?) }
            } else {
                Entry { scope: Some(key), form: Form::Type(input.parse()?) }
            });
        }
        if input.peek(Ident) && input.peek2(token::Paren) {
            let key: Ident = input.parse()?;
            if key == "value" || key == "with" {
                return Err(syn::Error::new(key.span(), format!("write `{key} = ..`, or `<transport>({key} = ..)` to scope it")));
            }
            let content;
            parenthesized!(content in input);
            let inner: Ident = content.parse()?;
            content.parse::<Token![=]>()?;
            let form = if inner == "value" {
                Form::Value(content.parse()?)
            } else if inner == "with" {
                Form::With(parse_closure(&content)?)
            } else {
                return Err(syn::Error::new(
                    inner.span(),
                    format!("expected `{key}(value = ..)` or `{key}(with = ..)`; a type scoped to `{key}` is written `{key} = Type`"),
                ));
            };
            if !content.is_empty() {
                return Err(content.error("one entry per transport-scoped form"));
            }
            return Ok(Entry { scope: Some(key), form });
        }
        Ok(Entry { scope: None, form: Form::Type(input.parse()?) })
    }
}

fn parse_closure(input: ParseStream<'_>) -> syn::Result<ExprClosure> {
    match input.parse::<Expr>()? {
        Expr::Closure(closure) => {
            check_factory_params(&closure)?;
            Ok(closure)
        }
        other => Err(syn::Error::new_spanned(
            other,
            "`with` takes a closure whose parameters are sites, as in `with = |u: Ext<CurrentUser>| RoleGuard::require(u)`",
        )),
    }
}

impl ToTokens for Entry {
    fn to_tokens(&self, tokens: &mut TokenStream) {
        let inner = match &self.form {
            Form::Type(ty) => quote!(#ty),
            Form::Value(expr) => quote!(value = #expr),
            Form::With(closure) => quote!(with = #closure),
        };
        match (&self.scope, &self.form) {
            (None, _) => inner.to_tokens(tokens),
            (Some(key), Form::Type(ty)) => quote!(#key = #ty).to_tokens(tokens),
            (Some(key), _) => quote!(#key(#inner)).to_tokens(tokens),
        }
    }
}

impl EnhancerAttr {
    pub(crate) fn from_attr(role: Role, attr: &syn::Attribute) -> syn::Result<Self> {
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

/// `#[guards]` and its kin reached as attribute macros: outside a `#[routes]` impl, where
/// nothing consumed them, or above `#[routes]`, which expands after them.
pub(crate) fn marker(name: &str, _attr: TokenStream, _item: TokenStream) -> syn::Result<TokenStream> {
    Err(syn::Error::new(
        Span::call_site(),
        format!(
            "#[{name}] goes below #[routes] on a #[routes] impl block, or on one of its handler methods; \
             #[routes] reads it, and an attribute written above #[routes] expands before it"
        ),
    ))
}

/// `__handler` reached as an attribute macro: the method has no transport attribute that
/// consumed it.
pub(crate) fn unconsumed_handler(_attr: TokenStream, _item: TokenStream) -> syn::Result<TokenStream> {
    Err(syn::Error::new(
        Span::call_site(),
        "this handler's transport attribute did not read its enhancers; the transport crate may not support #[routes]",
    ))
}

/// The tokens `__handler(..)` carries for one handler.
pub(crate) struct HandlerTokens {
    pub(crate) handler: Ident,
    pub(crate) controller: Vec<EnhancerAttr>,
    pub(crate) method: Vec<EnhancerAttr>,
}

impl HandlerTokens {
    /// The attribute `#[routes]` appends: `#[::ulo::__private::__handler(name, controller(..), method(..))]`.
    pub(crate) fn to_attribute(&self) -> TokenStream {
        let ulo = ulo();
        let handler = &self.handler;
        let controller = &self.controller;
        let method = &self.method;
        quote! {
            #[#ulo::__private::__handler(#handler, controller(#(#controller),*), method(#(#method),*))]
        }
    }
}

impl Parse for HandlerTokens {
    fn parse(input: ParseStream<'_>) -> syn::Result<Self> {
        let handler = input.call(Ident::parse_any)?;
        input.parse::<Token![,]>()?;
        let controller = tier(input, "controller")?;
        input.parse::<Token![,]>()?;
        let method = tier(input, "method")?;
        input.parse::<Option<Token![,]>>()?;
        Ok(HandlerTokens { handler, controller, method })
    }
}

fn tier(input: ParseStream<'_>, name: &str) -> syn::Result<Vec<EnhancerAttr>> {
    let ident: Ident = input.parse()?;
    if ident != name {
        return Err(syn::Error::new(ident.span(), format!("expected `{name}(..)`")));
    }
    let content;
    parenthesized!(content in input);
    Ok(Punctuated::<EnhancerAttr, Token![,]>::parse_terminated(&content)?.into_iter().collect())
}

/// `<Transport type>, "<scope key>", <__handler tokens>`.
struct SpecsInput {
    transport: Type,
    key: LitStr,
    tokens: HandlerTokens,
}

impl Parse for SpecsInput {
    fn parse(input: ParseStream<'_>) -> syn::Result<Self> {
        let transport = input.parse()?;
        input.parse::<Token![,]>()?;
        let key = input.parse()?;
        input.parse::<Token![,]>()?;
        let tokens = input.parse()?;
        Ok(SpecsInput { transport, key, tokens })
    }
}

#[derive(Clone, Copy, PartialEq, Eq)]
enum Tier {
    Controller,
    Method,
}

/// `__enhancer_specs!(<Transport type>, "<scope key>", <__handler tokens>)`: an expression
/// evaluating to `(EnhancerSpec<Transport>, EnhancerSpec<Transport>)`, controller tier then
/// method tier, keeping only the entries unscoped or scoped to this key, with the role
/// assertions.
///
/// Each tier is built in a local and returned by value: the `EnhancerSpec` methods take and
/// return `&mut Self`, so a chain on a temporary would not outlive the statement.
pub(crate) fn specs(input: TokenStream) -> syn::Result<TokenStream> {
    let SpecsInput { transport, key, tokens } = syn::parse2(input)?;
    let ulo = ulo();
    let key = key.value();
    let mut counter = 0usize;
    let controller_var = Ident::new("__ulo_controller", Span::mixed_site());
    let method_var = Ident::new("__ulo_method", Span::mixed_site());
    let controller = tier_statements(Tier::Controller, &tokens, &tokens.controller, &key, &transport, &controller_var, &mut counter)?;
    let method = tier_statements(Tier::Method, &tokens, &tokens.method, &key, &transport, &method_var, &mut counter)?;
    let controller_mut = (!controller.is_empty()).then(|| quote!(mut));
    let method_mut = (!method.is_empty()).then(|| quote!(mut));
    Ok(quote! {
        {
            let #controller_mut #controller_var = #ulo::EnhancerSpec::<#transport>::new();
            #(#controller)*
            let #method_mut #method_var = #ulo::EnhancerSpec::<#transport>::new();
            #(#method)*
            (#controller_var, #method_var)
        }
    })
}

fn tier_statements(
    tier: Tier,
    tokens: &HandlerTokens,
    attrs: &[EnhancerAttr],
    key: &str,
    transport: &Type,
    var: &Ident,
    counter: &mut usize,
) -> syn::Result<Vec<TokenStream>> {
    let mut statements = Vec::new();
    for attr in attrs {
        for entry in &attr.entries {
            if let Some(scope) = &entry.scope {
                if scope.unraw() != key {
                    if tier == Tier::Method {
                        return Err(syn::Error::new(
                            scope.span(),
                            format!(
                                "`{handler}` is a `{key}` handler, so an entry scoped to `{scope}` applies to nothing; \
                                 write it unscoped, or on the handler it belongs to",
                                handler = tokens.handler.unraw(),
                            ),
                        ));
                    }
                    continue;
                }
            }
            statements.push(entry_statement(attr.role, &entry.form, &tokens.handler, transport, var, *counter));
            *counter += 1;
        }
    }
    Ok(statements)
}

/// One entry, registered through a local fn named after the handler. The fn's definition is
/// spanned at the handler and its call at the entry: an entry lacking the role for this
/// transport fails at the entry, and the "required by a bound" note names and points at the
/// handler. A value or closure is bound outside the block holding that fn, so a free fn the
/// expression calls by the handler's name is not shadowed by it.
fn entry_statement(role: Role, form: &Form, handler: &Ident, transport: &Type, var: &Ident, index: usize) -> TokenStream {
    let handler_span = handler.span();
    let ulo = ulo_at(handler_span);
    let role_trait = role.role_trait(handler_span);
    let (by_type, by_value, by_closure) = role.spec_methods();
    let entry_span = form.span();
    match form {
        Form::Type(ty) => {
            let def = quote_spanned! {handler_span=>
                fn #handler<__UloE: #role_trait<#transport>>(spec: &mut #ulo::EnhancerSpec<#transport>) {
                    spec.#by_type::<__UloE>();
                }
            };
            let call = quote_spanned! {entry_span=>
                #handler::<#ty>(&mut #var);
            };
            quote!({ #def #call })
        }
        Form::Value(expr) => {
            let value = Ident::new(&format!("__ulo_enhancer_{index}"), entry_span);
            let def = quote_spanned! {handler_span=>
                fn #handler<__UloE: #role_trait<#transport>>(spec: &mut #ulo::EnhancerSpec<#transport>, value: __UloE) {
                    spec.#by_value(value);
                }
            };
            quote_spanned! {entry_span=>
                let #value = #expr;
                { #def #handler(&mut #var, #value); }
            }
        }
        Form::With(closure) => {
            let build = Ident::new(&format!("__ulo_enhancer_{index}"), entry_span);
            let closure = wrap_async(closure);
            let def = quote_spanned! {handler_span=>
                fn #handler<__UloA, __UloF>(spec: &mut #ulo::EnhancerSpec<#transport>, build: __UloF)
                where
                    __UloF: #ulo::Factory<__UloA>,
                    <__UloF as #ulo::Factory<__UloA>>::Output: #role_trait<#transport>,
                {
                    spec.#by_closure::<__UloA, __UloF>(build);
                }
            };
            quote_spanned! {entry_span=>
                let #build = #closure;
                { #def #handler(&mut #var, #build); }
            }
        }
    }
}

/// `|u: Ext<CurrentUser>| RoleGuard::require(u)` becomes `|u: Ext<CurrentUser>| async move {
/// RoleGuard::require(u) }`. An explicit return type moves onto a binding inside the block,
/// since an `async` block cannot carry one.
fn wrap_async(closure: &ExprClosure) -> ExprClosure {
    if closure.asyncness.is_some() || matches!(*closure.body, Expr::Async(_)) {
        return closure.clone();
    }
    let mut wrapped = closure.clone();
    let body = &closure.body;
    let new_body: Expr = match &closure.output {
        ReturnType::Default => syn::parse_quote_spanned! {body.span()=> async move { #body } },
        ReturnType::Type(_, ty) => syn::parse_quote_spanned! {body.span()=>
            async move {
                let __ulo_built: #ty = #body;
                __ulo_built
            }
        },
    };
    wrapped.output = ReturnType::Default;
    wrapped.body = Box::new(new_body);
    wrapped
}
