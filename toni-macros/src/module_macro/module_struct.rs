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
    imports: Vec<Ident>,
    controllers: Vec<Ident>,
    providers: Vec<Ident>,
    exports: Vec<Ident>,
}

struct ConfigParser {
    imports: Vec<Ident>,
    controllers: Vec<Ident>,
    providers: Vec<Ident>,
    exports: Vec<Ident>,
}

impl Parse for ConfigParser {
    fn parse(input: ParseStream) -> syn::Result<Self> {
        let mut config = ConfigParser {
            imports: Vec::new(),
            controllers: Vec::new(),
            providers: Vec::new(),
            exports: Vec::new(),
        };

        while !input.is_empty() {
            let key: Ident = input.parse()?;
            input.parse::<Token![:]>()?;
            let content;
            bracketed!(content in input);

            let fields = Punctuated::<Ident, Token![,]>::parse_terminated(&content)?;

            match key.to_string().as_str() {
                "imports" => config.imports = fields.into_iter().collect(),
                "controllers" => {
                    config.controllers = fields
                        .into_iter()
                        .map(|field| {
                            Ident::new(&format!("{}Manager", field), field.span())
                        })
                        .collect()
                }
                "providers" => {
                    config.providers = fields
                        .into_iter()
                        .map(|field| {
                            Ident::new(&format!("{}Manager", field), field.span())
                        })
                        .collect()
                }
                "exports" => config.exports = fields.into_iter().collect(),
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
    let imports: &Vec<Ident> = &config.imports;
    let controllers = config.controllers;
    let providers = &config.providers;
    let exports = &config.exports;
    let exports_string: Vec<String> = exports.iter().map(|e| e.to_string()).collect();

    let generated = quote! {
        pub struct #input_ident {
            id: ::uuid::Uuid
        }

        impl #input_ident {
            pub fn module_definition() -> ::toni::module_helpers::module_enum::ModuleDefinition {
                let app_module = Self {
                    id: ::uuid::Uuid::new_v4()
                };
                ::toni::module_helpers::module_enum::ModuleDefinition::DefaultModule(Box::new(app_module))
            }
            pub fn new() -> Self {
                Self {
                    id: ::uuid::Uuid::new_v4()
                }
            }
        }

        impl ::toni::traits_helpers::ModuleMetadata for #input_ident {
            fn get_id(&self) -> String {
                #input_name.to_string()
            }
            fn get_name(&self) -> String {
                #input_name.to_string()
            }
            fn imports(&self) -> Option<Vec<Box<dyn ::toni::traits_helpers::ModuleMetadata>>> {
                Some(vec![#(Box::new(#imports::new())),*])
            }
            fn controllers(&self) -> Option<Vec<Box<dyn ::toni::traits_helpers::Controller>>> {
                Some(vec![#(Box::new(#controllers)),*])
            }
            fn providers(&self) -> Option<Vec<Box<dyn ::toni::traits_helpers::Provider>>> {
                Some(vec![#(Box::new(#providers)),*])
            }
            fn exports(&self) -> Option<Vec<String>> {
                Some(vec![#(#exports_string.to_string()),*])
            }
        }
    };

    generated.into()
}
