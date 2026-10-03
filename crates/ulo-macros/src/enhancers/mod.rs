//! The enhancer attributes reached outside `#[routes]`, and `__enhancer_specs!`, which a
//! transport's mount function calls to build one handler's two `EnhancerSpec` tiers (§7,
//! transports DESIGN §2.1).
//!
//! The grammar of `#[guards(..)]`, `#[interceptors(..)]` and `#[error_handlers(..)]` and of the
//! `__handler` tokens lives in `ulo_handler_codegen::protocol`, read the same way by `#[routes]`
//! and every transport attribute.
//!
//! How an enhancer is built (type, value, closure) and how long it lives are separate. `Auto`
//! builds a closure's enhancer once and shares it, unless what the closure reads needs an
//! execution; then it is built per execution. Inference reads the closure's parameters, not its
//! body, so a closure that creates per-call state while reading nothing per call, such as
//! `|| RequestTimer::start()`, is written `with(execution)`.
//!
//! An impl-level `value` entry is built once by `Controller::mount` and reaches each handler
//! through the mount function's `shared` parameter, registered with `*_arc` (X2); a method-level
//! one is built once when its handler mounts. On a method, a transport-scoped entry for another
//! transport than the handler's applies to nothing and is an error.
//!
//! Every by-type and by-closure entry is registered through a local fn named after the handler
//! whose bound is the role for the handler's transport, so a missing role reads "`AuthGuard` is
//! not a guard for `Rpc`", points at the entry, and notes "required by a bound in `get_rpc`" at
//! the handler. An impl-level value's role is the mount function's bound instead, checked at
//! `Controller::mount`'s call of it.

use proc_macro2::{Span, TokenStream};
use quote::{quote, quote_spanned};
use syn::ext::IdentExt;
use syn::parse::{Parse, ParseStream};
use syn::spanned::Spanned;
use syn::{Expr, ExprClosure, Ident, Index, LitStr, ReturnType, Token, Type};
use ulo_handler_codegen::protocol::{EnhancerAttr, Form, HandlerTokens, Role};

use crate::shared::{ulo, ulo_at};

/// The role trait's path, spanned at `span`: the handler, where the "required by a bound" note
/// points.
fn role_trait(role: Role, span: Span) -> TokenStream {
    let ulo = ulo_at(span);
    let name = role.trait_ident(span);
    quote_spanned!(span=> #ulo::#name)
}

/// The `EnhancerSpec` methods for the by-type, by-value and by-closure forms, the last as `Auto`
/// and in a scope written out.
fn spec_methods(role: Role) -> SpecMethods {
    let [by_type, by_value, by_closure, by_closure_in] = match role {
        Role::Guard => ["guard", "guard_value", "guard_with", "guard_with_in"],
        Role::Interceptor => ["interceptor", "interceptor_value", "interceptor_with", "interceptor_with_in"],
        Role::ErrorHandler => ["error_handler", "error_handler_value", "error_handler_with", "error_handler_with_in"],
    };
    let ident = |name: &str| Ident::new(name, Span::call_site());
    SpecMethods { by_type: ident(by_type), by_value: ident(by_value), by_closure: ident(by_closure), by_closure_in: ident(by_closure_in) }
}

struct SpecMethods {
    by_type: Ident,
    by_value: Ident,
    by_closure: Ident,
    by_closure_in: Ident,
}

/// `#[meta]` reached as an attribute macro: outside a `#[routes]` impl, or above `#[routes]`.
/// `#[routes]` takes it from the impl and from every method, inside `cfg_attr` or not, so one
/// written below `#[routes]` never reaches here.
pub(crate) fn meta_marker(_attr: TokenStream, _item: TokenStream) -> syn::Result<TokenStream> {
    Err(unread("meta"))
}

/// `#[guards]` and its kin reached as attribute macros, under the same conditions as `#[meta]`.
pub(crate) fn marker(name: &str, _attr: TokenStream, _item: TokenStream) -> syn::Result<TokenStream> {
    Err(unread(name))
}

fn unread(name: &str) -> syn::Error {
    syn::Error::new(
        Span::call_site(),
        format!(
            "#[{name}] is read by #[routes]: it goes on a #[routes] impl block, below #[routes], or on a method of one; \
             no #[routes] read this one, so it is outside a #[routes] impl or above #[routes], which expands after it"
        ),
    )
}

/// `__handler` reached as an attribute macro: no transport attribute on the method consumed it.
pub(crate) fn unconsumed_handler(_attr: TokenStream, _item: TokenStream) -> syn::Result<TokenStream> {
    Err(syn::Error::new(
        Span::call_site(),
        "no transport attribute read this handler's enhancers: #[routes] took this method for a handler because of an \
         attribute macro on it, read through `cfg_attr` (one outside any `cfg_attr` counts in every build, otherwise the \
         first inside one decides), and no transport attribute supporting #[routes] is present in this build; \
         a helper goes in a separate impl block",
    ))
}

/// `<Transport type>, "<transport key>", <shared ident>, <__handler tokens>`.
struct SpecsInput {
    transport: Type,
    key: LitStr,
    shared: Ident,
    tokens: HandlerTokens,
}

impl Parse for SpecsInput {
    fn parse(input: ParseStream<'_>) -> syn::Result<Self> {
        let transport = input.parse()?;
        input.parse::<Token![,]>()?;
        let key = input.parse()?;
        input.parse::<Token![,]>()?;
        let shared = input.parse()?;
        input.parse::<Token![,]>()?;
        let tokens = input.parse()?;
        Ok(SpecsInput { transport, key, shared, tokens })
    }
}

#[derive(Clone, Copy, PartialEq, Eq)]
enum Tier {
    Controller,
    Method,
}

/// `__enhancer_specs!(<Transport type>, "<transport key>", <shared ident>, <__handler tokens>)`:
/// an expression evaluating to `(EnhancerSpec<Transport>, EnhancerSpec<Transport>)`, controller
/// tier then method tier, keeping only the entries for every transport or for this key, with the
/// role assertions. `<shared ident>` names the mount function's `&Shared<..>` parameter, which
/// holds the impl-level values in the order `ulo_handler_codegen::shared::shared_values` gives.
///
/// Each tier is built in a local and returned by value: the `EnhancerSpec` methods take and
/// return `&mut Self`, so a chain on a temporary would not outlive the statement.
pub(crate) fn specs(input: TokenStream) -> syn::Result<TokenStream> {
    let SpecsInput { transport, key, shared, tokens } = syn::parse2(input)?;
    let ulo = ulo();
    let key = key.value();
    let mut counter = 0usize;
    let controller_var = Ident::new("__ulo_controller", Span::mixed_site());
    let method_var = Ident::new("__ulo_method", Span::mixed_site());
    let at = Site { tokens: &tokens, key: &key, transport: &transport, shared: &shared };
    let controller = tier_statements(Tier::Controller, &at, &tokens.controller, &controller_var, &mut counter)?;
    let method = tier_statements(Tier::Method, &at, &tokens.method, &method_var, &mut counter)?;
    let controller_mut = binding_mut(&controller);
    let method_mut = binding_mut(&method);
    let [controller, method] = [controller, method].map(|statements| statements.into_iter().map(|(tokens, _)| tokens));
    Ok(quote! {
        {
            #controller_mut #controller_var = #ulo::EnhancerSpec::<#transport>::new();
            #(#controller)*
            #method_mut #method_var = #ulo::EnhancerSpec::<#transport>::new();
            #(#method)*
            (#controller_var, #method_var)
        }
    })
}

/// `let` when no statement registers into the binding, `let mut` when one does in every build,
/// and `let mut` with `unused_mut` allowed when every one is gated, since a build compiling them
/// all out never mutates it.
fn binding_mut(statements: &[(TokenStream, bool)]) -> TokenStream {
    if statements.is_empty() {
        quote!(let)
    } else if statements.iter().all(|(_, gated)| *gated) {
        quote!(#[allow(unused_mut)] let mut)
    } else {
        quote!(let mut)
    }
}

/// What every entry of one handler is registered against.
struct Site<'a> {
    tokens: &'a HandlerTokens,
    key: &'a str,
    transport: &'a Type,
    shared: &'a Ident,
}

/// Each applying entry's statement, under its attribute's gates, with whether it has any.
fn tier_statements(
    tier: Tier,
    at: &Site<'_>,
    attrs: &[EnhancerAttr],
    var: &Ident,
    counter: &mut usize,
) -> syn::Result<Vec<(TokenStream, bool)>> {
    let mut statements = Vec::new();
    // Counts every impl-level value, those scoped to other transports included: the position in
    // `Shared` is the same for every handler of the impl.
    let mut shared_index = 0usize;
    for attr in attrs {
        for entry in &attr.entries {
            let position = shared_index;
            if tier == Tier::Controller && matches!(entry.form, Form::Value(_)) {
                shared_index += 1;
            }
            if !entry.applies_to(at.key) {
                if tier == Tier::Method {
                    let transport = entry.transport.as_ref().map(|t| t.unraw().to_string()).unwrap_or_default();
                    return Err(syn::Error::new(
                        entry.transport.as_ref().map_or_else(Span::call_site, |t| t.span()),
                        format!(
                            "`{handler}` is a `{key}` handler, so an entry for `{transport}` applies to nothing; \
                             write it for every transport, or on the handler it belongs to",
                            handler = at.tokens.handler.unraw(),
                            key = at.key,
                        ),
                    ));
                }
                continue;
            }
            let statement = match (&entry.form, tier) {
                (Form::Value(expr), Tier::Controller) => shared_statement(attr.role, expr, at.shared, var, position),
                (form, _) => entry_statement(attr.role, form, &at.tokens.handler, at.transport, var, *counter),
            };
            let gates = &attr.gates;
            statements.push((quote!(#(#gates)* #statement), !gates.is_empty()));
            *counter += 1;
        }
    }
    Ok(statements)
}

/// An impl-level value: a clone of its `Arc` in `Shared`, unsized into the role by `*_arc`. The
/// role bound sits on the mount function's type parameter for this position.
fn shared_statement(role: Role, expr: &Expr, shared: &Ident, var: &Ident, position: usize) -> TokenStream {
    let ulo = ulo();
    let method = role.arc_method();
    // Both fields are interpolated: `.0.` written in the template would lex `0.` as a float.
    let tuple = Index { index: 0, span: expr.span() };
    let index = Index { index: position as u32, span: expr.span() };
    quote_spanned! {expr.span()=>
        #var.#method(#ulo::__private::Arc::clone(&#shared.#tuple.#index));
    }
}

/// One entry other than an impl-level value, as one block statement, so the gates of the
/// attribute holding it cover the whole entry; registered through a local fn named after the
/// handler. The fn's definition is
/// spanned at the handler and its call at the entry: an entry lacking the role for this
/// transport fails at the entry, and the "required by a bound" note names and points at the
/// handler. A value or closure is bound outside the block holding that fn, so a free fn the
/// expression calls by the handler's name is not shadowed by it.
fn entry_statement(role: Role, form: &Form, handler: &Ident, transport: &Type, var: &Ident, index: usize) -> TokenStream {
    let handler_span = handler.span();
    let ulo = ulo_at(handler_span);
    let role_trait = role_trait(role, handler_span);
    let SpecMethods { by_type, by_value, by_closure, by_closure_in } = spec_methods(role);
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
                {
                    let #value = #expr;
                    { #def #handler(&mut #var, #value); }
                }
            }
        }
        Form::With { scope, closure } => {
            let build = Ident::new(&format!("__ulo_enhancer_{index}"), entry_span);
            let closure = wrap_async(closure);
            let register = match scope {
                None => quote_spanned!(handler_span=> spec.#by_closure::<__UloA, __UloF>(build);),
                Some(scope) => {
                    let scope = scope.path();
                    quote_spanned!(handler_span=> spec.#by_closure_in::<#scope, __UloA, __UloF>(build);)
                }
            };
            // The core records `Location::caller()` for a closure declaration. With the fn
            // `#[track_caller]` and its call spanned at the entry, callee included, that location
            // is the entry rather than the handler.
            let mut callee = handler.clone();
            callee.set_span(entry_span);
            let def = quote_spanned! {handler_span=>
                #[track_caller]
                fn #handler<__UloA, __UloF>(spec: &mut #ulo::EnhancerSpec<#transport>, build: __UloF)
                where
                    __UloF: #ulo::Factory<__UloA>,
                    <__UloF as #ulo::Factory<__UloA>>::Output: #role_trait<#transport>,
                {
                    #register
                }
            };
            quote_spanned! {entry_span=>
                {
                    let #build = #closure;
                    { #def #callee(&mut #var, #build); }
                }
            }
        }
    }
}

/// `|u: Ext<CurrentUser>| RoleGuard::require(u)` becomes `|u: Ext<CurrentUser>| async move {
/// RoleGuard::require(u) }`. An explicit return type moves onto a binding inside the block,
/// since an `async` block cannot carry one. `#[module]`'s `into` lists share it, so a `with`
/// entry is written the same way in both places.
pub(crate) fn wrap_async(closure: &ExprClosure) -> ExprClosure {
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
