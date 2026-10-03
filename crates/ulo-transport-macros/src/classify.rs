use proc_macro2::{Span, TokenStream};
use quote::{quote, quote_spanned};
use syn::{Data, DeriveInput, Ident};

/// The kind words `#[classify(..)]` takes, and the `ErrorKind` variant each names.
pub(crate) const KINDS: &[(&str, &str)] = &[
    ("bad_request", "BadRequest"),
    ("unauthorized", "Unauthorized"),
    ("forbidden", "Forbidden"),
    ("not_found", "NotFound"),
    ("conflict", "Conflict"),
    ("unprocessable", "Unprocessable"),
    ("too_many_requests", "TooManyRequests"),
    ("timeout", "Timeout"),
    ("unavailable", "Unavailable"),
    ("unimplemented", "Unimplemented"),
    ("internal", "Internal"),
];

/// One `#[classify(<kind>)]`: the `ErrorKind` variant it names, with the word's span.
#[derive(Clone)]
pub(crate) struct KindArg {
    pub(crate) variant: Ident,
    pub(crate) span: Span,
}

impl KindArg {
    /// `::ulo_transport::ErrorKind::<Variant>`, spanned at the kind word, so a type error in the
    /// generated impl points at the attribute that chose it.
    fn path(&self) -> TokenStream {
        let variant = &self.variant;
        quote_spanned!(self.span=> ::ulo_transport::ErrorKind::#variant)
    }
}

/// `impl ::ulo_transport::Classify for <Type>`, its `classify` matching each variant to its kind.
pub(crate) fn expand(input: TokenStream) -> syn::Result<TokenStream> {
    let input: DeriveInput = syn::parse2(input)?;
    let name = &input.ident;
    let default = kind_of(&input.attrs)?;

    let body = match &input.data {
        Data::Struct(_) => match &default {
            Some(kind) => kind.path(),
            None => {
                return Err(syn::Error::new_spanned(
                    name,
                    "`Classify` needs a kind: add `#[classify(<kind>)]` to the struct, e.g. `#[classify(not_found)]`",
                ));
            }
        },
        Data::Enum(data) => {
            let mut arms = Vec::with_capacity(data.variants.len());
            let mut missing: Vec<syn::Error> = Vec::new();
            for variant in &data.variants {
                let own = kind_of(&variant.attrs)?;
                let Some(kind) = own.or_else(|| default.clone()) else {
                    let error = syn::Error::new_spanned(
                        &variant.ident,
                        format!(
                            "variant `{}` has no kind: add `#[classify(<kind>)]` to it, or one to the enum as the default",
                            variant.ident
                        ),
                    );
                    missing.push(error);
                    continue;
                };
                let ident = &variant.ident;
                let path = kind.path();
                arms.push(quote!(Self::#ident { .. } => #path,));
            }
            let missing = missing.into_iter().reduce(|mut all, next| {
                all.combine(next);
                all
            });
            if let Some(error) = missing {
                return Err(error);
            }
            quote!(match *self { #(#arms)* })
        }
        Data::Union(data) => {
            return Err(syn::Error::new_spanned(
                data.union_token,
                "`Classify` derives on a struct or an enum, not a union",
            ));
        }
    };

    let (impl_generics, ty_generics, where_clause) = input.generics.split_for_impl();
    Ok(quote! {
        impl #impl_generics ::ulo_transport::Classify for #name #ty_generics #where_clause {
            fn classify(&self) -> ::ulo_transport::ErrorKind {
                #body
            }
        }
    })
}

/// The `#[classify(..)]` among `attrs`, if any; a word outside [`KINDS`] or a second attribute is
/// a span error.
pub(crate) fn kind_of(attrs: &[syn::Attribute]) -> syn::Result<Option<KindArg>> {
    let mut found: Option<KindArg> = None;
    for attr in attrs {
        if !attr.path().is_ident("classify") {
            continue;
        }
        if found.is_some() {
            return Err(syn::Error::new_spanned(attr, "a second `#[classify(..)]`: an item takes one kind"));
        }
        let word: Ident = attr.parse_args().map_err(|error| {
            syn::Error::new(error.span(), format!("`#[classify(..)]` takes one kind word: {}", words()))
        })?;
        let text = word.to_string();
        let Some((_, variant)) = KINDS.iter().find(|(kind, _)| *kind == text) else {
            return Err(syn::Error::new(word.span(), format!("unknown kind `{text}`; expected one of {}", words())));
        };
        found = Some(KindArg { variant: Ident::new(variant, word.span()), span: word.span() });
    }
    Ok(found)
}

fn words() -> String {
    KINDS.iter().map(|(kind, _)| format!("`{kind}`")).collect::<Vec<_>>().join(", ")
}
