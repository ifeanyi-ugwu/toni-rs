use proc_macro::TokenStream;
use quote::quote;
use syn::{parse_macro_input, punctuated::Punctuated, Ident, Token};

/// Attribute macro for applying interceptors to a route handler method
///
/// # Example
/// ```rust
/// #[use_interceptors(TimingInterceptor, LoggingInterceptor)]
/// #[get("/users")]
/// fn find_all(&self, req: HttpRequest) -> HttpResponse {
///     // ...
/// }
/// ```
pub fn use_interceptors_impl(attr: TokenStream, item: TokenStream) -> TokenStream {
    // Parse the list of interceptor types
    let interceptors = parse_macro_input!(attr with Punctuated::<Ident, Token![,]>::parse_terminated);

    // Parse the method
    let method = parse_macro_input!(item as syn::ImplItemFn);

    // Create a marker attribute that the controller macro will read
    let interceptor_list: Vec<_> = interceptors.iter().collect();

    let output = quote! {
        #[toni::toni_interceptors(#(#interceptor_list),*)]
        #method
    };

    output.into()
}
