use proc_macro::TokenStream;
use quote::quote;
use syn::{parse_macro_input, punctuated::Punctuated, Ident, Token, Item};

/// Attribute macro for applying guards to a route handler method or controller impl block
///
/// # Example - Method level
/// ```rust
/// #[use_guards(AuthGuard, RoleGuard)]
/// #[get("/admin")]
/// fn admin_panel(&self, req: HttpRequest) -> HttpResponse {
///     // ...
/// }
/// ```
///
/// # Example - Controller level
/// ```rust
/// #[use_guards(AuthGuard)]  // Applies to ALL methods
/// #[controller("/api")]
/// impl MyController {
///     // All methods get AuthGuard
/// }
/// ```
pub fn use_guards_impl(attr: TokenStream, item: TokenStream) -> TokenStream {
    // Parse the list of guard types
    let guards = parse_macro_input!(attr with Punctuated::<Ident, Token![,]>::parse_terminated);
    let guard_list: Vec<_> = guards.iter().collect();

    // Try to parse as either a method or an impl block
    let item_parsed: Item = parse_macro_input!(item as Item);

    let output = match item_parsed {
        Item::Fn(method) => {
            quote! {
                #[toni::toni_guards(#(#guard_list),*)]
                #method
            }
        }
        Item::Impl(impl_block) => {
            quote! {
                #[toni::toni_guards(#(#guard_list),*)]
                #impl_block
            }
        }
        _ => {
            quote! {
                #[toni::toni_guards(#(#guard_list),*)]
                #item_parsed
            }
        }
    };

    output.into()
}
