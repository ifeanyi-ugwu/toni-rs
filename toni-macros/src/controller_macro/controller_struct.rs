use proc_macro2::TokenStream;
use syn::{Ident, ItemImpl, ItemStruct, Result, parse2};

use crate::utils::extracts::{extract_controller_prefix, extract_struct_dependencies};

use super::instance_injection::generate_instance_controller_system;

pub fn handle_controller_struct(
    attr: TokenStream,
    item: TokenStream,
    _trait_name: Ident,
) -> Result<TokenStream> {
    let struct_attrs = parse2::<ItemStruct>(attr)?;
    let impl_block = parse2::<ItemImpl>(item)?;

    let prefix_path = extract_controller_prefix(&impl_block)?;
    let dependencies = extract_struct_dependencies(&struct_attrs)?;

    // Use new instance injection pattern
    let expanded = generate_instance_controller_system(
        &struct_attrs,
        &impl_block,
        &dependencies,
        &prefix_path,
    )?;

    Ok(expanded)
}
