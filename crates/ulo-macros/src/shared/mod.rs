//! Helpers every macro uses: the path to the core, attribute handling, and dependency emission.

pub(crate) mod attrs;
pub(crate) mod dependencies;
pub(crate) mod scope;

use proc_macro2::{Span, TokenStream};
use quote::{quote, quote_spanned};
use syn::Ident;

/// The path generated code names the core by. The macros are used through `ulo`'s re-exports,
/// so `::ulo` resolves wherever they expand.
pub(crate) fn ulo() -> TokenStream {
    ulo_at(Span::call_site())
}

/// [`ulo`] spanned at `span`, for a path whose span a diagnostic prints.
pub(crate) fn ulo_at(span: Span) -> TokenStream {
    quote_spanned!(span=> ::ulo)
}

pub(crate) use ulo_handler_codegen::util::{check_factory_params, combine};

/// The `Dependencies` parameter of the generated `Construct::dependencies`. Mixed-site, so no
/// field or parameter name the user writes can shadow it.
pub(crate) fn dependencies_param() -> Ident {
    Ident::new("d", Span::mixed_site())
}

/// The `Resolver` parameter of the generated `Construct::construct`. Mixed-site for the same
/// reason as [`dependencies_param`]: a constructor parameter named `r` would otherwise shadow it
/// between two reads.
pub(crate) fn resolver_param() -> Ident {
    Ident::new("r", Span::mixed_site())
}

/// The `Construct` impl both forms of `#[injectable]` write: scope, optional `CONSTRUCT_TIMEOUT`,
/// `dependencies`, `construct` and the probing `hooks`.
pub(crate) struct ConstructImpl<'a> {
    pub(crate) self_ty: &'a syn::Type,
    pub(crate) generics: &'a syn::Generics,
    /// `::ulo::scope::Auto` when no scope is written.
    pub(crate) scope: TokenStream,
    pub(crate) timeout: Option<&'a syn::Expr>,
    /// The body of `fn dependencies(d: &mut ::ulo::Dependencies)`, assertions included.
    pub(crate) dependencies: TokenStream,
    /// The body of `async fn construct(r: &::ulo::Resolver<'_>) -> Result<Self, ::ulo::ConstructError>`.
    pub(crate) construct: TokenStream,
    /// Where `construct` is spanned: the constructor for the impl form, so a future that is not
    /// `Send` is reported at the user's function; the struct's name for the struct form.
    pub(crate) construct_span: Span,
}

impl ConstructImpl<'_> {
    pub(crate) fn emit(&self) -> TokenStream {
        let ulo = ulo();
        let ConstructImpl { self_ty, generics, scope, timeout, dependencies, construct, construct_span } = self;
        let (impl_generics, _, where_clause) = generics.split_for_impl();
        let d = dependencies_param();
        let r = resolver_param();
        let timeout = timeout.map(|expr| {
            quote_spanned! {syn::spanned::Spanned::span(expr)=>
                const CONSTRUCT_TIMEOUT: #ulo::Bound = #ulo::Bound::After(#expr);
            }
        });
        let construct_fn = quote_spanned! {*construct_span=>
            #[allow(unused_variables)]
            async fn construct(#r: &#ulo::Resolver<'_>) -> ::core::result::Result<Self, #ulo::ConstructError> {
                #construct
            }
        };
        quote! {
            impl #impl_generics #ulo::Construct for #self_ty #where_clause {
                type Scope = #scope;

                #timeout

                #[allow(unused_variables)]
                fn dependencies(#d: &mut #ulo::Dependencies) {
                    #dependencies
                }

                #construct_fn

                fn hooks(h: &mut #ulo::Hooks<Self>) {
                    #ulo::hooks!(h);
                }
            }
        }
    }
}
