use std::collections::HashSet;

use syn::{
    Error, Expr, FnArg, Ident, ImplItemFn, ItemImpl, ItemStruct, LitStr, Pat, Result, Type,
    TypePath, TypeReference, spanned::Spanned,
};

use crate::shared::dependency_info::DependencyInfo;

pub fn extract_controller_prefix(impl_block: &ItemImpl) -> Result<String> {
    impl_block
        .attrs
        .iter()
        .find(|attr| attr.path().is_ident("controller"))
        .map(|attr| attr.parse_args::<LitStr>().map(|lit| lit.value()))
        .transpose()
        .map(|opt| opt.unwrap_or_default())
}

pub fn extract_struct_dependencies(struct_attrs: &ItemStruct) -> Result<DependencyInfo> {
    let unique_types = HashSet::new();
    let mut fields = Vec::new();
    let mut owned_fields = Vec::new();

    // BACKWARD COMPATIBILITY: Check if ANY field has #[inject] attribute
    let has_any_inject = struct_attrs.fields.iter().any(|field| {
        field
            .attrs
            .iter()
            .any(|attr| attr.path().is_ident("inject"))
    });

    for field in &struct_attrs.fields {
        let field_ident = field
            .ident
            .as_ref()
            .ok_or_else(|| syn::Error::new_spanned(field, "Unnamed struct fields not supported"))?;

        // Check for #[inject] attribute
        let has_inject = field
            .attrs
            .iter()
            .any(|attr| attr.path().is_ident("inject"));

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

        // BACKWARD COMPATIBILITY: If NO field has #[inject], treat all fields as DI dependencies (old behavior)
        if !has_any_inject {
            // Old behavior: all fields are DI dependencies
            let full_type = field.ty.clone();
            let type_ident = extract_ident_from_type(&field.ty)?;
            let lookup_token = type_ident.to_string();
            fields.push((field_ident.clone(), full_type, lookup_token));
        } else {
            // New behavior: explicit #[inject] required
            if has_inject {
                // This is a DI dependency
                let full_type = field.ty.clone();
                let type_ident = extract_ident_from_type(&field.ty)?;
                let lookup_token = type_ident.to_string();
                fields.push((field_ident.clone(), full_type, lookup_token));
            } else {
                // This is an owned field
                owned_fields.push((field_ident.clone(), field.ty.clone(), default_expr));
            }
        }
    }

    Ok(DependencyInfo {
        fields,
        owned_fields,
        init_method: None, // Will be set by caller if provided in attributes
        unique_types,
    })
}

/// Extract the #[default(expr)] attribute from a field
fn extract_default_attr(field: &syn::Field) -> Result<Option<Expr>> {
    for attr in &field.attrs {
        if attr.path().is_ident("default") {
            let expr: Expr = attr.parse_args()?;
            return Ok(Some(expr));
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
