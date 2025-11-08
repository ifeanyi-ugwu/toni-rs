use proc_macro::TokenStream;
use quote::quote;
use syn::{parse_macro_input, punctuated::Punctuated, Ident, Token, Item};

/// Attribute macro for applying pipes to a route handler method or controller impl block
///
/// # Example - Method level
/// ```rust
/// #[use_pipes(ValidationPipe, TransformPipe)]
/// #[post("/users")]
/// fn create_user(&self, req: HttpRequest) -> HttpResponse {
///     // ...
/// }
/// ```
///
/// # Example - Controller level
/// ```rust
/// #[use_pipes(ValidationPipe)]  // Applies to ALL methods
/// #[controller("/api")]
/// impl MyController {
///     // All methods get ValidationPipe
/// }
/// ```
pub fn use_pipes_impl(attr: TokenStream, item: TokenStream) -> TokenStream {
    // Parse the list of pipe types
    let pipes = parse_macro_input!(attr with Punctuated::<Ident, Token![,]>::parse_terminated);
    let pipe_list: Vec<_> = pipes.iter().collect();

    // Try to parse as either a method or an impl block
    let item_parsed: Item = parse_macro_input!(item as Item);

    let output = match item_parsed {
        Item::Fn(method) => {
            quote! {
                #[toni::toni_pipes(#(#pipe_list),*)]
                #method
            }
        }
        Item::Impl(impl_block) => {
            quote! {
                #[toni::toni_pipes(#(#pipe_list),*)]
                #impl_block
            }
        }
        _ => {
            quote! {
                #[toni::toni_pipes(#(#pipe_list),*)]
                #item_parsed
            }
        }
    };

    output.into()
}
