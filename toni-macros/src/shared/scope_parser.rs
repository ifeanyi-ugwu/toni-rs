use syn::{Attribute, Ident, LitStr, Result, Token, parse::{Parse, ParseStream}};

/// Provider scope types
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ProviderScope {
    Singleton,
    Request,
    Transient,
}

impl Default for ProviderScope {
    fn default() -> Self {
        Self::Singleton
    }
}

/// Parse scope attribute from: #[scope("singleton")]
pub struct ScopeAttribute {
    pub scope: ProviderScope,
}

impl Parse for ScopeAttribute {
    fn parse(input: ParseStream) -> Result<Self> {
        // Parse just the string literal
        let value: LitStr = input.parse()?;

        let scope = match value.value().as_str() {
            "singleton" => ProviderScope::Singleton,
            "request" => ProviderScope::Request,
            "transient" => ProviderScope::Transient,
            other => {
                return Err(syn::Error::new(
                    value.span(),
                    format!(
                        "Invalid scope: '{}'. Must be 'singleton', 'request', or 'transient'",
                        other
                    )
                ));
            }
        };

        Ok(ScopeAttribute { scope })
    }
}

/// Extract scope from attributes like #[scope("singleton")]
pub fn parse_scope_from_attrs(attrs: &[Attribute]) -> Result<ProviderScope> {
    for attr in attrs {
        if attr.path().is_ident("scope") {
            let scope_attr: ScopeAttribute = attr.parse_args()?;
            return Ok(scope_attr.scope);
        }
    }

    // Default to singleton if no scope attribute found
    Ok(ProviderScope::default())
}

#[cfg(test)]
mod tests {
    use super::*;
    use quote::quote;
    use syn::parse_quote;

    #[test]
    fn test_parse_singleton_scope() {
        let attr: Attribute = parse_quote! {
            #[scope("singleton")]
        };
        let scope = parse_scope_from_attrs(&[attr]).unwrap();
        assert_eq!(scope, ProviderScope::Singleton);
    }

    #[test]
    fn test_parse_request_scope() {
        let attr: Attribute = parse_quote! {
            #[scope("request")]
        };
        let scope = parse_scope_from_attrs(&[attr]).unwrap();
        assert_eq!(scope, ProviderScope::Request);
    }

    #[test]
    fn test_parse_transient_scope() {
        let attr: Attribute = parse_quote! {
            #[scope("transient")]
        };
        let scope = parse_scope_from_attrs(&[attr]).unwrap();
        assert_eq!(scope, ProviderScope::Transient);
    }

    #[test]
    fn test_default_scope() {
        // No scope attribute = defaults to singleton
        let scope = parse_scope_from_attrs(&[]).unwrap();
        assert_eq!(scope, ProviderScope::Singleton);
    }

    #[test]
    fn test_invalid_scope() {
        let attr: Attribute = parse_quote! {
            #[scope("invalid")]
        };
        let result = parse_scope_from_attrs(&[attr]);
        assert!(result.is_err());
    }
}
