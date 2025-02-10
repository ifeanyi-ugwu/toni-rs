use proc_macro2::TokenStream;
use syn::{Ident, ItemImpl, ItemStruct, Result, parse2};

use crate::utils::extracts::{extract_controller_prefix, extract_struct_dependencies};

use super::{
    impl_functions::process_impl_functions, manager::generate_manager, output::generate_output,
};

pub fn handle_controller_struct(
    attr: TokenStream,
    item: TokenStream,
    trait_name: Ident,
) -> Result<TokenStream> {
    let struct_attrs = parse2::<ItemStruct>(attr)?;
    let impl_block = parse2::<ItemImpl>(item)?;

    let prefix_path = extract_controller_prefix(&impl_block)?;
    let dependencies = extract_struct_dependencies(&struct_attrs)?;

    let (controllers, metadata, unique_dependencies) = process_impl_functions(
        &impl_block,
        &dependencies,
        &struct_attrs.ident,
        &trait_name,
        &prefix_path,
    )?;

    let manager = generate_manager(&struct_attrs.ident, metadata, unique_dependencies);
    let expanded = generate_output(struct_attrs, impl_block, controllers, manager);

    Ok(expanded.into())
}
