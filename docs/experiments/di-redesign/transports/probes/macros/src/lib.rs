//! X1 as a real attribute pair. `routes_like("<key>")` sits on the impl and expands first: it
//! re-emits the impl with the method attributes untouched and adds a free `const _` asserting the
//! key against `<Ty>::__FW_KEY_<name>` for every method carrying `get_like`. `get_like("<key>")`
//! sits on a method and expands after: it re-emits the method and adds the associated const.
//! Name resolution runs after both expansions, so the outer macro's const refers to items the
//! inner one wrote.
use proc_macro::TokenStream;
use quote::{format_ident, quote, quote_spanned};
use syn::{ImplItem, ItemImpl, LitStr, parse_macro_input};

#[proc_macro_attribute]
pub fn routes_like(attr: TokenStream, item: TokenStream) -> TokenStream {
    let key = parse_macro_input!(attr as LitStr);
    let item = parse_macro_input!(item as ItemImpl);
    let self_ty = &item.self_ty;
    let names: Vec<_> = item
        .items
        .iter()
        .filter_map(|i| match i {
            ImplItem::Fn(f) if f.attrs.iter().any(|a| a.path().segments.last().is_some_and(|s| s.ident == "get_like")) => {
                Some(f.sig.ident.clone())
            }
            _ => None,
        })
        .collect();
    let consts = names.iter().map(|n| format_ident!("__FW_KEY_{}", n));
    let handlers = names.iter().map(|n| n.to_string()).collect::<Vec<_>>().join(", ");
    let msg = format!("`{}` is not the key of any handler's transport in this impl (handlers: {handlers})", key.value());
    let check = quote_spanned! {key.span()=>
        const _: () = ::core::assert!(crate::key_in(#key, &[#(<#self_ty>::#consts),*]), #msg);
    };
    quote! { #item #check }.into()
}

#[proc_macro_attribute]
pub fn get_like(attr: TokenStream, item: TokenStream) -> TokenStream {
    let key = parse_macro_input!(attr as LitStr);
    let method = parse_macro_input!(item as syn::ImplItemFn);
    let name = format_ident!("__FW_KEY_{}", method.sig.ident);
    let mount = format_ident!("__fw_mount_{}", method.sig.ident);
    quote! {
        #method
        pub const #name: &'static str = #key;
        pub fn #mount() {}
    }
    .into()
}
