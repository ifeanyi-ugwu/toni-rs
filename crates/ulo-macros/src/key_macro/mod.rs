use proc_macro2::TokenStream;
use quote::quote;
use syn::{DeriveInput, Result, parse2};

/// `impl Key for T { type Value = T; }`: the type becomes writable in a key position, naming its
/// own slot.
pub fn derive_key(input: TokenStream) -> Result<TokenStream> {
    let input = parse2::<DeriveInput>(input)?;
    let name = &input.ident;
    let (impl_generics, type_generics, where_clause) = input.generics.split_for_impl();
    let mut predicates = where_clause
        .map(|clause| clause.predicates.iter().cloned().collect::<Vec<_>>())
        .unwrap_or_default();
    predicates.push(syn::parse_quote!(Self: 'static));
    Ok(quote! {
        impl #impl_generics ::ulo::di::Key for #name #type_generics
        where
            #(#predicates),*
        {
            type Value = Self;
        }
    })
}
