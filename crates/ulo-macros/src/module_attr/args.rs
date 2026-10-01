use syn::parse::{Parse, ParseStream};
use syn::punctuated::Punctuated;
use syn::{Expr, Ident, Token, Type, bracketed};

use crate::module_attr::providers::ProviderEntry;

/// `#[module(global, imports = [..], providers = [..], controllers = [..], exports = [..])]`,
/// each key at most once, in any order.
#[derive(Default)]
pub(crate) struct ModuleArgs {
    pub(crate) global: bool,
    pub(crate) imports: Vec<Expr>,
    pub(crate) providers: Vec<ProviderEntry>,
    pub(crate) controllers: Vec<Type>,
    pub(crate) exports: Vec<ExportEntry>,
}

pub(crate) enum ExportEntry {
    /// `m.export::<T>()`.
    Export(Type),
    /// `reexport T`: `m.reexport::<T>()`.
    Reexport(Type),
}

impl Parse for ModuleArgs {
    fn parse(input: ParseStream<'_>) -> syn::Result<Self> {
        let mut args = ModuleArgs::default();
        let mut seen: Vec<String> = Vec::new();
        while !input.is_empty() {
            let key: Ident = input.parse()?;
            let name = key.to_string();
            if seen.contains(&name) {
                return Err(syn::Error::new(key.span(), format!("`{name}` is written once")));
            }
            seen.push(name.clone());
            match name.as_str() {
                "global" => args.global = true,
                "imports" => args.imports = list::<Expr>(input)?,
                "providers" => args.providers = list::<ProviderEntry>(input)?,
                "controllers" => args.controllers = list::<Type>(input)?,
                "exports" => args.exports = list::<ExportEntry>(input)?,
                _ => {
                    return Err(syn::Error::new(
                        key.span(),
                        "expected `global`, `imports`, `providers`, `controllers` or `exports`",
                    ));
                }
            }
            if !input.is_empty() {
                input.parse::<Token![,]>()?;
            }
        }
        Ok(args)
    }
}

/// `= [a, b, ..]`, trailing comma allowed.
fn list<T: Parse>(input: ParseStream<'_>) -> syn::Result<Vec<T>> {
    input.parse::<Token![=]>()?;
    let content;
    bracketed!(content in input);
    Ok(Punctuated::<T, Token![,]>::parse_terminated(&content)?.into_iter().collect())
}

impl Parse for ExportEntry {
    fn parse(input: ParseStream<'_>) -> syn::Result<Self> {
        if input.peek(Ident) && input.fork().parse::<Ident>().is_ok_and(|i| i == "reexport") && !input.peek2(Token![,]) {
            input.parse::<Ident>()?;
            return Ok(ExportEntry::Reexport(input.parse()?));
        }
        Ok(ExportEntry::Export(input.parse()?))
    }
}
