use proc_macro2::TokenStream;
use syn::{Ident, ItemImpl, Result, parse2};

use crate::{
    shared::scope_parser::ControllerStructArgs,
    utils::extracts::{extract_controller_prefix, extract_struct_dependencies},
};

use super::instance_injection::generate_instance_controller_system;

pub fn handle_controller_struct(
    attr: TokenStream,
    item: TokenStream,
    _trait_name: Ident,
) -> Result<TokenStream> {
    // Parse: #[controller_struct(scope = "request", pub struct Foo { ... })]
    let args = parse2::<ControllerStructArgs>(attr)?;
    let scope = args.scope;
    let was_explicit = args.was_explicit;
    let struct_attrs = args.struct_def;

    let impl_block = parse2::<ItemImpl>(item)?;

    let prefix_path = extract_controller_prefix(&impl_block)?;
    let dependencies = extract_struct_dependencies(&struct_attrs)?;

    // Use new instance injection pattern with scope and explicitness
    let expanded = generate_instance_controller_system(
        &struct_attrs,
        &impl_block,
        &dependencies,
        &prefix_path,
        scope,
        was_explicit,
    )?;

    Ok(expanded)
}
