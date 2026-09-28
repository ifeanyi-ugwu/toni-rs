use proc_macro2::TokenStream;
use quote::quote_spanned;
use syn::{ItemStruct, spanned::Spanned};

use super::attr_is;

/// One `const` item per `#[default]` field, failing const evaluation when a `#[new]` constructor
/// builds the struct: the constructor sets every field and the default never applies. The struct
/// macro cannot see the `#[new]` method, so the item reads `__ULO_ONE_NEW_PER_TYPE`, the const
/// `#[new]` emits over the blanket default in `ulo::__construct::CtorBridge`. It is spanned at the
/// attribute's path, which puts the error on the attribute to remove.
pub fn default_beside_new(struct_def: &ItemStruct) -> TokenStream {
    let name = &struct_def.ident;
    struct_def
        .fields
        .iter()
        .filter_map(|field| {
            let attr = field.attrs.iter().find(|attr| attr_is(attr, "default"))?;
            let field_name = field.ident.as_ref()?;
            let message = format!(
                "`#[default]` on `{name}::{field_name}` never applies: `{name}` is built by its \
                 `#[new]` constructor, which sets every field; remove `#[default]`, or remove \
                 `#[new]` to build `{name}` from its fields"
            );
            Some(quote_spanned! {attr.path().span()=>
                const _: () = {
                    #[allow(unused_imports)]
                    use ::ulo::__construct::CtorBridge as _;
                    if <#name>::__ULO_ONE_NEW_PER_TYPE.is_some() {
                        ::core::panic!(#message);
                    }
                };
            })
        })
        .collect()
}
