//! The argument of `#[inject(K)]`: a key type, checked against what the field or parameter holds.
//!
//! A field names what a slot holds through its shape: `V` or `Arc<V>` for a slot holding a sized
//! type, `Arc<dyn Trait>` for one holding a trait object, and `Vec<Arc<dyn Trait>>` for a
//! collection. The held type is read off the field's shape here and checked against `K::Value` at
//! the key.

use proc_macro2::TokenStream;
use quote::quote_spanned;
use syn::spanned::Spanned;
use syn::{Attribute, Error, Result, Type};

use crate::utils::extracts::{extract_arc_inner, extract_vec_arc_dyn_inner};

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
    let held = held_type(filled);
    Ok(Some(match key {
        Type::TraitObject(_) => quote_spanned!(key.span()=> {
            let _: ::std::marker::PhantomData<#held> = ::std::marker::PhantomData::<#key>;
            ::ulo::di::token_of::<#key>()
        }),
        _ => quote_spanned!(key.span()=> ::ulo::__di::inject_key::<#key, #held>()),
    }))
}

/// What the slot a field of type `filled` reads must hold: the trait object for `Vec<Arc<dyn Trait>>`,
/// `T` for `Arc<T>`, and the field's own type otherwise. These are the shapes the injection code
/// reads a collection, a shared instance and a trait-object slot by.
fn held_type(filled: &Type) -> Type {
    extract_vec_arc_dyn_inner(filled)
        .or_else(|| extract_arc_inner(filled))
        .unwrap_or_else(|| filled.clone())
}
