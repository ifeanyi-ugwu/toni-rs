use proc_macro2::TokenStream;
use quote::quote;
use syn::{ItemImpl, ItemStruct};

pub fn generate_output(
    struct_attrs: ItemStruct,
    impl_block: ItemImpl,
    providers: Vec<TokenStream>,
    manager: TokenStream,
) -> TokenStream {
    quote! {
        #struct_attrs
        #impl_block
        #(#providers)*
        #manager
    }
}
