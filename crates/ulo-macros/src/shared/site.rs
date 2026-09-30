//! A call on the `ulo::__di::Site` of an injection site's type.

use proc_macro2::TokenStream;
use quote::quote;
use syn::Type;

/// `call` on the `ulo::__di::Site` of `ty`, with its rungs in scope. Autoref picks the rung by the
/// type the compiler sees, so what the key, the answer taken, the value read and the `#[inject(K)]`
/// check are follows the type, an alias or a renamed `Arc` included.
pub fn site_call(ty: &Type, call: TokenStream) -> TokenStream {
    quote! {{
        #[allow(unused_imports)]
        use ::ulo::__di::{
            SiteCollection as _, SiteExtension as _, SiteObject as _, SiteShared as _,
            SiteValueVec as _,
        };
        (&&&::ulo::__di::Site::<#ty>::new()).#call
    }}
}
