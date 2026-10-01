//! `#[injectable]`: the struct form, whose fields are sites, and the impl form, whose
//! constructor's parameters are sites. Both write a `Construct` impl (§4, §5).

mod args;
mod impl_form;
mod struct_form;

use proc_macro2::TokenStream;
use syn::Item;

pub(crate) use args::InjectableArgs;

pub(crate) fn expand(attr: TokenStream, item: TokenStream) -> syn::Result<TokenStream> {
    let args: InjectableArgs = syn::parse2(attr)?;
    match syn::parse2::<Item>(item)? {
        Item::Struct(item) => struct_form::expand(args, item),
        Item::Impl(item) => impl_form::expand(args, item),
        other => Err(syn::Error::new_spanned(
            other,
            "#[injectable] goes on a struct, whose fields are sites, or on an impl block holding its constructor",
        )),
    }
}
