use proc_macro::TokenStream;
use quote::quote;
use syn::{parse_macro_input, punctuated::Punctuated, Ident, Token};

/// Attribute macro for applying pipes to a route handler method
///
/// # Example
/// ```rust
/// #[use_pipes(ValidationPipe, TransformPipe)]
/// #[post("/users")]
/// fn create_user(&self, req: HttpRequest) -> HttpResponse {
///     // ...
/// }
/// ```
pub fn use_pipes_impl(attr: TokenStream, item: TokenStream) -> TokenStream {
    // Parse the list of pipe types
    let pipes = parse_macro_input!(attr with Punctuated::<Ident, Token![,]>::parse_terminated);

    // Parse the method
    let method = parse_macro_input!(item as syn::ImplItemFn);

    // Create a marker attribute that the controller macro will read
    let pipe_list: Vec<_> = pipes.iter().collect();

    let output = quote! {
        #[toni::toni_pipes(#(#pipe_list),*)]
        #method
    };

    output.into()
}
