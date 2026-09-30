//! The argument of `#[inject(K)]`: a key type, checked against what the field or parameter holds.
//!
//! A field names what a slot holds through its type: `V` or `Arc<V>` for a slot holding a sized
//! type, `Arc<dyn Trait>` for one holding a trait object, and `Vec<Arc<dyn Trait>>` for a
//! collection. The held type is read from the field's type by its `ulo::__di::Site` and checked
//! against `K::Value` at the key.

use proc_macro2::TokenStream;
use quote::quote_spanned;
use syn::spanned::Spanned;
use syn::{Attribute, Error, Result, Type};

/// The expression producing the key `attr` names for a field or parameter of type `filled`, or
/// `None` for bare `#[inject]`.
pub fn inject_key(attr: &Attribute, filled: &Type) -> Result<Option<TokenStream>> {
    if attr.meta.require_path_only().is_ok() {
        return Ok(None);
    }
    let args = attr.meta.require_list()?.tokens.clone();
    if syn::parse2::<syn::LitStr>(args.clone()).is_ok() {
        return Err(Error::new_spanned(
            args,
            "a key is a type, not a string: declare a marker with `key!(pub Name: T)` and write \
             `#[inject(Name)]`",
        ));
    }
    let key: Type = syn::parse2(args.clone()).map_err(|_| {
        Error::new_spanned(
            &args,
            "`#[inject(..)]` takes a key type: a marker declared with `key!`, a type implementing \
             `Key`, or `dyn Trait`",
        )
    })?;
    let check = match key {
        Type::TraitObject(_) => quote_spanned!(key.span()=> inject_dyn::<#key>()),
        _ => quote_spanned!(key.span()=> inject_key::<#key>()),
    };
    Ok(Some(crate::shared::site::site_call(filled, check)))
}
