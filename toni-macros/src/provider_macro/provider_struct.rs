use proc_macro2::TokenStream;
use syn::{Ident, ItemImpl, ItemStruct, Result, parse2};

use crate::{
    shared::scope_parser::ProviderStructArgs, utils::extracts::extract_struct_dependencies,
};

use super::instance_injection::generate_instance_provider_system;

pub fn handle_provider_struct(
    attr: TokenStream,
    item: TokenStream,
    _trait_name: Ident,
) -> Result<TokenStream> {
    // Parse attributes: scope = "request", init = "new", etc.
    let args = parse2::<ProviderStructArgs>(attr)?;

    let scope = args.scope;
    let init_method = args.init;

    // Get struct definition from either args (old syntax) or item (new syntax)
    let (struct_attrs, impl_block) = if let Some(struct_def) = args.struct_def {
        // Old syntax: #[provider_struct(pub struct Foo { ... })]
        // In this case, item should be impl block
        let impl_block = parse2::<ItemImpl>(item)?;
        (struct_def, impl_block)
    } else {
        // New syntax: #[provider_struct] pub struct Foo { ... }
        // Parse struct from item, create empty impl block
        let struct_attrs = parse2::<ItemStruct>(item)?;
        let struct_name = &struct_attrs.ident;
        let empty_impl: ItemImpl = syn::parse_quote! {
            impl #struct_name {}
        };
        (struct_attrs, empty_impl)
    };

    let mut dependencies = extract_struct_dependencies(&struct_attrs)?;

    // If init method is specified, disable backward compat mode
    // Move all fields to owned_fields if they're currently in fields (backward compat mode)
    if init_method.is_some() {
        if dependencies.owned_fields.is_empty() && !dependencies.fields.is_empty() {
            // We're in backward compat mode (no #[inject], all fields treated as DI)
            // But with init, all fields should be owned (init method handles them)
            dependencies.owned_fields = dependencies
                .fields
                .drain(..)
                .map(|(name, ty, _)| (name, ty, None))
                .collect();
        }
    }

    dependencies.init_method = init_method;

    let expanded =
        generate_instance_provider_system(&struct_attrs, &impl_block, &dependencies, scope)?;

    Ok(expanded)
}
