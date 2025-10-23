use proc_macro::TokenStream;
use quote::quote;
use syn::{Data, DeriveInput, Field, Fields, Type, parse_macro_input};

pub fn derive_config(input: TokenStream) -> TokenStream {
    let input = parse_macro_input!(input as DeriveInput);
    let name = &input.ident;

    let Data::Struct(data_struct) = &input.data else {
        return syn::Error::new_spanned(&input, "Config can only be derived for structs")
            .to_compile_error()
            .into();
    };

    let Fields::Named(fields) = &data_struct.fields else {
        return syn::Error::new_spanned(&input, "Config requires named fields")
            .to_compile_error()
            .into();
    };

    let field_loaders = fields
        .named
        .iter()
        .map(|field| generate_field_loader(field));

    // Check if validator is being used
    let has_validation = fields.named.iter().any(|field| {
        field
            .attrs
            .iter()
            .any(|attr| attr.path().is_ident("validate"))
    });

    let validation_impl = if has_validation {
        quote! {
            impl ::toni_config::Validate for #name {
                fn validate(&self) -> Result<(), ::toni_config::ConfigError> {
                    #[cfg(feature = "validation")]
                    {
                        ::validator::Validate::validate(self)
                            .map_err(|e| ::toni_config::ConfigError::ValidationError(e.to_string()))
                    }
                    #[cfg(not(feature = "validation"))]
                    {
                        Ok(())
                    }
                }
            }
        }
    } else {
        quote! {
            impl ::toni_config::Validate for #name {
                fn validate(&self) -> Result<(), ::toni_config::ConfigError> {
                    Ok(())
                }
            }
        }
    };

    let expanded = quote! {
        impl ::toni_config::FromEnv for #name {
            fn load_from_env() -> Result<Self, ::toni_config::ConfigError> {
                Ok(Self {
                    #(#field_loaders),*
                })
            }
        }

        #validation_impl
    };

    TokenStream::from(expanded)
}

fn generate_field_loader(field: &Field) -> proc_macro2::TokenStream {
    let field_name = field.ident.as_ref().unwrap();
    let field_type = &field.ty;

    // Extract attributes
    let env_key = extract_env_key(field);
    let default_value = extract_default_value(field);
    let is_nested = has_nested_attr(field);
    let is_optional = is_option_type(field_type);

    // Default env key is SCREAMING_SNAKE_CASE of field name
    let env_key = env_key.unwrap_or_else(|| field_name.to_string().to_uppercase());

    if is_nested {
        // Nested config struct
        quote! {
            #field_name: {
                use ::toni_config::FromEnv;
                <#field_type>::load_from_env()?
            }
        }
    } else if let Some(default) = default_value {
        // Has default value
        // If env var is set, we must parse it or error
        // If env var is not set, use the default
        quote! {
            #field_name: match std::env::var(#env_key) {
                Ok(val) => val.parse()
                    .map_err(|e| ::toni_config::ConfigError::ParseError {
                        key: #env_key.to_string(),
                        message: format!("{:?}", e),
                    })?,
                Err(_) => {
                    let default_val: #field_type = (#default).into();
                    default_val
                }
            }
        }
    } else if is_optional {
        // Optional field (Option<T>)
        quote! {
            #field_name: std::env::var(#env_key)
                .ok()
                .and_then(|s| s.parse().ok())
        }
    } else {
        // Required field
        quote! {
            #field_name: {
                let val = std::env::var(#env_key)
                    .map_err(|_| ::toni_config::ConfigError::MissingEnvVar(#env_key.to_string()))?;
                val.parse()
                    .map_err(|e| ::toni_config::ConfigError::ParseError {
                        key: #env_key.to_string(),
                        message: format!("{:?}", e),
                    })?
            }
        }
    }
}

fn extract_env_key(field: &Field) -> Option<String> {
    for attr in &field.attrs {
        if attr.path().is_ident("env") {
            if let Ok(lit) = attr.parse_args::<syn::LitStr>() {
                return Some(lit.value());
            }
        }
    }
    None
}

fn extract_default_value(field: &Field) -> Option<proc_macro2::TokenStream> {
    for attr in &field.attrs {
        if attr.path().is_ident("default") {
            // Try to parse as literal or expression
            if let Ok(tokens) = attr.parse_args::<proc_macro2::TokenStream>() {
                return Some(tokens);
            }
        }
    }
    None
}

fn has_nested_attr(field: &Field) -> bool {
    field
        .attrs
        .iter()
        .any(|attr| attr.path().is_ident("nested"))
}

fn is_option_type(ty: &Type) -> bool {
    if let Type::Path(type_path) = ty {
        if let Some(segment) = type_path.path.segments.last() {
            return segment.ident == "Option";
        }
    }
    false
}
