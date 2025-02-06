use proc_macro2::Span;
use syn::{parse_quote, punctuated::Punctuated, Ident, Path, PathArguments, PathSegment, Type};

pub fn create_type_reference(name: &str, arc_type: bool, box_type: bool, dyn_type: bool) -> Type {
    let mut base_type = create_base_type(name);

    if dyn_type {
        base_type = wrap_in_dyn(base_type);
    }

    if box_type {
        base_type = wrap_in_box(base_type);
    }

    if arc_type {
        base_type = wrap_in_arc(base_type);
    }

    base_type
}

fn create_base_type(name: &str) -> Type {
  Type::Path(syn::TypePath {
      qself: None,
      path: Path {
          leading_colon: Some(Default::default()),
          segments: Punctuated::from_iter(vec![
              PathSegment::from(Ident::new("tonirs_core", Span::mixed_site())),
              PathSegment::from(Ident::new("traits_helpers", Span::mixed_site())),
              PathSegment::from(Ident::new(name, Span::mixed_site())),
          ]),
      },
  })
}

fn wrap_in_dyn(base_type: Type) -> Type {
  Type::TraitObject(syn::TypeTraitObject {
      dyn_token: Some(parse_quote! {dyn}),
      bounds: Punctuated::from_iter(vec![syn::TypeParamBound::Trait(syn::TraitBound {
          paren_token: None,
          modifier: syn::TraitBoundModifier::None,
          lifetimes: None,
          path: match base_type {
              Type::Path(type_path) => type_path.path,
              _ => panic!("Expected Type::Path for base_type"),
          },
      })]),
  })
}

fn wrap_in_box(base_type: Type) -> Type {
  Type::Path(syn::TypePath {
      qself: None,
      path: Path {
          leading_colon: None,
          segments: Punctuated::from_iter(vec![PathSegment {
              ident: syn::Ident::new("Box", proc_macro2::Span::mixed_site()),
              arguments: PathArguments::AngleBracketed(syn::AngleBracketedGenericArguments {
                  colon2_token: None,
                  lt_token: parse_quote! {<},
                  args: Punctuated::from_iter(vec![syn::GenericArgument::Type(base_type)]),
                  gt_token: parse_quote! {>},
              }),
          }]),
      },
  })
}

fn wrap_in_arc(base_type: Type) -> Type {
  Type::Path(syn::TypePath {
      qself: None,
      path: Path {
          leading_colon: Some(Default::default()),
          segments: Punctuated::from_iter(vec![
              PathSegment::from(Ident::new("std", Span::mixed_site())),
              PathSegment::from(Ident::new("sync", Span::mixed_site())),
              PathSegment {
                  ident: syn::Ident::new("Arc", proc_macro2::Span::mixed_site()),
                  arguments: PathArguments::AngleBracketed(syn::AngleBracketedGenericArguments {
                      colon2_token: None,
                      lt_token: parse_quote! {<},
                      args: Punctuated::from_iter(vec![syn::GenericArgument::Type(base_type)]),
                      gt_token: parse_quote! {>},
                  }),
              },
          ]),
      },
  })
}
