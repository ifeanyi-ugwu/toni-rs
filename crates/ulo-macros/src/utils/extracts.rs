use std::collections::HashSet;

use proc_macro2::TokenStream;
use quote::quote;
use syn::{
    Error, Expr, FnArg, Ident, ImplItemFn, ItemImpl, ItemStruct, LitStr, Pat, Result, Type,
    TypePath, TypeReference, spanned::Spanned,
};

use crate::shared::attr_is;
use crate::shared::dependency_info::{DependencyInfo, DependencySource};

pub fn extract_controller_prefix(impl_block: &ItemImpl) -> Result<String> {
    impl_block
        .attrs
        .iter()
        .find(|attr| attr_is(attr, "controller"))
        .map(|attr| attr.parse_args::<LitStr>().map(|lit| lit.value()))
        .transpose()
        .map(|opt| opt.unwrap_or_default())
}

pub fn extract_struct_dependencies(struct_attrs: &ItemStruct) -> Result<DependencyInfo> {
    let unique_types = HashSet::new();
    let mut fields = Vec::new();
    let mut owned_fields = Vec::new();

    // Check if struct is empty
    if struct_attrs.fields.is_empty() {
        return Ok(DependencyInfo {
            fields,
            owned_fields,
            unique_types,
            source: DependencySource::None,
        });
    }

    // Check if ANY field has DI annotations (#[inject] or #[default])
    let has_di_annotations = struct_attrs.fields.iter().any(|field| {
        field
            .attrs
            .iter()
            .any(|attr| attr_is(attr, "inject") || attr_is(attr, "default"))
    });

    for field in &struct_attrs.fields {
        let field_ident = field
            .ident
            .as_ref()
            .ok_or_else(|| syn::Error::new_spanned(field, "Unnamed struct fields not supported"))?;

        let inject_attr = extract_inject_attr(field)?;
        let has_inject = inject_attr.is_some();

        // Check for #[default] attribute
        let default_expr = extract_default_attr(field)?;

        // Validate: can't have both #[inject] and #[default]
        if has_inject && default_expr.is_some() {
            return Err(syn::Error::new_spanned(
                field,
                "Field cannot have both #[inject] and #[default] attributes. \
                 Use #[inject] for DI dependencies or #[default(...)] for owned fields, not both.",
            ));
        }

        if has_di_annotations {
            // Explicit annotation mode: #[inject] means DI, no annotation or #[default] means owned
            if has_inject {
                // This is a DI dependency
                let full_type = field.ty.clone();

                // Determine the lookup token
                let lookup_token_expr = if let Some(custom_token_expr) = inject_attr.unwrap() {
                    custom_token_expr
                } else {
                    // #[inject] - use type-based token
                    extract_type_token(&field.ty)?
                };

                fields.push((field_ident.clone(), full_type, lookup_token_expr));
            } else {
                // This is an owned field - will use Default::default() if no #[default(...)]
                owned_fields.push((field_ident.clone(), field.ty.clone(), default_expr));
            }
        } else {
            // No annotations - DefaultFallback mode: all fields are owned and use Default trait
            owned_fields.push((field_ident.clone(), field.ty.clone(), None));
        }
    }

    // Determine source
    let source = if has_di_annotations {
        DependencySource::Annotations
    } else {
        DependencySource::DefaultFallback
    };

    Ok(DependencyInfo {
        fields,
        owned_fields,
        unique_types,
        source,
    })
}

/// Extract the #[default(expr)] attribute from a field
fn extract_default_attr(field: &syn::Field) -> Result<Option<Expr>> {
    for attr in &field.attrs {
        if attr_is(attr, "default") {
            let expr: Expr = attr.parse_args()?;
            return Ok(Some(expr));
        }
    }
    Ok(None)
}

/// The `#[inject]` attribute on a field: `None` without one, `Some(None)` for bare `#[inject]`,
/// and `Some(Some(key))` for `#[inject(K)]`.
fn extract_inject_attr(field: &syn::Field) -> Result<Option<Option<TokenStream>>> {
    for attr in &field.attrs {
        if attr_is(attr, "inject") {
            return crate::shared::inject_key::inject_key(attr, &field.ty).map(Some);
        }
    }
    Ok(None)
}

pub fn extract_ident_from_type(ty: &Type) -> Result<&Ident> {
    if let Type::Reference(TypeReference { elem, .. }) = ty {
        if let Type::Path(TypePath { path, .. }) = &**elem {
            if let Some(segment) = path.segments.last() {
                return Ok(&segment.ident);
            }
        }
    }
    if let Type::Path(TypePath { path, .. }) = ty {
        if let Some(segment) = path.segments.last() {
            return Ok(&segment.ident);
        }
    }
    Err(Error::new(ty.span(), "Invalid type"))
}

/// Extracts a type token expression: the key the site of type `ty` reads, computed from the type
/// the compiler sees: `T` for `Arc<T>`, the element type for a collection `Vec<Arc<T>>`, the type
/// itself otherwise. The compiler canonicalizes the spelling, so qualified paths, aliases, a
/// renamed `Arc` and generic parameters all produce the name the registration side uses.
pub fn extract_type_token(ty: &Type) -> Result<TokenStream> {
    // Handle references by unwrapping to inner type
    let actual_type = if let Type::Reference(TypeReference { elem, .. }) = ty {
        &**elem
    } else {
        ty
    };

    if let Type::Path(_) = actual_type {
        return Ok(crate::shared::site::site_call(
            actual_type,
            quote! { key() },
        ));
    }

    Err(syn::Error::new_spanned(
        ty,
        "Expected a type path (e.g., MyType or MyType<T>)",
    ))
}

/// Returns the inner type if `ty` is `Arc<T>`, a trait object included, otherwise `None`.
pub fn extract_arc_inner(ty: &Type) -> Option<Type> {
    let Type::Path(syn::TypePath { path, .. }) = ty else {
        return None;
    };
    let seg = path.segments.last()?;
    if seg.ident != "Arc" {
        return None;
    }
    let syn::PathArguments::AngleBracketed(args) = &seg.arguments else {
        return None;
    };
    match args.args.first()? {
        syn::GenericArgument::Type(inner) => Some(inner.clone()),
        _ => None,
    }
}

/// Returns the inner trait-object type if `ty` is written `Vec<Arc<dyn Trait...>>`, otherwise
/// `None`. The compile-time refusal of a plain field skips a site written this way.
pub fn extract_vec_arc_dyn_inner(ty: &Type) -> Option<Type> {
    let Type::Path(syn::TypePath { path, .. }) = ty else {
        return None;
    };
    let seg = path.segments.last()?;
    if seg.ident != "Vec" {
        return None;
    }
    let syn::PathArguments::AngleBracketed(args) = &seg.arguments else {
        return None;
    };
    let syn::GenericArgument::Type(inner) = args.args.first()? else {
        return None;
    };
    let Type::Path(syn::TypePath { path: arc_path, .. }) = inner else {
        return None;
    };
    let arc_seg = arc_path.segments.last()?;
    if arc_seg.ident != "Arc" {
        return None;
    }
    let syn::PathArguments::AngleBracketed(arc_args) = &arc_seg.arguments else {
        return None;
    };
    let syn::GenericArgument::Type(dyn_ty) = arc_args.args.first()? else {
        return None;
    };
    matches!(dyn_ty, Type::TraitObject(_)).then(|| dyn_ty.clone())
}

pub fn extract_impl_self_ident(impl_block: &ItemImpl) -> Result<Ident> {
    if let syn::Type::Path(type_path) = &*impl_block.self_ty {
        if let Some(ident) = type_path.path.get_ident() {
            return Ok(ident.clone());
        }
    }
    Err(syn::Error::new_spanned(
        &impl_block.self_ty,
        "expected a simple struct name (no generic parameters or path segments)",
    ))
}

pub fn extract_params_from_impl_fn(func: &ImplItemFn) -> Vec<(Ident, Type)> {
    let mut params = Vec::new();

    for input in &func.sig.inputs {
        if let FnArg::Typed(pat_type) = input {
            let param_name = match &*pat_type.pat {
                Pat::Ident(pat_ident) => pat_ident.ident.clone(),
                _ => continue,
            };

            let param_type = (*pat_type.ty).clone();

            params.push((param_name, param_type));
        }
    }

    params
}
