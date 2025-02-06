use proc_macro2::{Ident, Span};

use super::snake_to_upper::{snake_to_upper, upper_to_snake};

pub fn create_struct_name(ref_name: &str, method_name: &Ident) -> Ident {
    let method_name_upper = snake_to_upper(method_name);
    let struct_name_created = format!("{}{}", method_name_upper, ref_name);
    let clean_name = match struct_name_created.strip_prefix("_") {
        Some(name) => name,
        None => struct_name_created.as_str(),
    };
    Ident::new(clean_name, Span::call_site())
}

pub fn create_field_struct_name(ref_name: &str, field_name: &Ident) -> Ident {
    let method_name_snake = upper_to_snake(ref_name);
    let struct_name_created = format!("{}{}", field_name, method_name_snake);
    Ident::new(&struct_name_created, Span::call_site())
}


pub fn create_provider_name_by_fn_and_struct_ident(function_name: &Ident, struct_ident: &Ident) -> String {
    let function_name_upper = snake_to_upper(function_name);
    let provider_name = format!("{}{}", function_name_upper, struct_ident);
    let clean_name = match provider_name.strip_prefix("_") {
        Some(name) => name.to_string(),
        None => provider_name,
    };
    clean_name
}