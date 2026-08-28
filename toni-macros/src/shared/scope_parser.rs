use syn::{
    ItemStruct, LitStr, Result, Token,
    parse::{Parse, ParseStream},
};

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

/// Controller scope types (only Singleton and Request)
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ControllerScope {
    Singleton,
    Request,
}

impl Default for ControllerScope {
    fn default() -> Self {
        Self::Singleton // Controllers are Singleton by default (like NestJS)
    }
}

/// Parse injectable attribute
/// Supports two syntaxes:
/// 1. Attribute: #[injectable(scope = "request", init = "new")] pub struct Foo { ... }
/// 2. Inline: #[injectable(scope = "request", pub struct Foo { ... })]
pub struct ProviderStructArgs {
    pub scope: ProviderScope,
    pub init: Option<String>, // Optional custom constructor method name
    pub struct_def: Option<ItemStruct>, // None if using attribute syntax
}

/// Parse controller_struct attribute: #[controller_struct(scope = "request", init = "new", pub struct Foo { ... })]
pub struct ControllerStructArgs {
    pub scope: ControllerScope,
    pub was_explicit: bool,   // Did user explicitly write scope = "..."?
    pub init: Option<String>, // Optional custom constructor method name
    pub struct_def: ItemStruct,
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
    pub init: Option<String>,
    /// `None` when the struct is defined above the impl (preferred style).
    /// `Some` for the legacy inline syntax.
    pub struct_def: Option<ItemStruct>,
}

impl Parse for ProviderStructArgs {
    fn parse(input: ParseStream) -> Result<Self> {
        let mut scope = ProviderScope::default();
        let mut init: Option<String> = None;

        // Parse optional attributes: scope = "...", init = "..."
        while input.peek(syn::Ident) && !input.peek(Token![pub]) && !input.peek(Token![struct]) {
            let ident: syn::Ident = input.parse()?;

            if ident == "scope" {
                // Parse: scope = "request"
                let _eq: Token![=] = input.parse()?;
                let value: LitStr = input.parse()?;

                scope = match value.value().as_str() {
                    "singleton" => ProviderScope::Singleton,
                    "request" => ProviderScope::Request,
                    "transient" => ProviderScope::Transient,
                    other => {
                        return Err(syn::Error::new(
                            value.span(),
                            format!(
                                "Invalid scope: '{}'. Must be 'singleton', 'request', or 'transient'",
                                other
                            ),
                        ));
                    }
                };
            } else if ident == "init" {
                // Parse: init = "new"
                let _eq: Token![=] = input.parse()?;
                let value: LitStr = input.parse()?;
                init = Some(value.value());
            } else {
                return Err(syn::Error::new(
                    ident.span(),
                    format!("Unknown attribute: '{}'. Expected 'scope' or 'init'", ident),
                ));
            }

            // Consume the comma after attribute
            if input.peek(Token![,]) {
                let _: Token![,] = input.parse()?;
            }
        }

        // Try to parse struct definition (inline syntax)
        // If input is empty, struct_def will be None (attribute syntax)
        let struct_def = if !input.is_empty() {
            Some(input.parse::<ItemStruct>()?)
        } else {
            None
        };

        Ok(ProviderStructArgs {
            scope,
            init,
            struct_def,
        })
    }
}

impl Parse for ControllerStructArgs {
    fn parse(input: ParseStream) -> Result<Self> {
        let mut scope = ControllerScope::default();
        let mut was_explicit = false;
        let mut init: Option<String> = None;

        // Parse optional attributes: scope = "...", init = "..."
        while input.peek(syn::Ident) && !input.peek(Token![pub]) && !input.peek(Token![struct]) {
            let ident: syn::Ident = input.parse()?;

            if ident == "scope" {
                // Parse: scope = "request"
                let _eq: Token![=] = input.parse()?;
                let value: LitStr = input.parse()?;

                was_explicit = true; // User explicitly set the scope
                scope = match value.value().as_str() {
                    "singleton" => ControllerScope::Singleton,
                    "request" => ControllerScope::Request,
                    other => {
                        return Err(syn::Error::new(
                            value.span(),
                            format!(
                                "Invalid controller scope: '{}'. Must be 'singleton' or 'request'. Note: Controllers cannot be 'transient'",
                                other
                            ),
                        ));
                    }
                };
            } else if ident == "init" {
                // Parse: init = "new"
                let _eq: Token![=] = input.parse()?;
                let value: LitStr = input.parse()?;
                init = Some(value.value());
            } else {
                return Err(syn::Error::new(
                    ident.span(),
                    format!("Unknown attribute: '{}'. Expected 'scope' or 'init'", ident),
                ));
            }

            // Consume the comma after attribute
            if input.peek(Token![,]) {
                let _: Token![,] = input.parse()?;
            }
        }

        // Parse the struct definition
        let struct_def: ItemStruct = input.parse()?;

        Ok(ControllerStructArgs {
            scope,
            was_explicit,
            init,
            struct_def,
        })
    }
}

impl Parse for ControllerArgs {
    fn parse(input: ParseStream) -> Result<Self> {
        let mut path = String::new();
        let mut scope = ControllerScope::default();
        let mut was_explicit = false;
        let mut init: Option<String> = None;

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
                    "request" => ControllerScope::Request,
                    other => {
                        return Err(syn::Error::new(
                            value.span(),
                            format!(
                                "Invalid controller scope: '{}'. Must be 'singleton' or 'request'",
                                other
                            ),
                        ));
                    }
                };
            } else if ident == "init" {
                let _eq: Token![=] = input.parse()?;
                let value: LitStr = input.parse()?;
                init = Some(value.value());
            } else {
                return Err(syn::Error::new(
                    ident.span(),
                    format!("Unknown attribute: '{}'. Expected 'scope' or 'init'", ident),
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
            init,
            struct_def,
        })
    }
}
