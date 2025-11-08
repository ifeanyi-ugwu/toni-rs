use proc_macro::TokenStream;
use quote::quote;
use syn::{parse_macro_input, punctuated::Punctuated, Ident, Token, Item};

/// Attribute macro for applying interceptors to a route handler method or controller impl block
///
/// # Example - Method level
/// ```rust
/// #[use_interceptors(TimingInterceptor, LoggingInterceptor)]
/// #[get("/users")]
/// fn find_all(&self, req: HttpRequest) -> HttpResponse {
///     // ...
/// }
/// ```
///
/// # Example - Controller level
/// ```rust
/// #[use_interceptors(LoggingInterceptor)]  // Applies to ALL methods
/// #[controller("/api")]
/// impl MyController {
///     // All methods get LoggingInterceptor
/// }
/// ```
pub fn use_interceptors_impl(attr: TokenStream, item: TokenStream) -> TokenStream {
    // Parse the list of interceptor types
    let interceptors = parse_macro_input!(attr with Punctuated::<Ident, Token![,]>::parse_terminated);
    let interceptor_list: Vec<_> = interceptors.iter().collect();

    // Try to parse as either a method or an impl block
    let item_parsed: Item = parse_macro_input!(item as Item);

    let output = match item_parsed {
        Item::Fn(method) => {
            // Method-level: wrap the function
            quote! {
                #[toni::toni_interceptors(#(#interceptor_list),*)]
                #method
            }
        }
        Item::Impl(impl_block) => {
            // Controller-level: wrap the impl block
            quote! {
                #[toni::toni_interceptors(#(#interceptor_list),*)]
                #impl_block
            }
        }
        _ => {
            // Fallback: just add the marker attribute
            quote! {
                #[toni::toni_interceptors(#(#interceptor_list),*)]
                #item_parsed
            }
        }
    };

    output.into()
}
