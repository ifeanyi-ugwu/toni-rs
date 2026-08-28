//! `#[controller("/path")]` — the struct-attribute form of a controller.
//!
//! Placed on the struct, exactly like `#[injectable]`: `#[inject]` fields are dependencies,
//! `#[default(expr)]` fields are owned state, and construction/lifecycle reach the impl through the
//! `toni::__construct` / `toni::__lifecycle` bridges. The route handlers live in a sibling
//! `#[routes] impl` block, which the struct attribute never sees.
//!
//! This attribute produces a complete controller: the re-emitted struct (with `InjectFields`),
//! the `ControllerFactory`, the `Controller` object, the per-call provider its `DispatchSource`
//! resolves from, and four inherent bridge fns (build-from-deps, the dependency token list, the
//! route prefix, and whether the controller is explicitly request-scoped). The object's
//! `dispatch()` goes through the `DispatchBridge`, whose default dispatches nothing — so a
//! controller with no handler impl is valid, and the handler impl (`#[routes]`, `#[patterns]`,
//! `#[grpc_methods]`) names the transport by shadowing `__toni_dispatch`.

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
    if args.init.is_some() {
        return Err(syn::Error::new_spanned(
            &struct_def.ident,
            "`init = \"…\"` is not supported on `#[controller]`; mark the constructor with `#[new]` \
             (or use `#[inject]` field injection), as with `#[injectable]`",
        ));
    }

    let struct_name = struct_def.ident.clone();
    let path = args.path;
    let is_request = matches!(args.scope, ControllerScope::Request);
    let dependencies = extract_struct_dependencies(&struct_def)?;

    let emitted_struct = add_inject_fields(&struct_def);
    let bridges = generate_bridges(
        &struct_name,
        &struct_def.fields,
        &dependencies,
        &path,
        is_request,
    );
    let system = generate_dispatch_system(&struct_name);

    Ok(quote! {
        #[allow(dead_code)]
        #emitted_struct

        #bridges
        #system
    })
}

fn generate_bridges(
    struct_name: &Ident,
    fields: &Fields,
    dependencies: &DependencyInfo,
    path: &str,
    is_request: bool,
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
                    use ::toni::__construct::OwnedFieldDefaultFallback as _;
                    (&::toni::__construct::OwnedFieldDefault::<#field_type>::new())
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
            /// one exists (inherent fn shadows the blanket `CtorBridge` default), else by field
            /// injection. `__exec_ctx` is the execution being served, or `None` at startup.
            #[doc(hidden)]
            #[allow(unused_variables, non_snake_case, clippy::all)]
            pub async fn __toni_build_from_deps(
                dependencies: &::toni::FxHashMap<
                    String,
                    ::std::sync::Arc<Box<dyn ::toni::traits_helpers::Provider>>,
                >,
                __exec_ctx: ::toni::ProviderContext,
            ) -> Self {
                use ::toni::__construct::CtorBridge as _;
                match <Self>::__toni_ctor_build(dependencies, __exec_ctx.clone()) {
                    ::std::option::Option::Some(__fut) => __fut.await,
                    ::std::option::Option::None => {
                        #(#field_resolutions)*
                        #struct_literal
                    }
                }
            }

            /// The dependency tokens this controller needs — the `#[new]` constructor's tokens when
            /// present, else the `#[inject]` field tokens.
            #[doc(hidden)]
            #[allow(non_snake_case)]
            pub fn __toni_dependencies() -> ::std::vec::Vec<String> {
                use ::toni::__construct::CtorBridge as _;
                <Self>::__toni_ctor_tokens().unwrap_or_else(|| ::std::vec![#(#field_tokens),*])
            }

            #[doc(hidden)]
            #[allow(non_snake_case)]
            pub fn __toni_prefix() -> &'static str {
                #path
            }

            #[doc(hidden)]
            #[allow(non_snake_case)]
            pub fn __toni_is_request_scoped() -> bool {
                #is_request
            }
        }
    }
}

/// Resolve the `#[inject]` fields from the dependency map.
///
/// Fields are grouped by lookup token and deduplicated scope-aware, matching NestJS: singleton and
/// request-scoped providers are resolved once and shared (cloned) across same-token fields, while
/// transient providers get a fresh instance per field. The explicit dedup is required because not
/// every provider caches in the `RequestCache` (e.g. closure-based `provider_factory!`). Returns the
/// resolution statements plus the field names, in declaration order.
fn resolve_fields(dependencies: &DependencyInfo) -> (Vec<TokenStream>, Vec<Ident>) {
    use indexmap::IndexMap;

    let mut groups: IndexMap<String, Vec<(Ident, Type, TokenStream)>> = IndexMap::new();
    for (name, ty, token) in &dependencies.fields {
        groups.entry(quote!(#token).to_string()).or_default().push((
            name.clone(),
            ty.clone(),
            token.clone(),
        ));
    }

    let mut resolutions = Vec::new();
    let mut field_names = Vec::new();

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
                if matches!(__provider.get_scope(), ::toni::ProviderScope::Transient) {
                    #(
                        #idents = {
                            let __ctx = #ctx;
                            let __any = __provider.execute(::std::vec::Vec::new(), __ctx).await;
                            *__any.downcast::<#ty>().unwrap_or_else(|_| panic!(
                                "Failed to downcast '{}' to {}", __lookup_token, stringify!(#ty)
                            ))
                        };
                    )*
                } else {
                    let __shared: #ty = {
                        let __ctx = #ctx;
                        let __any = __provider.execute(::std::vec::Vec::new(), __ctx).await;
                        *__any.downcast::<#ty>().unwrap_or_else(|_| panic!(
                            "Failed to downcast '{}' to {}", __lookup_token, stringify!(#ty)
                        ))
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

/// One scope-aware field resolution: request-scoped providers get the active HTTP context (threaded
/// via `request_parts` + the shared `__request_cache`), anything else `ProviderContext::None`.
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
            let __any = __provider.execute(::std::vec::Vec::new(), __ctx).await;
            *__any.downcast::<#ty>().unwrap_or_else(|_| panic!(
                "Failed to downcast '{}' to {} for field '{}'",
                __lookup_token, stringify!(#ty), #name_str
            ))
        };
    }
}

/// The `ProviderContext` for a `__provider` in scope: this execution when the
/// provider is request-scoped, `None` otherwise. Resolving it in the same
/// execution is what makes one construction shared across the request.
fn ctx_expr() -> TokenStream {
    quote! {
        if matches!(__provider.get_scope(), ::toni::ProviderScope::Request) {
            __exec_ctx.clone()
        } else {
            ::toni::ProviderContext::None
        }
    }
}
