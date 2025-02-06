extern crate proc_macro2;

use controller::controller_struct::handle_controller_struct;
use proc_macro::TokenStream;
use proc_macro2::Span;
use provider::provider_struct::handle_provider_struct;
use syn::Ident;

mod controller_macro;
mod service_struct;
mod utils;
mod module_macro;
mod controller;
mod shared;
mod provider;

#[proc_macro_attribute]
pub fn module(attr: TokenStream, item: TokenStream) -> TokenStream {
    module_macro::module_struct::module(attr, item)
}

#[proc_macro_attribute]
pub fn controller(_attr: TokenStream, item: TokenStream) -> TokenStream {
    controller_macro::controller::controllers(_attr, item)
}

#[proc_macro_attribute]
pub fn controller_struct(attr: TokenStream, item: TokenStream) -> TokenStream {
    let attr = proc_macro2::TokenStream::from(attr);
    let item = proc_macro2::TokenStream::from(item);
    let trait_name = Ident::new("ControllerTrait", Span::call_site());
    let output = handle_controller_struct(attr, item, trait_name);
    proc_macro::TokenStream::from(output.unwrap_or_else(|e| e.to_compile_error().into()))
}

#[proc_macro_attribute]
pub fn provider_struct(attr: TokenStream, item: TokenStream) -> TokenStream {
    let attr = proc_macro2::TokenStream::from(attr);
    let item = proc_macro2::TokenStream::from(item);
    let trait_name = Ident::new("ProviderTrait", Span::call_site());
    let output = handle_provider_struct(attr, item, trait_name);
    proc_macro::TokenStream::from(output.unwrap_or_else(|e| e.to_compile_error().into()))
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
pub fn provider_struct2(_attr: TokenStream, item: TokenStream) -> TokenStream {
    service_struct::insert_struct(_attr, item, Ident::new("ProviderTrait", Span::call_site()))
}

#[proc_macro_attribute]
pub fn controller_struct2(_attr: TokenStream, item: TokenStream) -> TokenStream {
    controller_macro::controller_struct::insert_struct(_attr, item, Ident::new("ControllerTrait", Span::call_site()))
}