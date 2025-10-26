use proc_macro2::TokenStream;
use syn::{Ident, ItemImpl, ItemStruct, Result, parse2};

use crate::utils::extracts::extract_struct_dependencies;

use super::instance_injection::generate_instance_provider_system;

pub fn handle_provider_struct(
    attr: TokenStream,
    item: TokenStream,
    _trait_name: Ident,
) -> Result<TokenStream> {
    let struct_attrs = parse2::<ItemStruct>(attr)?;
    let impl_block = parse2::<ItemImpl>(item)?;

    let dependencies = extract_struct_dependencies(&struct_attrs)?;

    let expanded = generate_instance_provider_system(&struct_attrs, &impl_block, &dependencies)?;

    Ok(expanded)
}
