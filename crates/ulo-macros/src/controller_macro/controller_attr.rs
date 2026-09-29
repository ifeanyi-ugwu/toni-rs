//! `#[controller("/path")]` — the struct-attribute form of a controller.
//!
//! Placed on the struct, exactly like `#[injectable]`: `#[inject]` fields are dependencies,
//! `#[default(expr)]` fields are owned state, and construction/lifecycle reach the impl through the
//! `ulo::__construct` / `ulo::__lifecycle` bridges. The route handlers live in a sibling
//! `#[routes] impl` block, which the struct attribute never sees.
//!
//! This attribute produces a complete controller: the re-emitted struct (with `InjectFields`),
//! the `ControllerFactory`, the `Controller` object, the per-call provider its `DispatchSource`
//! resolves from, and four inherent bridge fns (build-from-deps, the dependency token list, the
//! route prefix, and whether the controller is explicitly execution-scoped). The object's
//! `dispatch()` goes through the `DispatchBridge`, whose default dispatches nothing — so a
//! controller with no handler impl is valid, and the handler impl (`#[routes]`, `#[patterns]`,
//! `#[grpc_methods]`) names the transport by shadowing `__ulo_dispatch`.

use proc_macro2::TokenStream;
use quote::quote;
use syn::{Fields, Ident, ItemStruct, Result, Type, parse2};

use crate::provider_macro::instance_injection::{add_inject_fields, generate_dispatch_system};
use crate::shared::dependency_info::DependencyInfo;
use crate::shared::scope_parser::{ControllerArgs, ControllerScope};
use crate::utils::extracts::extract_struct_dependencies;

pub fn handle_controller(attr: TokenStream, item: TokenStream) -> Result<TokenStream> {
    let struct_def = parse2::<ItemStruct>(item)?;
    let args = parse2::<ControllerArgs>(attr)?;

    if args.struct_def.is_some() {
        return Err(syn::Error::new_spanned(
            &struct_def.ident,
            "the inline-struct form `#[controller(\"/p\", pub struct …)]` has been removed; place \
             `#[controller(\"/p\")]` on the struct and `#[routes]` on its impl",
        ));
    }

    let struct_name = struct_def.ident.clone();
    let path = args.path;
    let is_execution_scoped = matches!(args.scope, ControllerScope::Execution);
    let dependencies = extract_struct_dependencies(&struct_def)?;

    let emitted_struct = add_inject_fields(&struct_def);
    let bridges = generate_bridges(
        &struct_name,
        &struct_def.fields,
        &dependencies,
        &path,
        is_execution_scoped,
    );
    let system = generate_dispatch_system(&struct_name);
    let default_checks = crate::shared::default_beside_new::default_beside_new(&struct_def);

    Ok(quote! {
        #[allow(dead_code)]
        #emitted_struct

        #bridges
        #system
        #default_checks
    })
}

fn generate_bridges(
    struct_name: &Ident,
    fields: &Fields,
    dependencies: &DependencyInfo,
    path: &str,
    is_execution_scoped: bool,
) -> TokenStream {
    let field_tokens: Vec<&TokenStream> = dependencies
        .fields
        .iter()
        .map(|(_, _, token)| token)
        .collect();

    let (field_resolutions, field_names) = resolve_fields(dependencies);

    let owned_field_inits: Vec<TokenStream> = dependencies
        .owned_fields
        .iter()
        .map(
            |(field_name, field_type, default_expr)| match default_expr {
                Some(expr) => quote! { #field_name: #expr },
                None => quote! { #field_name: {
                    #[allow(unused_imports)]
                    use ::ulo::__construct::OwnedFieldDefaultFallback as _;
                    (&::ulo::__construct::OwnedFieldDefault::<#field_type>::new())
                        .field_default(stringify!(#field_name), stringify!(#field_type))
                } },
            },
        )
        .collect();

    // Unit structs (`struct Foo;`) construct as `Self`, not `Self {}`.
    let struct_literal = match fields {
        Fields::Unit => quote! { Self },
        _ => quote! {
            Self {
                #(#field_names,)*
                #(#owned_field_inits),*
            }
        },
    };

    quote! {
        impl #struct_name {
            /// Build the controller from resolved dependencies — via the `#[new]` constructor when
            /// one exists (its inherent const shadows the blanket `CtorBridge` default), else by
            /// field injection. `__exec_ctx` is the execution being served, or `None` at startup.
            #[doc(hidden)]
            #[allow(unused_variables, non_snake_case, clippy::all)]
            pub async fn __ulo_build_from_deps(
                dependencies: &::ulo::FxHashMap<
                    String,
                    ::std::sync::Arc<dyn ::ulo::spi::Provider>,
                >,
                __exec_ctx: ::ulo::di::Execution,
            ) -> ::std::result::Result<Self, ::ulo::di::ResolutionError> {
                use ::ulo::__construct::CtorBridge as _;
                match <Self>::__ULO_ONE_NEW_PER_TYPE.map(|__ctor| (__ctor.build)(dependencies, __exec_ctx.clone())) {
                    ::std::option::Option::Some(__fut) => __fut.await,
                    ::std::option::Option::None => {
                        #(#field_resolutions)*
                        ::std::result::Result::Ok(#struct_literal)
                    }
                }
            }

            /// The dependency tokens this controller needs — the `#[new]` constructor's tokens when
            /// present, else the `#[inject]` field tokens.
            #[doc(hidden)]
            #[allow(non_snake_case)]
            pub fn __ulo_dependencies() -> ::std::vec::Vec<String> {
                use ::ulo::__construct::CtorBridge as _;
                <Self>::__ULO_ONE_NEW_PER_TYPE.map(|__ctor| (__ctor.tokens)()).unwrap_or_else(|| ::std::vec![#(#field_tokens),*])
            }

            #[doc(hidden)]
            #[allow(non_snake_case)]
            pub fn __ulo_prefix() -> &'static str {
                #path
            }

            #[doc(hidden)]
            #[allow(non_snake_case)]
            pub fn __ulo_is_execution_scoped() -> bool {
                #is_execution_scoped
            }
        }
    }
}

/// Resolve the `#[inject]` fields from the dependency map.
///
/// A collection field resolves on its own, ahead of the rest. The other fields are grouped by
/// lookup token, in order of first appearance, and deduplicated scope-aware: singleton and
/// execution-scoped providers are resolved once and shared (cloned) across same-token fields, while
/// transient providers get a fresh instance per field. The explicit dedup is required because not
/// every provider caches in the execution's cache (e.g. a hand-written `Provider`). Returns the
/// resolution statements plus the field names.
fn resolve_fields(dependencies: &DependencyInfo) -> (Vec<TokenStream>, Vec<Ident>) {
    use indexmap::IndexMap;

    let mut resolutions = Vec::new();
    let mut field_names = Vec::new();

    let mut groups: IndexMap<String, Vec<(Ident, Type, TokenStream)>> = IndexMap::new();
    for (name, ty, token) in &dependencies.fields {
        // A collection answers its items erased; the groups below downcast to the field's own type.
        if let Some(inner_trait) = crate::utils::extracts::extract_vec_arc_dyn_inner(ty) {
            resolutions.push(
                crate::provider_macro::instance_injection::collection_field_resolution(
                    name,
                    ty,
                    &inner_trait,
                    token,
                    quote! { dependencies },
                    ctx_expr(),
                ),
            );
            field_names.push(name.clone());
            continue;
        }
        groups.entry(quote!(#token).to_string()).or_default().push((
            name.clone(),
            ty.clone(),
            token.clone(),
        ));
    }

    for (_key, group) in groups {
        let (first_name, ty, token) = &group[0];
        if group.len() == 1 {
            resolutions.push(resolve_one(first_name, ty, token));
            field_names.push(first_name.clone());
            continue;
        }

        // Same token shared by several fields — resolve once (or per-field for transient).
        let idents: Vec<&Ident> = group.iter().map(|(n, _, _)| n).collect();
        let decls: Vec<TokenStream> = idents.iter().map(|n| quote! { let #n: #ty; }).collect();
        let ctx = ctx_expr();
        resolutions.push(quote! {
            #(#decls)*
            {
                let __lookup_token = #token;
                let __provider = dependencies.get(&__lookup_token)
                    .unwrap_or_else(|| panic!("Missing dependency '{}'", __lookup_token));
                if matches!(__provider.scope(), ::ulo::di::ProviderScope::Transient) {
                    #(
                        #idents = {
                            let __ctx = #ctx;
                            let __any = __provider.resolve(__ctx).await?;
                            *__any.downcast::<#ty>().map_err(|_| {
                                ::ulo::di::ResolutionError::TypeMismatch {
                                    token: __lookup_token.clone(),
                                }
                            })?
                        };
                    )*
                } else {
                    let __shared: #ty = {
                        let __ctx = #ctx;
                        let __any = __provider.resolve(__ctx).await?;
                        *__any.downcast::<#ty>().map_err(|_| {
                            ::ulo::di::ResolutionError::TypeMismatch {
                                token: __lookup_token.clone(),
                            }
                        })?
                    };
                    #( #idents = __shared.clone(); )*
                }
            }
        });
        for (n, _, _) in &group {
            field_names.push(n.clone());
        }
    }

    (resolutions, field_names)
}

/// One field resolution, in the execution the controller is built in.
fn resolve_one(name: &Ident, ty: &Type, token: &TokenStream) -> TokenStream {
    let name_str = name.to_string();
    let ctx = ctx_expr();
    quote! {
        let #name: #ty = {
            let __lookup_token = #token;
            let __provider = dependencies.get(&__lookup_token).unwrap_or_else(|| panic!(
                "Missing dependency '{}' for field '{}'", __lookup_token, #name_str
            ));
            let __ctx = #ctx;
            let __any = __provider.resolve(__ctx).await?;
            *__any.downcast::<#ty>().map_err(|_| ::ulo::di::ResolutionError::TypeMismatch {
                token: __lookup_token.clone(),
            })?
        };
    }
}

/// The execution a field resolves in: the one the controller is built in, `None` at startup. A
/// transient field is built in it too, so what that transient injects per execution is shared
/// with the rest of the call.
fn ctx_expr() -> TokenStream {
    quote! { __exec_ctx.clone() }
}
