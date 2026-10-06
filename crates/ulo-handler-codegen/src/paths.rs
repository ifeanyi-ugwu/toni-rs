use proc_macro2::TokenStream;
use quote::quote;

/// The paths generated code names its crates by. A transport's macro crate builds one: the core
/// is always `::ulo`, which `#[routes]` and `__enhancer_specs!` name too, so an application
/// depends on `ulo` directly. What the generated call reads besides the core is reached through
/// the transport crate, so an application needs no direct dependency on `ulo-transport` for what
/// a handler attribute generates.
#[derive(Clone)]
pub struct Paths {
    /// `::ulo`.
    pub core: TokenStream,
    /// The path whose `__private` module supplies what the generated call names: `Param` and
    /// `controller`, which extract each parameter and resolve the controller, and the reply probe
    /// `IntoReplyProbe` with its arms `ViaCallError`, `ViaBoxError` and `ViaValue`.
    ///
    /// [`Paths::new`] points it at `ulo-transport` as the transport crate re-exports it,
    /// `::ulo_http::__private::transport`, whose probe converts a value through `IntoReply<T>`. A
    /// transport whose answers that probe cannot convert points it at a crate of its own carrying
    /// the same names: the RPC attributes at `::ulo_rpc`, whose `__private` re-exports `Param` and
    /// `controller` from `ulo-transport` beside its own probe, which also answers a bare
    /// `Serialize` value and a stream; the WebSocket attribute at `::ulo_ws::__private::reply`,
    /// whose probe answers `()`, a `Serialize` value, a `Frame` and a stream.
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
