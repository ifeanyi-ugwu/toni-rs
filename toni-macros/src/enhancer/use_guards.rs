use proc_macro::TokenStream;
use quote::quote;
use syn::{parse_macro_input, punctuated::Punctuated, Ident, Token};

/// Attribute macro for applying guards to a route handler method
///
/// # Example
/// ```rust
/// #[use_guards(AuthGuard, RoleGuard)]
/// #[get("/admin")]
/// fn admin_panel(&self, req: HttpRequest) -> HttpResponse {
///     // ...
/// }
/// ```
pub fn use_guards_impl(attr: TokenStream, item: TokenStream) -> TokenStream {
    // Parse the list of guard types
    let guards = parse_macro_input!(attr with Punctuated::<Ident, Token![,]>::parse_terminated);

    // Parse the method
    let method = parse_macro_input!(item as syn::ImplItemFn);

    // Create a marker attribute that the controller macro will read
    let guard_list: Vec<_> = guards.iter().collect();

    let output = quote! {
        #[toni::toni_guards(#(#guard_list),*)]
        #method
    };

    output.into()
}
