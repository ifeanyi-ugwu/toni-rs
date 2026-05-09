use proc_macro::TokenStream;
use proc_macro2::TokenStream as TokenStream2;
use quote::quote;
use syn::{Attribute, Data, DataEnum, DataStruct, DeriveInput, Ident, parse_macro_input};

/// `#[derive(AppError)]` — generates `impl ::toni::AppError`, deriving the
/// `kind()` method from a `#[app_error(...)]` attribute.
///
/// On a struct: `#[app_error(NotFound)]` at the top level.
/// On an enum: `#[app_error(NotFound)]` on each variant. A top-level
/// attribute on the enum acts as the default for untagged variants;
/// missing tags otherwise default to `ErrorKind::Internal`.
pub fn derive_app_error(input: TokenStream) -> TokenStream {
    let input = parse_macro_input!(input as DeriveInput);
    let name = &input.ident;
    let (impl_generics, ty_generics, where_clause) = input.generics.split_for_impl();

    let kind_body = match &input.data {
        Data::Struct(data) => match struct_kind_body(&input.attrs, data) {
            Ok(ts) => ts,
            Err(e) => return e.to_compile_error().into(),
        },
        Data::Enum(data) => match enum_kind_body(&input.attrs, data) {
            Ok(ts) => ts,
            Err(e) => return e.to_compile_error().into(),
        },
        Data::Union(_) => {
            return syn::Error::new_spanned(
                &input,
                "AppError cannot be derived on unions",
            )
            .to_compile_error()
            .into();
        }
    };

    let expanded = quote! {
        #[automatically_derived]
        impl #impl_generics ::toni::AppError for #name #ty_generics #where_clause {
            fn kind(&self) -> ::toni::ErrorKind {
                #kind_body
            }
        }
    };

    expanded.into()
}

fn struct_kind_body(attrs: &[Attribute], _data: &DataStruct) -> syn::Result<TokenStream2> {
    let kind = parse_kind_attr(attrs)?.unwrap_or_else(default_kind);
    Ok(quote! { ::toni::ErrorKind::#kind })
}

fn enum_kind_body(top_attrs: &[Attribute], data: &DataEnum) -> syn::Result<TokenStream2> {
    let fallback = parse_kind_attr(top_attrs)?.unwrap_or_else(default_kind);

    let arms = data
        .variants
        .iter()
        .map(|variant| {
            let ident = &variant.ident;
            let kind = parse_kind_attr(&variant.attrs)?.unwrap_or_else(|| fallback.clone());
            let pattern = match &variant.fields {
                syn::Fields::Named(_) => quote! { Self::#ident { .. } },
                syn::Fields::Unnamed(_) => quote! { Self::#ident(..) },
                syn::Fields::Unit => quote! { Self::#ident },
            };
            Ok(quote! { #pattern => ::toni::ErrorKind::#kind, })
        })
        .collect::<syn::Result<Vec<_>>>()?;

    Ok(quote! {
        match self {
            #(#arms)*
        }
    })
}

fn default_kind() -> Ident {
    Ident::new("Internal", proc_macro2::Span::call_site())
}

/// Parse `#[app_error(KIND)]` from a list of attributes. Returns the
/// inner ident (a variant of `ErrorKind`) or `None` if the attribute is
/// absent.
fn parse_kind_attr(attrs: &[Attribute]) -> syn::Result<Option<Ident>> {
    let mut found: Option<Ident> = None;
    for attr in attrs {
        if !attr.path().is_ident("app_error") {
            continue;
        }
        if found.is_some() {
            return Err(syn::Error::new_spanned(
                attr,
                "duplicate #[app_error(...)] attribute",
            ));
        }
        let kind: Ident = attr.parse_args().map_err(|_| {
            syn::Error::new_spanned(
                attr,
                "expected #[app_error(KIND)] where KIND is an ErrorKind variant, e.g. #[app_error(NotFound)]",
            )
        })?;
        found = Some(kind);
    }
    Ok(found)
}
