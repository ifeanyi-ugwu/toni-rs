use proc_macro2::{Span, TokenStream};
use quote::quote_spanned;
use syn::parse::{Parse, ParseStream};
use syn::{Expr, Ident, Token};

/// `#[injectable(<scope>?, timeout = <expr>?)]`, in either order.
#[derive(Default)]
pub(crate) struct InjectableArgs {
    pub(crate) scope: Option<ScopeArg>,
    pub(crate) timeout: Option<Expr>,
}

pub(crate) enum ScopeArg {
    Singleton(Span),
    Execution(Span),
    Transient(Span),
}

impl ScopeArg {
    /// The scope marker the generated `Construct::Scope` names, spanned at the argument so a
    /// scope error points at the word the user wrote.
    pub(crate) fn path(&self) -> TokenStream {
        match self {
            ScopeArg::Singleton(span) => quote_spanned!(*span=> ::ulo::scope::Singleton),
            ScopeArg::Execution(span) => quote_spanned!(*span=> ::ulo::scope::PerExecution),
            ScopeArg::Transient(span) => quote_spanned!(*span=> ::ulo::scope::Transient),
        }
    }
}

impl Parse for InjectableArgs {
    fn parse(input: ParseStream<'_>) -> syn::Result<Self> {
        let mut args = InjectableArgs::default();
        while !input.is_empty() {
            let ident: Ident = input.parse()?;
            match ident.to_string().as_str() {
                "singleton" | "execution" | "transient" if args.scope.is_some() => {
                    return Err(syn::Error::new(ident.span(), "a scope is written once"));
                }
                "singleton" => args.scope = Some(ScopeArg::Singleton(ident.span())),
                "execution" => args.scope = Some(ScopeArg::Execution(ident.span())),
                "transient" => args.scope = Some(ScopeArg::Transient(ident.span())),
                "timeout" if args.timeout.is_some() => {
                    return Err(syn::Error::new(ident.span(), "`timeout` is written once"));
                }
                "timeout" => {
                    input.parse::<Token![=]>()?;
                    args.timeout = Some(input.parse()?);
                }
                _ => {
                    return Err(syn::Error::new(
                        ident.span(),
                        "expected `singleton`, `execution`, `transient` or `timeout = <Duration>`",
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
