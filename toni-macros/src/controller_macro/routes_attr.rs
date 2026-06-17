//! `#[routes]` — the impl-side half of a controller.
//!
//! Pairs with `#[controller("/p")]` on the struct. Scans the handler methods and emits the route
//! wrappers, the `Controller` object, and the factory; construction and the route prefix are
//! delegated to the struct's bridges. See [`super::instance_injection`] for the codegen.

use proc_macro2::TokenStream;
use syn::{ItemImpl, Result, parse2};

use super::instance_injection::generate_routes_system;

pub fn handle_routes(item: TokenStream) -> Result<TokenStream> {
    let impl_block = parse2::<ItemImpl>(item)?;
    generate_routes_system(&impl_block)
}
