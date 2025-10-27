use proc_macro2::TokenStream;
use syn::{Ident, ItemImpl, Result, parse2};

use crate::{
    shared::scope_parser::ProviderStructArgs,
    utils::extracts::extract_struct_dependencies,
};

use super::instance_injection::generate_instance_provider_system;

pub fn handle_provider_struct(
    attr: TokenStream,
    item: TokenStream,
    _trait_name: Ident,
) -> Result<TokenStream> {
    // Parse: #[provider_struct(scope = "request", pub struct Foo { ... })]
    let args = parse2::<ProviderStructArgs>(attr)?;
    let impl_block = parse2::<ItemImpl>(item)?;

    let scope = args.scope;
    let struct_attrs = args.struct_def;

    let dependencies = extract_struct_dependencies(&struct_attrs)?;

    let expanded = generate_instance_provider_system(&struct_attrs, &impl_block, &dependencies, scope)?;

    Ok(expanded)
}
