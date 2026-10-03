//! One entry of a `providers` list and its lowering to the value API:
//!
//! | Entry | Lowers to |
//! |---|---|
//! | `UserService` | `m.provide::<UserService>();` |
//! | `PgUserRepo as dyn UserRepo` | `m.provide::<PgUserRepo>().also_as::<dyn UserRepo>(\|a\| a);` |
//! | `AppConfig::from_env()?` | `m.try_value(AppConfig::from_env());` |
//! | `expr` | `m.value(expr);` |
//! | a closure | `singleton` or `try_singleton`, picked by autoref over the closure's own type |
//! | `into K: [..]` | one `m.contribute::<K>()` statement per item, below |
//!
//! An `into` list has one grammar for every key, a role key and `dyn Plugin` alike, the one
//! `#[guards]` uses for a type, a value and a closure:
//!
//! | Item | Lowers to |
//! |---|---|
//! | `A` | `m.contribute::<K>().provide::<A>(\|a\| a);` |
//! | `value = expr` | `m.contribute::<K>().value(Arc::new(expr));` |
//! | `value = expr?` | `m.contribute::<K>().try_value(Result::map(expr, \|v\| -> Arc<K> { Arc::new(v) }));` |
//! | `with = closure` | `m.contribute::<K>()` then `.singleton(closure, \|a\| a)` or `.try_singleton(..)`, by the same autoref ranking |
//!
//! A `with` closure is written as in `#[guards]`: a synchronous body is wrapped in `async move`,
//! and a closure already `async` is kept. It builds a singleton, as a providers-list closure does.
//! A `value` is evaluated when `register` runs, once per module. `value = expr?` records an `Err`
//! for `wire()` to report, as a providers list's `expr?` does. Its closure's return type is
//! written because `Result<Arc<T>, E>` does not coerce to `Result<Arc<K>, E>`; the annotation
//! makes the closure's tail the coercion site where `Arc<T>` widens to `Arc<K>`.
//!
//! The core gives the enhancer role to a contribution under a role key at freeze, from the key's
//! type, so `into AnyGuard<Http>: [AuthGuard]` registers a global guard and nothing here tells a
//! role key from another key.
//!
//! A type entry is a plain type path: `Foo`, `db::Pool<Pg>`. In a providers list, a path with
//! parenthesized arguments, `Config::default()`, is a call and reads as an expression, and a bare
//! path naming a constant reads as a type, so a constant is bound by value with a block,
//! `{ LIMITS }`. In an `into` list an expression is written `value = expr`.
//!
//! An entry `X as <role key>` is an error: `as` binds a single instance, which the transports'
//! walk over a role's contributions never reaches.
//!
//! A type that does not implement `Construct` fails at `provide::<T>()`; the generated call is
//! spanned at the entry, where the error is reported.

use proc_macro2::TokenStream;
use quote::{quote, quote_spanned};
use syn::parse::{Parse, ParseStream};
use syn::punctuated::Punctuated;
use syn::spanned::Spanned;
use syn::{Expr, ExprClosure, Ident, PathArguments, Token, Type, TypeParamBound, bracketed};

use crate::enhancers::wrap_async;
use crate::module_attr::module_def_param;
use crate::shared::{check_factory_params, ulo};

pub(crate) enum ProviderEntry {
    Provide(Type),
    ProvideAs { ty: Type, as_ty: Type },
    TryValue(Expr),
    Value(Expr),
    Factory(ExprClosure),
    Contribute { into: Type, items: Vec<Contribution> },
}

/// One item of an `into K: [..]` list.
pub(crate) enum Contribution {
    Type(Type),
    Value(Expr),
    /// `value = expr?`, holding `expr` without the `?`.
    TryValue(Expr),
    With(ExprClosure),
}

const CONTRIBUTION_FORMS: &str = "an `into` entry is a type, `value = expr` or `with = |..| ..`";

impl Parse for ProviderEntry {
    /// `into T: [..]` first, then a type followed by `,`, `as` or the end, then an expression.
    fn parse(input: ParseStream<'_>) -> syn::Result<Self> {
        if is_contribution(input) {
            input.parse::<Ident>()?;
            let into: Type = input.parse()?;
            input.parse::<Token![:]>()?;
            let content;
            bracketed!(content in input);
            let items = Punctuated::<Contribution, Token![,]>::parse_terminated(&content)?.into_iter().collect();
            return Ok(ProviderEntry::Contribute { into, items });
        }

        let fork = input.fork();
        if fork.parse::<Type>().is_ok_and(|ty| is_type_entry(&ty))
            && (fork.is_empty() || fork.peek(Token![,]) || fork.peek(Token![as]))
        {
            let ty: Type = input.parse()?;
            if input.peek(Token![as]) {
                input.parse::<Token![as]>()?;
                let as_ty: Type = input.parse()?;
                if written_as_role_key(&as_ty) {
                    return Err(syn::Error::new_spanned(
                        as_ty,
                        "a role key takes contributions, and `as` binds a single instance; \
                         a global enhancer is written `into AnyGuard<Http>: [AuthGuard]`",
                    ));
                }
                return Ok(ProviderEntry::ProvideAs { ty, as_ty });
            }
            return Ok(ProviderEntry::Provide(ty));
        }

        Ok(match input.parse::<Expr>()? {
            Expr::Try(expr) => ProviderEntry::TryValue(*expr.expr),
            Expr::Closure(closure) => {
                check_factory_params(&closure)?;
                ProviderEntry::Factory(closure)
            }
            expr => ProviderEntry::Value(expr),
        })
    }
}

impl Parse for Contribution {
    /// `value = ..` and `with = ..` first, then a type followed by `,` or the end.
    fn parse(input: ParseStream<'_>) -> syn::Result<Self> {
        if input.peek(Ident) && input.peek2(Token![=]) && !input.peek2(Token![==]) && !input.peek2(Token![=>]) {
            let key: Ident = input.parse()?;
            input.parse::<Token![=]>()?;
            if key == "value" {
                return Ok(match input.parse::<Expr>()? {
                    Expr::Try(expr) => Contribution::TryValue(*expr.expr),
                    expr => Contribution::Value(expr),
                });
            }
            if key == "with" {
                return match input.parse::<Expr>()? {
                    Expr::Closure(closure) => {
                        check_factory_params(&closure)?;
                        Ok(Contribution::With(closure))
                    }
                    other => Err(syn::Error::new_spanned(
                        other,
                        "`with` takes a closure whose parameters are injection points, \
                         as in `with = |cfg: Dep<MetricsConfig>| MetricsPlugin::new(cfg)`",
                    )),
                };
            }
            return Err(syn::Error::new(key.span(), CONTRIBUTION_FORMS));
        }

        let fork = input.fork();
        if fork.parse::<Type>().is_ok_and(|ty| !is_call(&ty)) && (fork.is_empty() || fork.peek(Token![,])) {
            return Ok(Contribution::Type(input.parse()?));
        }
        let expr: Expr = input.parse()?;
        Err(syn::Error::new_spanned(expr, format!("{CONTRIBUTION_FORMS}; an expression is contributed with `value = ..`")))
    }
}

impl ProviderEntry {
    /// The statement this entry writes inside `register`, with `m` the `ModuleDef`.
    pub(crate) fn lower(&self) -> TokenStream {
        let ulo = ulo();
        let m = module_def_param();
        match self {
            ProviderEntry::Provide(ty) => quote_spanned! {ty.span()=>
                #m.provide::<#ty>();
            },
            ProviderEntry::ProvideAs { ty, as_ty } => quote_spanned! {ty.span()=>
                #m.provide::<#ty>().also_as::<#as_ty>(|a| a);
            },
            ProviderEntry::TryValue(expr) => quote_spanned! {expr.span()=>
                #m.try_value(#expr);
            },
            ProviderEntry::Value(expr) => quote_spanned! {expr.span()=>
                #m.value(#expr);
            },
            ProviderEntry::Factory(closure) => quote_spanned! {closure.span()=>
                {
                    #[allow(unused_imports)]
                    use #ulo::__private::factory::{Fallible as _, Plain as _};
                    (&#ulo::__private::factory::Probe::new(#closure)).register_singleton(&mut *#m);
                }
            },
            ProviderEntry::Contribute { into, items } => {
                let contributions = items.iter().map(|item| item.lower(into));
                quote!(#(#contributions)*)
            }
        }
    }
}

impl Contribution {
    /// The statement contributing this item to the collection `into`.
    fn lower(&self, into: &Type) -> TokenStream {
        let ulo = ulo();
        let m = module_def_param();
        match self {
            Contribution::Type(ty) => quote_spanned! {ty.span()=>
                #m.contribute::<#into>().provide::<#ty>(|a| a);
            },
            Contribution::Value(expr) => quote_spanned! {expr.span()=>
                #m.contribute::<#into>().value(#ulo::__private::Arc::new(#expr));
            },
            Contribution::TryValue(expr) => quote_spanned! {expr.span()=>
                #m.contribute::<#into>().try_value(::core::result::Result::map(
                    #expr,
                    |v| -> #ulo::__private::Arc<#into> { #ulo::__private::Arc::new(v) },
                ));
            },
            Contribution::With(closure) => {
                let closure = wrap_async(closure);
                quote_spanned! {closure.span()=>
                    {
                        #[allow(unused_imports)]
                        use #ulo::__private::factory::{FallibleContribution as _, PlainContribution as _};
                        (&#ulo::__private::factory::Probe::new(#closure))
                            .contribute_singleton::<#into, _>(&mut *#m, |a| a);
                    }
                }
            }
        }
    }
}

/// Whether `ty` is spelled like one of the role keys: a path ending in `AnyGuard`,
/// `AnyInterceptor` or `AnyErrorHandler`, or `dyn` of the `Erased*` trait each aliases. Read for
/// the `as` diagnostic alone; no lowering depends on it. An alias under another name passes
/// unreported.
fn written_as_role_key(ty: &Type) -> bool {
    const ALIASES: &[&str] = &["AnyGuard", "AnyInterceptor", "AnyErrorHandler"];
    const TWINS: &[&str] = &["ErasedGuard", "ErasedInterceptor", "ErasedErrorHandler"];
    let ends_in = |path: &syn::Path, names: &[&str]| {
        path.segments.last().is_some_and(|segment| names.iter().any(|name| segment.ident == *name))
    };
    match ty {
        Type::Group(group) => written_as_role_key(&group.elem),
        Type::Paren(paren) => written_as_role_key(&paren.elem),
        Type::Path(path) => path.qself.is_none() && ends_in(&path.path, ALIASES),
        Type::TraitObject(object) => object
            .bounds
            .iter()
            .any(|bound| matches!(bound, TypeParamBound::Trait(t) if ends_in(&t.path, TWINS))),
        _ => false,
    }
}

/// `into` followed by a type and a single `:`; `into(x)` and a path through `into::` are
/// expressions.
fn is_contribution(input: ParseStream<'_>) -> bool {
    let fork = input.fork();
    let Ok(ident) = fork.parse::<Ident>() else { return false };
    ident == "into" && fork.parse::<Type>().is_ok() && fork.peek(Token![:]) && !fork.peek(Token![::])
}

/// A provider type is a path without parenthesized arguments. syn reads `Config::default()` as a
/// type, the `Fn()` sugar, so a call would otherwise bind as `provide::<..>()`.
fn is_type_entry(ty: &Type) -> bool {
    matches!(ty, Type::Path(_)) && !is_call(ty)
}

/// A path with parenthesized arguments, looking through the invisible group a `macro_rules`
/// `$t:ty` produces. An `into` list refuses only this shape among types, having no bare
/// expressions for any other to be mistaken for.
fn is_call(ty: &Type) -> bool {
    match ty {
        Type::Group(group) => is_call(&group.elem),
        Type::Path(path) => {
            path.path.segments.iter().any(|segment| matches!(segment.arguments, PathArguments::Parenthesized(_)))
        }
        _ => false,
    }
}
