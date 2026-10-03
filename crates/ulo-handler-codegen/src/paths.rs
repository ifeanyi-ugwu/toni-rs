use proc_macro2::TokenStream;
use quote::quote;

/// The paths generated code names its crates by. A transport's macro crate builds one: the core
/// is always `::ulo`, which `#[routes]` and `__enhancer_specs!` name too, so an application
/// depends on `ulo` directly. The transport-neutral crate is reached through the transport
/// crate's re-export, so an application needs no direct dependency on `ulo-transport` for what
/// a handler attribute generates.
#[derive(Clone)]
pub struct Paths {
    /// `::ulo`.
    pub core: TokenStream,
    /// `ulo-transport` as the transport crate re-exports it: `::ulo_http::__private::transport`.
    pub transport: TokenStream,
    /// The transport crate itself: `::ulo_http`.
    pub this: TokenStream,
    /// The transport marker type: `::ulo_http::Http`.
    pub marker: TokenStream,
}

impl Paths {
    /// `::ulo`, with the transport crate's `::<krate>`, its `__private::transport` re-export and its
    /// marker `::<krate>::<Marker>`.
    pub fn new(krate: &str, marker: &str) -> Self {
        let krate = syn::Ident::new(krate, proc_macro2::Span::call_site());
        let marker = syn::Ident::new(marker, proc_macro2::Span::call_site());
        Paths {
            core: quote!(::ulo),
            transport: quote!(::#krate::__private::transport),
            this: quote!(::#krate),
            marker: quote!(::#krate::#marker),
        }
    }
}
