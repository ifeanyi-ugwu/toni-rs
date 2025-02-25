use proc_macro2::TokenStream;
use quote::quote;
use syn::{ItemImpl, ItemStruct};

pub fn generate_output(
    struct_attrs: ItemStruct,
    impl_block: ItemImpl,
    controllers: Vec<TokenStream>,
    manager: TokenStream,
) -> TokenStream {
    quote! {
        #[allow(dead_code)]
        #struct_attrs
        #[allow(dead_code)]
        #impl_block
        #(#controllers)*
        #manager
    }
}
