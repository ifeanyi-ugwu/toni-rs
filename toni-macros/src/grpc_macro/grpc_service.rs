//! `#[grpc_service]` — declares a gRPC service struct as a DI provider that
//! the framework discovers and registers with the gRPC adapter at bind time.
//!
//! Lives on the **struct declaration + an inherent impl block** containing the
//! `new()` constructor (parallel to `#[rpc_controller]`). The trait impl
//! against the proto-generated trait gets `#[grpc_methods]` separately —
//! that's what produces the `GrpcServiceSource` impl that knows how to wrap
//! `self` in the tonic `*Server`.

use std::collections::HashSet;

use proc_macro2::TokenStream;
use syn::{ItemImpl, ItemStruct, Result, parse2};

use crate::controller_macro::controller_struct::{extract_constructor_params, has_new_method};
use crate::provider_macro::instance_injection::generate_instance_provider_system;
use crate::shared::dependency_info::{DependencyInfo, DependencySource};
use crate::shared::scope_parser::ProviderScope;
use crate::utils::extracts::extract_struct_dependencies;

struct GrpcServiceArgs {
    struct_def: Option<ItemStruct>,
}

impl syn::parse::Parse for GrpcServiceArgs {
    fn parse(input: syn::parse::ParseStream) -> Result<Self> {
        let struct_def = if !input.is_empty() {
            Some(input.parse::<ItemStruct>()?)
        } else {
            None
        };
        Ok(GrpcServiceArgs { struct_def })
    }
}

pub fn handle_grpc_service(attr: TokenStream, item: TokenStream) -> Result<TokenStream> {
    let args = parse2::<GrpcServiceArgs>(attr)?;
    let impl_block = parse2::<ItemImpl>(item)?;

    let struct_def = args.struct_def;

    let mut dependencies = match &struct_def {
        Some(s) => extract_struct_dependencies(s)?,
        None => DependencyInfo {
            fields: vec![],
            owned_fields: vec![],
            init_method: None,
            constructor_params: vec![],
            unique_types: HashSet::new(),
            source: DependencySource::None,
        },
    };

    if has_new_method(&impl_block) {
        let params = extract_constructor_params(&impl_block, "new")?;
        dependencies.init_method = Some("new".to_string());
        dependencies.constructor_params = params;
        dependencies.source = DependencySource::Constructor("new".to_string());
    } else if struct_def.is_none() {
        return Err(syn::Error::new_spanned(
            &impl_block.self_ty,
            "add a `fn new(...) -> Self` constructor to declare this gRPC service's dependencies, \
             or move the struct definition into the macro attribute",
        ));
    }

    generate_instance_provider_system(
        struct_def.as_ref(),
        &impl_block,
        &dependencies,
        ProviderScope::Singleton,
        false, // is_gateway
        false, // is_rpc_controller
        true,  // is_grpc_service
    )
}
