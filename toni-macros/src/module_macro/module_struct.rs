use proc_macro::TokenStream;
use proc_macro2::Span;
use quote::quote;
use syn::{
    Ident, ItemImpl, Token, Type, TypePath, bracketed,
    parse::{Parse, ParseStream},
    parse_macro_input,
    punctuated::Punctuated,
};

#[derive(Debug, Default)]
struct ModuleConfig {
    imports: Vec<syn::Expr>,
    controllers: Vec<Ident>,
    providers: Vec<Ident>,
    exports: Vec<Ident>,
    global: bool,
}

struct ConfigParser {
    imports: Vec<syn::Expr>,
    controllers: Vec<Ident>,
    providers: Vec<Ident>,
    exports: Vec<Ident>,
    global: bool,
}

impl Parse for ConfigParser {
    fn parse(input: ParseStream) -> syn::Result<Self> {
        let mut config = ConfigParser {
            imports: Vec::new(),
            controllers: Vec::new(),
            providers: Vec::new(),
            exports: Vec::new(),
            global: false,
        };

        while !input.is_empty() {
            let key: Ident = input.parse()?;

            // Handle global as a boolean (not an array)
            if key.to_string().as_str() == "global" {
                input.parse::<Token![:]>()?;
                let value: syn::LitBool = input.parse()?;
                config.global = value.value;

                if !input.is_empty() {
                    input.parse::<Token![,]>()?;
                }
                continue;
            }

            input.parse::<Token![:]>()?;
            let content;
            bracketed!(content in input);

            match key.to_string().as_str() {
                "imports" => {
                    // Parse imports as expressions (allows method calls, etc.)
                    let fields = Punctuated::<syn::Expr, Token![,]>::parse_terminated(&content)?;
                    config.imports = fields.into_iter().collect();
                }
                "controllers" => {
                    let fields = Punctuated::<Ident, Token![,]>::parse_terminated(&content)?;
                    config.controllers = fields
                        .into_iter()
                        .map(|field| Ident::new(&format!("{}Manager", field), field.span()))
                        .collect()
                }
                "providers" => {
                    let fields = Punctuated::<Ident, Token![,]>::parse_terminated(&content)?;
                    config.providers = fields
                        .into_iter()
                        .map(|field| Ident::new(&format!("{}Manager", field), field.span()))
                        .collect()
                }
                "exports" => {
                    let fields = Punctuated::<Ident, Token![,]>::parse_terminated(&content)?;
                    config.exports = fields.into_iter().collect()
                }
                _ => return Err(syn::Error::new(key.span(), "Unknown field")),
            }

            if !input.is_empty() {
                input.parse::<Token![,]>()?;
            }
        }

        Ok(config)
    }
}

impl TryFrom<TokenStream> for ModuleConfig {
    type Error = syn::Error;
    fn try_from(attr: TokenStream) -> syn::Result<Self> {
        let parser = syn::parse::<ConfigParser>(attr)?;
        Ok(ModuleConfig {
            imports: parser.imports,
            controllers: parser.controllers,
            providers: parser.providers,
            exports: parser.exports,
            global: parser.global,
        })
    }
}

pub fn module(attr: TokenStream, item: TokenStream) -> TokenStream {
    let config = match ModuleConfig::try_from(attr) {
        Ok(c) => c,
        Err(e) => return e.to_compile_error().into(),
    };

    let input = parse_macro_input!(item as ItemImpl);
    let input_type = input.self_ty.as_ref();
    let input_ident = match input_type {
        Type::Path(TypePath { path, .. }) => path.segments.last().unwrap().ident.clone(),
        _ => {
            return syn::Error::new(Span::call_site(), "Invalid input type")
                .to_compile_error()
                .into();
        }
    };
    let input_name = input_ident.to_string();
    let imports = &config.imports;
    let controllers = config.controllers;
    let providers = &config.providers;
    let exports = &config.exports;
    let exports_string: Vec<String> = exports.iter().map(|e| e.to_string()).collect();
    let is_global = config.global;

    let generated = quote! {
        pub struct #input_ident;

        impl #input_ident {
            pub fn module_definition() -> ::toni::module_helpers::module_enum::ModuleDefinition {
                let app_module = Self;
                ::toni::module_helpers::module_enum::ModuleDefinition::DefaultModule(Box::new(app_module))
            }
            pub fn new() -> Self {
                Self
            }
        }

        impl ::toni::traits_helpers::ModuleMetadata for #input_ident {
            fn get_id(&self) -> String {
                #input_name.to_string()
            }
            fn get_name(&self) -> String {
                #input_name.to_string()
            }
            fn is_global(&self) -> bool {
                #is_global
            }
            fn imports(&self) -> Option<Vec<Box<dyn ::toni::traits_helpers::ModuleMetadata>>> {
                Some(vec![#(Box::new(#imports)),*])
            }
            fn controllers(&self) -> Option<Vec<Box<dyn ::toni::traits_helpers::Controller>>> {
                Some(vec![#(Box::new(#controllers)),*])
            }
            fn providers(&self) -> Option<Vec<Box<dyn ::toni::traits_helpers::Provider>>> {
                // Auto-inject built-in RequestManager + user providers
                Some(vec![
                    Box::new(::toni::RequestManager),
                    #(Box::new(#providers)),*
                ])
            }
            fn exports(&self) -> Option<Vec<String>> {
                Some(vec![#(#exports_string.to_string()),*])
            }
        }
    };

    generated.into()
}
