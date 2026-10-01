//! One entry of a `providers` list and its lowering to the value API:
//!
//! | Entry | Lowers to |
//! |---|---|
//! | `UserService` | `m.provide::<UserService>();` |
//! | `PgUserRepo as dyn UserRepo` | `m.provide::<PgUserRepo>().also_as::<dyn UserRepo>(\|a\| a);` |
//! | `AppConfig::from_env()?` | `m.try_value(AppConfig::from_env());` |
//! | `expr` | `m.value(expr);` |
//! | a closure | `singleton` or `try_singleton`, picked by autoref over the closure's own type |
//! | `into dyn Plugin: [A, B]` | `m.contribute::<dyn Plugin>().provide::<A>(\|a\| a);` per entry |
//!
//! A type entry is a plain type path: `Foo`, `db::Pool<Pg>`. A path with parenthesized arguments,
//! `Config::default()`, is a call and reads as an expression. A bare path naming a constant reads
//! as a type, so a constant is bound by value with a block, `{ LIMITS }`.
//!
//! A type that does not implement `Construct` fails at `provide::<T>()`; the generated call is
//! spanned at the entry, where the error is reported.

use proc_macro2::TokenStream;
use quote::{quote, quote_spanned};
use syn::parse::{Parse, ParseStream};
use syn::punctuated::Punctuated;
use syn::spanned::Spanned;
use syn::{Expr, ExprClosure, Ident, PathArguments, Token, Type, bracketed};

use crate::module_attr::module_def_param;
use crate::shared::{check_factory_params, ulo};

pub(crate) enum ProviderEntry {
    Provide(Type),
    ProvideAs { ty: Type, as_ty: Type },
    TryValue(Expr),
    Value(Expr),
    Factory(ExprClosure),
    Contribute { into: Type, items: Vec<Type> },
}

impl Parse for ProviderEntry {
    /// `into T: [..]` first, then a type followed by `,`, `as` or the end, then an expression.
    fn parse(input: ParseStream<'_>) -> syn::Result<Self> {
        if is_contribution(input) {
            input.parse::<Ident>()?;
            let into: Type = input.parse()?;
            input.parse::<Token![:]>()?;
            let content;
            bracketed!(content in input);
            let items = Punctuated::<Type, Token![,]>::parse_terminated(&content)?.into_iter().collect();
            return Ok(ProviderEntry::Contribute { into, items });
        }

        let fork = input.fork();
        if fork.parse::<Type>().is_ok_and(|ty| is_type_entry(&ty))
            && (fork.is_empty() || fork.peek(Token![,]) || fork.peek(Token![as]))
        {
            let ty: Type = input.parse()?;
            if input.peek(Token![as]) {
                input.parse::<Token![as]>()?;
                return Ok(ProviderEntry::ProvideAs { ty, as_ty: input.parse()? });
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
                let contributions = items.iter().map(|item| {
                    quote_spanned! {item.span()=>
                        #m.contribute::<#into>().provide::<#item>(|a| a);
                    }
                });
                quote!(#(#contributions)*)
            }
        }
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
    match ty {
        Type::Path(path) => {
            path.path.segments.iter().all(|segment| !matches!(segment.arguments, PathArguments::Parenthesized(_)))
        }
        _ => false,
    }
}
