//! `#[grpc_methods]` — applied to `impl SomeProtoTrait for MyService` blocks.
//!
//! Emits an `impl GrpcServiceTrait for MyService` whose `register_with`
//! body downcasts the registrar to `tonic::service::RoutesBuilder` and adds
//! `MyServiceServer::new(self.clone())`. The proto trait impl block itself
//! is passed through unchanged.
//!
//! By convention the wrapping `*Server` type name is the proto trait's
//! identifier with `Server` appended (`OrdersService` → `OrdersServer`),
//! resolved in the trait's parent path. Override with
//! `#[grpc_methods(server = path::to::OrdersServer)]` when the
//! tonic-generated wrapper lives elsewhere or has a non-standard name.

use proc_macro2::TokenStream;
use quote::quote;
use syn::{ItemImpl, Path, Result, Token, parse2};

struct GrpcMethodsArgs {
    server: Option<Path>,
}

impl syn::parse::Parse for GrpcMethodsArgs {
    fn parse(input: syn::parse::ParseStream) -> Result<Self> {
        if input.is_empty() {
            return Ok(GrpcMethodsArgs { server: None });
        }
        let key: syn::Ident = input.parse()?;
        if key != "server" {
            return Err(syn::Error::new(key.span(), "expected `server = path`"));
        }
        let _: Token![=] = input.parse()?;
        let server: Path = input.parse()?;
        Ok(GrpcMethodsArgs {
            server: Some(server),
        })
    }
}

pub fn handle_grpc_methods(attr: TokenStream, item: TokenStream) -> Result<TokenStream> {
    let args = parse2::<GrpcMethodsArgs>(attr)?;
    let impl_block = parse2::<ItemImpl>(item)?;

    let trait_path = impl_block
        .trait_
        .as_ref()
        .map(|(_, path, _)| path.clone())
        .ok_or_else(|| {
            syn::Error::new_spanned(
                &impl_block,
                "#[grpc_methods] must annotate `impl SomeProtoTrait for YourService` \
                 — it does nothing on inherent impls",
            )
        })?;

    let self_ty = impl_block.self_ty.as_ref();
    let self_ident = match self_ty {
        syn::Type::Path(tp) => tp
            .path
            .segments
            .last()
            .ok_or_else(|| syn::Error::new_spanned(self_ty, "self type has no segments"))?
            .ident
            .clone(),
        _ => {
            return Err(syn::Error::new_spanned(
                self_ty,
                "#[grpc_methods] expects a named `Self` type (got a non-path type)",
            ));
        }
    };

    let server_path = args.server.unwrap_or_else(|| infer_server_path(&trait_path));

    let token = self_ident.to_string();
    let trait_name_for_log = trait_path
        .segments
        .last()
        .map(|s| s.ident.to_string())
        .unwrap_or_default();

    let grpc_trait_impl = quote! {
        impl ::toni::adapter::GrpcServiceTrait for #self_ty {
            fn token(&self) -> ::std::string::String {
                #token.to_string()
            }

            fn register_with(&self, registrar: &mut dyn ::std::any::Any) {
                if let ::std::option::Option::Some(builder) = registrar.downcast_mut::<
                    ::tonic::service::RoutesBuilder,
                >() {
                    let server = #server_path::new(self.clone());
                    builder.add_service(server);
                } else {
                    ::toni::tracing::warn!(
                        service = #token,
                        proto_trait = #trait_name_for_log,
                        "GrpcServiceTrait::register_with received an unknown registrar; service not bound"
                    );
                }
            }
        }
    };

    Ok(quote! {
        #impl_block
        #grpc_trait_impl
    })
}

/// Convention: `OrdersService` (proto trait) → `OrdersServer` in the same
/// parent path. tonic-build emits `OrdersServer` alongside the trait, so
/// `parent::OrdersService` gives us `parent::OrdersServer`.
fn infer_server_path(trait_path: &Path) -> Path {
    let mut path = trait_path.clone();
    if let Some(last) = path.segments.last_mut() {
        let ident = last.ident.to_string();
        let base = ident.strip_suffix("Service").unwrap_or(&ident);
        let new_ident = format!("{}Server", base);
        last.ident = syn::Ident::new(&new_ident, last.ident.span());
        last.arguments = syn::PathArguments::None;
    }
    path
}
