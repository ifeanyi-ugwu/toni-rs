use syn::parse::{Parse, ParseStream};
use syn::{Expr, Ident, Token};

use crate::shared::scope::ScopeArg;

/// `#[injectable(<scope>?, timeout = <expr>?)]`, in either order.
#[derive(Default)]
pub(crate) struct InjectableArgs {
    pub(crate) scope: Option<ScopeArg>,
    pub(crate) timeout: Option<Expr>,
}

impl Parse for InjectableArgs {
    fn parse(input: ParseStream<'_>) -> syn::Result<Self> {
        let mut args = InjectableArgs::default();
        while !input.is_empty() {
            let ident: Ident = input.parse()?;
            if let Some(scope) = ScopeArg::from_ident(&ident) {
                if args.scope.is_some() {
                    return Err(syn::Error::new(ident.span(), "a scope is written once"));
                }
                args.scope = Some(scope);
            } else if ident == "timeout" {
                if args.timeout.is_some() {
                    return Err(syn::Error::new(ident.span(), "`timeout` is written once"));
                }
                input.parse::<Token![=]>()?;
                args.timeout = Some(input.parse()?);
            } else {
                return Err(syn::Error::new(
                    ident.span(),
                    "expected `singleton`, `execution`, `transient` or `timeout = <Duration>`",
                ));
            }
            if !input.is_empty() {
                input.parse::<Token![,]>()?;
            }
        }
        Ok(args)
    }
}
