use std::collections::HashSet;

use syn::{
    Error, FnArg, Ident, ImplItemFn, ItemImpl, ItemStruct, LitStr, Pat, Result, Type, TypePath,
    TypeReference, spanned::Spanned,
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

    for field in &struct_attrs.fields {
        let field_ident = field
            .ident
            .as_ref()
            .ok_or_else(|| syn::Error::new_spanned(field, "Unnamed struct fields not supported"))?;

        // Extract the full type (preserves generics like ConfigService<AppConfig>)
        let full_type = field.ty.clone();

        // Extract just the type identifier for provider lookup token
        let type_ident = extract_ident_from_type(&field.ty)?;
        let lookup_token = type_ident.to_string();

        fields.push((field_ident.clone(), full_type, lookup_token));
    }

    Ok(DependencyInfo {
        fields,
        unique_types,
    })
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
