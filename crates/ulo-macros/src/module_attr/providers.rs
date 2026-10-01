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
//! A type that does not implement `Construct` fails at `provide::<T>()`; the generated call is
//! spanned at the entry so the error carries the hint "add #[injectable] or bind it with a
//! factory".

use proc_macro2::TokenStream;
use syn::parse::{Parse, ParseStream};
use syn::{Expr, ExprClosure, Type};

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
        todo!()
    }
}

impl ProviderEntry {
    /// The statement this entry writes inside `register`, with `m` the `ModuleDef`.
    pub(crate) fn lower(&self) -> TokenStream {
        todo!()
    }
}
