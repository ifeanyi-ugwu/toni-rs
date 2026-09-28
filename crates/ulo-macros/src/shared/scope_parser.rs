use syn::{
    ItemStruct, LitStr, Result, Token,
    parse::{Parse, ParseStream},
};

/// What `scope = "request"` is answered with. The unit was never one HTTP request; ADR-0046 has
/// the reasoning, and the old spelling is refused rather than accepted as an alias.
pub(crate) const SCOPE_RENAMED: &str = "scope = \"request\" is now scope = \"execution\". The unit is one \
execution: one HTTP request, one WebSocket message, one RPC or gRPC call, or one standalone \
execution.";

/// Provider scope types
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ProviderScope {
    Singleton,
    Execution,
    Transient,
}

impl Default for ProviderScope {
    fn default() -> Self {
        Self::Singleton
    }
}

/// Controller scope types (only Singleton and Execution)
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ControllerScope {
    Singleton,
    Execution,
}

impl Default for ControllerScope {
    fn default() -> Self {
        Self::Singleton // Controllers are Singleton by default (like NestJS)
    }
}

/// Parse new consolidated controller attribute.
/// Supports:
/// - `#[controller] impl Foo { ... }` — struct defined separately (preferred)
/// - `#[controller("/path")] impl Foo { ... }` — with route prefix
/// - `#[controller("/path", pub struct Foo { ... })]` — inline struct (legacy)
pub struct ControllerArgs {
    pub path: String,
    pub scope: ControllerScope,
    pub was_explicit: bool,
    /// `None` when the struct is defined above the impl (preferred style).
    /// `Some` for the legacy inline syntax.
    pub struct_def: Option<ItemStruct>,
}

impl Parse for ControllerArgs {
    fn parse(input: ParseStream) -> Result<Self> {
        let mut path = String::new();
        let mut scope = ControllerScope::default();
        let mut was_explicit = false;

        if input.peek(LitStr) {
            let path_lit: LitStr = input.parse()?;
            crate::shared::route_path::validate_route_path(&path_lit)?;
            path = path_lit.value();

            if input.peek(Token![,]) {
                let _: Token![,] = input.parse()?;
            }
        }

        while input.peek(syn::Ident) && !input.peek(Token![pub]) && !input.peek(Token![struct]) {
            let ident: syn::Ident = input.parse()?;

            if ident == "scope" {
                let _eq: Token![=] = input.parse()?;
                let value: LitStr = input.parse()?;

                was_explicit = true;
                scope = match value.value().as_str() {
                    "singleton" => ControllerScope::Singleton,
                    "execution" => ControllerScope::Execution,
                    "request" => return Err(syn::Error::new(value.span(), SCOPE_RENAMED)),
                    other => {
                        return Err(syn::Error::new(
                            value.span(),
                            format!(
                                "Invalid controller scope: '{}'. Must be 'singleton' or 'execution'",
                                other
                            ),
                        ));
                    }
                };
            } else {
                return Err(syn::Error::new(
                    ident.span(),
                    format!("Unknown attribute: '{}'. Expected 'scope'", ident),
                ));
            }

            if input.peek(Token![,]) {
                let _: Token![,] = input.parse()?;
            }
        }

        // Inline struct is optional — absent means struct is defined separately above the impl.
        let struct_def = if !input.is_empty() {
            Some(input.parse::<ItemStruct>()?)
        } else {
            None
        };

        Ok(ControllerArgs {
            path,
            scope,
            was_explicit,
            struct_def,
        })
    }
}
