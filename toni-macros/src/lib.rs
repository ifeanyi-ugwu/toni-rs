extern crate proc_macro2;

use controller_macro::controller_struct::handle_controller_struct;
use proc_macro::TokenStream;
use proc_macro2::Span;
use provider_macro::provider_struct::handle_provider_struct;
use syn::Ident;

mod config_macro;
mod controller_macro;
mod enhancer;
mod markers_params;
mod middleware_macro;
mod module_macro;
mod provider_macro;
mod shared;
mod utils;

#[proc_macro_attribute]
pub fn module(attr: TokenStream, item: TokenStream) -> TokenStream {
    module_macro::module_struct::module(attr, item)
}

#[proc_macro_attribute]
pub fn controller_struct(attr: TokenStream, item: TokenStream) -> TokenStream {
    let attr = proc_macro2::TokenStream::from(attr);
    let item = proc_macro2::TokenStream::from(item);
    let trait_name = Ident::new("ControllerTrait", Span::call_site());
    let output = handle_controller_struct(attr, item, trait_name);
    proc_macro::TokenStream::from(output.unwrap_or_else(|e| e.to_compile_error()))
}

#[proc_macro_attribute]
pub fn provider_struct(attr: TokenStream, item: TokenStream) -> TokenStream {
    let attr = proc_macro2::TokenStream::from(attr);
    let item = proc_macro2::TokenStream::from(item);
    let trait_name = Ident::new("ProviderTrait", Span::call_site());
    let output = handle_provider_struct(attr, item, trait_name);
    proc_macro::TokenStream::from(output.unwrap_or_else(|e| e.to_compile_error()))
}

#[proc_macro_attribute]
pub fn controller(_attr: TokenStream, item: TokenStream) -> TokenStream {
    item
}

#[proc_macro_attribute]
pub fn get(_attr: TokenStream, item: TokenStream) -> TokenStream {
    item
}
#[proc_macro_attribute]
pub fn post(_attr: TokenStream, item: TokenStream) -> TokenStream {
    item
}
#[proc_macro_attribute]
pub fn put(_attr: TokenStream, item: TokenStream) -> TokenStream {
    item
}
#[proc_macro_attribute]
pub fn delete(_attr: TokenStream, item: TokenStream) -> TokenStream {
    item
}

#[proc_macro_attribute]
pub fn use_guard(_attr: TokenStream, item: TokenStream) -> TokenStream {
    item
}

#[proc_macro_attribute]
pub fn interceptor(_attr: TokenStream, item: TokenStream) -> TokenStream {
    item
}

#[proc_macro_attribute]
pub fn pipe(_attr: TokenStream, item: TokenStream) -> TokenStream {
    item
}

// Helper derive to register #[inject] and #[default] as valid attributes
// This allows them to be used on struct fields in provider_struct/controller_struct
#[proc_macro_derive(Injectable, attributes(inject, default))]
pub fn derive_injectable(_input: TokenStream) -> TokenStream {
    // This derive does nothing - it just registers the attributes
    TokenStream::new()
}

#[proc_macro_derive(Config, attributes(env, default, nested))]
pub fn derive_config(input: TokenStream) -> TokenStream {
    config_macro::derive_config(input)
}
