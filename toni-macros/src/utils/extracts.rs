use std::collections::HashSet;

use proc_macro2::TokenStream;
use quote::quote;
use syn::{
    Error, Expr, FnArg, Ident, ImplItemFn, ItemImpl, ItemStruct, LitStr, Pat, Result, Type,
    TypePath, TypeReference, spanned::Spanned,
};

use crate::shared::dependency_info::{DependencyInfo, DependencySource};
use crate::shared::{TokenType, attr_is};

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
            init_method: None,
            constructor_params: Vec::new(),
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

        // Check for #[inject] or #[inject("TOKEN")] attribute
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
                    // #[inject("TOKEN")] or #[inject(Type)] - use custom token
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
        init_method: None, // Will be set by caller if provided in attributes
        constructor_params: Vec::new(), // Will be populated by caller if constructor detected
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

/// Extract the #[inject] or #[inject(token)] attribute from a field
/// Returns:
/// - None: no #[inject] attribute
/// - Some(None): #[inject] without token (use type-based token)
/// - Some(Some(token_expr)): #[inject("TOKEN")] or #[inject(Type)] with custom token
fn extract_inject_attr(field: &syn::Field) -> Result<Option<Option<TokenStream>>> {
    for attr in &field.attrs {
        if attr_is(attr, "inject") {
            // Check if there's an argument
            if attr.meta.require_path_only().is_ok() {
                // #[inject] without arguments - use type-based token
                return Ok(Some(None));
            } else {
                // #[inject("TOKEN")] or #[inject(Type)] or #[inject(CONST)]
                // Parse as TokenType to support all token formats
                let token_type: TokenType = attr.parse_args()?;
                let token_expr = token_type.to_token_expr();
                return Ok(Some(Some(token_expr)));
            }
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

/// Extracts a type token expression: a call to `toni::di::token_of` with the
/// written type, resolved in the caller's scope. The compiler canonicalizes the
/// spelling — qualified paths, aliases, and generic parameters all produce the
/// fully-qualified name the registration side uses.
pub fn extract_type_token(ty: &Type) -> Result<TokenStream> {
    // Handle references by unwrapping to inner type
    let actual_type = if let Type::Reference(TypeReference { elem, .. }) = ty {
        &**elem
    } else {
        ty
    };

    if let Type::Path(_) = actual_type {
        return Ok(quote! { ::toni::di::token_of::<#actual_type>() });
    }

    Err(syn::Error::new_spanned(
        ty,
        "Expected a type path (e.g., MyType or MyType<T>)",
    ))
}

/// Normalize a trait-object type to always include `+ Send + Sync`.
///
/// Multi-providers store `Arc<dyn Trait + Send + Sync>` internally. When a user writes
/// `Vec<Arc<dyn Plugin>>` (omitting the bounds), the downcast type must still match.
pub fn normalize_trait_send_sync(ty: Type) -> Type {
    let Type::TraitObject(mut tobj) = ty else {
        return ty;
    };
    let has_send = tobj
        .bounds
        .iter()
        .any(|b| matches!(b, syn::TypeParamBound::Trait(t) if t.path.is_ident("Send")));
    let has_sync = tobj
        .bounds
        .iter()
        .any(|b| matches!(b, syn::TypeParamBound::Trait(t) if t.path.is_ident("Sync")));
    if !has_send {
        tobj.bounds
            .push(syn::TypeParamBound::Trait(syn::TraitBound {
                paren_token: None,
                modifier: syn::TraitBoundModifier::None,
                lifetimes: None,
                path: syn::parse_quote!(Send),
            }));
    }
    if !has_sync {
        tobj.bounds
            .push(syn::TypeParamBound::Trait(syn::TraitBound {
                paren_token: None,
                modifier: syn::TraitBoundModifier::None,
                lifetimes: None,
                path: syn::parse_quote!(Sync),
            }));
    }
    Type::TraitObject(tobj)
}

/// Returns the inner trait-object type if `ty` is `Vec<Arc<dyn Trait...>>`, otherwise `None`.
///
/// Used by injection codegen to detect multi-provider fields and generate the
/// appropriate double-Arc downcast instead of the regular single-value downcast.
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
