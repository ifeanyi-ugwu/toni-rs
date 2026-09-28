use std::collections::HashMap;

use proc_macro2::TokenStream;
use quote::{quote, quote_spanned};
use syn::parse::{Parse, ParseStream};
use syn::{Attribute, Error, Ident, Result, Token, punctuated::Punctuated, spanned::Spanned};

mod kw {
    syn::custom_keyword!(value);
}

fn is_enhancer(segment: &Ident) -> bool {
    matches!(
        segment.to_string().as_str(),
        "use_guards" | "use_interceptors" | "use_error_handlers"
    )
}

/// Matches by the path's last segment so path-qualified forms
/// (`#[ulo::use_guards(…)]`) are recognized alongside the bare ones.
pub fn has_enhancer_attribute(attr: &Attribute) -> bool {
    attr.path()
        .segments
        .last()
        .is_some_and(|segment| is_enhancer(&segment.ident))
}

/// One argument of `#[use_guards(…)]`, `#[use_interceptors(…)]` or `#[use_error_handlers(…)]`,
/// classified by its grammar. The spelling decides the lifecycle — `ulo::enhancer::GuardDeclaration`
/// or its sibling is the type each becomes — and [`enhancer_entries`] emits it.
#[derive(Clone)]
pub enum EnhancerInfo {
    /// `MyGuard` or `guards::Auth`: the binding under that type, resolved at `create`. Carries the
    /// expression that produces its key.
    Token(TokenStream),
    /// `MyGuard {}`, `MyGuard::new(..)` or `value LIMITER`: a value, built at startup and shared.
    Value(TokenStream),
    /// `|ctx| ..`: a constructor, run once per execution.
    Constructor(TokenStream),
}

/// One argument as written: a type, a value after `value`, or an expression.
enum EnhancerArg {
    Type(syn::Type),
    Value(syn::Expr),
    Expr(syn::Expr),
}

impl EnhancerArg {
    fn span(&self) -> proc_macro2::Span {
        match self {
            EnhancerArg::Type(t) => t.span(),
            EnhancerArg::Value(e) | EnhancerArg::Expr(e) => e.span(),
        }
    }
}

/// Whether the rest of this argument, up to the next comma, reads as one expression.
fn argument_reads_as_expression(input: ParseStream) -> bool {
    let fork = input.fork();
    fork.parse::<syn::Expr>().is_ok() && (fork.is_empty() || fork.peek(Token![,]))
}

/// How many tokens are left after `fork`, which a reading of the argument has advanced.
fn remaining(fork: &syn::parse::ParseBuffer) -> usize {
    fork.cursor().token_stream().into_iter().count()
}

/// Whether the argument reads as a type that goes further than any expression reading does.
///
/// `RoleGuard<A, B>` reads as the expression `RoleGuard < A`, which stops at the comma inside the
/// angle brackets, and as the type `RoleGuard<A, B>`, which reaches the argument's end. `MyGuard::new()`
/// reads fully either way, and there the expression wins.
fn argument_is_a_type(input: ParseStream) -> bool {
    let as_type = input.fork();
    if as_type.parse::<syn::Type>().is_err() || !(as_type.is_empty() || as_type.peek(Token![,])) {
        return false;
    }
    let as_expr = input.fork();
    match as_expr.parse::<syn::Expr>() {
        Ok(_) => remaining(&as_type) < remaining(&as_expr),
        Err(_) => true,
    }
}

impl Parse for EnhancerArg {
    fn parse(input: ParseStream) -> Result<Self> {
        // `value` counts as a keyword only where the argument does not already read as one
        // expression, so an item named `value` keeps its meaning; `value &X` is a keyword before a
        // reference to a `static` all the same.
        if input.peek(kw::value) {
            let fork = input.fork();
            fork.parse::<kw::value>()?;
            let reference = fork.peek(Token![&]);
            if (reference || !argument_reads_as_expression(input))
                && fork.parse::<syn::Expr>().is_ok()
                && (fork.is_empty() || fork.peek(Token![,]))
            {
                input.parse::<kw::value>()?;
                return Ok(EnhancerArg::Value(input.parse()?));
            }
        }
        // A generic type written without a turbofish reads further as a type than as an expression.
        if argument_is_a_type(input) {
            return Ok(EnhancerArg::Type(input.parse()?));
        }
        Ok(EnhancerArg::Expr(input.parse()?))
    }
}

/// Read every enhancer attribute into one ordered list per role.
///
/// Controller-level entries come first and method-level ones append after them, each in the order
/// written, so the list is the order written. The key is the attribute name without
/// `use_`: `guards`, `interceptors`, `error_handlers`.
pub fn create_enhancer_infos(
    controller_enhancers_attr: Vec<(&Ident, &Attribute)>,
    method_enhancers_attr: Vec<(&Ident, &Attribute)>,
) -> Result<HashMap<String, Vec<EnhancerInfo>>> {
    let mut enhancers: HashMap<String, Vec<EnhancerInfo>> = HashMap::new();

    for (ident, attr) in controller_enhancers_attr
        .into_iter()
        .chain(method_enhancers_attr)
    {
        let args = attr.parse_args_with(Punctuated::<EnhancerArg, Token![,]>::parse_terminated)?;

        let key = ident.to_string().replace("use_", "");

        for arg in args {
            let span = arg.span();
            let info = extract_enhancer_info(arg, &key)?;
            // An error handler has no per-execution arm to land on, so the refusal is here, at
            // the argument, rather than at startup.
            if key == "error_handlers" && matches!(info, EnhancerInfo::Constructor(_)) {
                return Err(Error::new(
                    span,
                    "an error handler is built once and shared, so the closure form has nothing \
                     to build per execution; write a type, a struct literal or call, or `value X`",
                ));
            }
            enhancers.entry(key.clone()).or_default().push(info);
        }
    }

    Ok(enhancers)
}

/// Whether any role has an entry. A generator that emits a descriptor only when something is
/// declared reads this.
pub fn declares_anything(infos: &HashMap<String, Vec<EnhancerInfo>>) -> bool {
    infos.values().any(|entries| !entries.is_empty())
}

/// The declaration type one role's entries are emitted as.
fn declaration_path(key: &str) -> TokenStream {
    match key {
        "guards" => quote! { ::ulo::enhancer::GuardDeclaration },
        "interceptors" => quote! { ::ulo::enhancer::InterceptorDeclaration },
        "error_handlers" => quote! { ::ulo::enhancer::ErrorHandlerDeclaration },
        other => panic!("no declaration type for enhancer role `{other}`"),
    }
}

/// One role's entries as the declaration values the descriptor carries, in the order written.
///
/// `transport` is the `ulo::dispatch` marker the generator serves — `::ulo::dispatch::Http` and
/// its three siblings. Every entry names it, so a value has a role trait object to coerce to and
/// a closure has a context type to infer its parameter from.
pub fn enhancer_entries(
    infos: &HashMap<String, Vec<EnhancerInfo>>,
    key: &str,
    transport: &TokenStream,
) -> Vec<TokenStream> {
    let declaration = declaration_path(key);
    infos
        .get(key)
        .map(|entries| {
            entries
                .iter()
                .map(|info| match info {
                    EnhancerInfo::Token(token) => {
                        quote! { #declaration::<#transport>::Token(#token) }
                    }
                    EnhancerInfo::Value(value) => {
                        quote! { #declaration::<#transport>::value(#value) }
                    }
                    EnhancerInfo::Constructor(build) => {
                        quote! { #declaration::<#transport>::constructor(#build) }
                    }
                })
                .collect()
        })
        .unwrap_or_default()
}

/// Classify one attribute argument by its grammar. A bare path is a type, never a const: a value
/// held in a const or a `static` takes `value` (ADR-0058, ADR-0059).
///
/// - `MyGuard`, `guards::Auth`, `RoleGuard<Admin>` — a type: the binding under it
/// - `MyGuard {}`, `MyGuard { role: "admin" }` — a struct literal: a value
/// - `MyGuard::new()`, `MyGuard::new("admin")` — a call: a value
/// - `value LIMITER`, `value &SHARED` — a value held in a const, or a `static` by reference
/// - `|ctx| MyGuard::for_call(ctx)` — a closure: a constructor
fn extract_enhancer_info(arg: EnhancerArg, role: &str) -> Result<EnhancerInfo> {
    let expr = match arg {
        EnhancerArg::Type(ty) => {
            return Ok(EnhancerInfo::Token(quote_spanned! {ty.span()=>
                ::ulo::di::token_of::<#ty>()
            }));
        }
        EnhancerArg::Value(value) => return Ok(EnhancerInfo::Value(quote! { #value })),
        EnhancerArg::Expr(expr) => expr,
    };
    match &expr {
        syn::Expr::Lit(syn::ExprLit {
            lit: syn::Lit::Str(_),
            ..
        }) => {
            let role_trait = match role {
                "interceptors" => "Interceptor",
                "error_handlers" => "ErrorHandler",
                _ => "Guard",
            };
            Err(Error::new(
                expr.span(),
                format!(
                    "a key is a type, not a string: declare a marker with \
                     `key!(pub Name: dyn {role_trait}<..>)` and name it here"
                ),
            ))
        }
        syn::Expr::Path(expr_path) if expr_path.attrs.is_empty() => {
            let ty = syn::TypePath {
                qself: expr_path.qself.clone(),
                path: expr_path.path.clone(),
            };
            Ok(EnhancerInfo::Token(quote_spanned! {expr.span()=>
                ::ulo::di::token_of::<#ty>()
            }))
        }
        syn::Expr::Struct(_) => Ok(EnhancerInfo::Value(quote! { #expr })),
        syn::Expr::Call(expr_call) => {
            if let syn::Expr::Path(_) = &*expr_call.func {
                return Ok(EnhancerInfo::Value(quote! { #expr }));
            }
            Err(Error::new(
                expr.span(),
                "expected a call to a path, such as `MyGuard::new()`",
            ))
        }
        syn::Expr::Closure(_) => Ok(EnhancerInfo::Constructor(quote! { #expr })),
        _ => Err(Error::new(
            expr.span(),
            "expected a type (MyGuard), a struct literal (MyGuard {}), a constructor call \
             (MyGuard::new()), `value X` for a value held in a const or `value &X` for one in a \
             static, or a closure (|ctx| MyGuard::new(ctx))",
        )),
    }
}

/// Collect enhancer attributes as (name, attribute) pairs in declaration order.
///
/// The name is the path's last segment, so `#[use_guards(…)]` and `#[ulo::use_guards(…)]`
/// collect identically. Stacked attributes of the same kind each get their own pair;
/// [`create_enhancer_infos`] appends them in order.
pub fn get_enhancers_attr(attrs: &[Attribute]) -> Result<Vec<(&Ident, &Attribute)>> {
    Ok(attrs
        .iter()
        .filter_map(|attr| {
            let segment = attr.path().segments.last()?;
            is_enhancer(&segment.ident).then_some((&segment.ident, attr))
        })
        .collect())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn arg(input: &str) -> EnhancerArg {
        syn::parse_str(input).unwrap()
    }

    #[test]
    fn a_bare_path_is_a_type_however_it_is_written() {
        for (input, written) in [
            ("Auth", "Auth"),
            ("guards::Auth", "guards :: Auth"),
            ("RoleGuard<Admin>", "RoleGuard < Admin >"),
            ("RoleGuard::<Admin>", "RoleGuard :: < Admin >"),
            ("RoleGuard<A, B>", "RoleGuard < A , B >"),
        ] {
            let EnhancerInfo::Token(key) = extract_enhancer_info(arg(input), "guards").unwrap()
            else {
                panic!("`{input}` did not read as a type");
            };
            let key = key.to_string();
            assert!(
                key.contains(&format!("token_of :: < {written} >")),
                "`{input}` keys as `{key}`"
            );
        }
    }

    #[test]
    fn a_call_is_a_value_though_it_also_reads_as_a_type() {
        assert!(matches!(
            arg("MyGuard::new()"),
            EnhancerArg::Expr(syn::Expr::Call(_))
        ));
    }

    #[test]
    fn a_generic_type_with_two_arguments_is_one_argument() {
        let args = syn::parse::Parser::parse_str(
            Punctuated::<EnhancerArg, Token![,]>::parse_terminated,
            "RoleGuard<A, B>, Other",
        )
        .unwrap();
        assert_eq!(args.len(), 2);
        assert!(matches!(args[0], EnhancerArg::Type(_)));
    }

    #[test]
    fn value_reads_as_a_keyword_only_where_no_single_expression_does() {
        assert!(matches!(
            arg("value ADMIN"),
            EnhancerArg::Value(syn::Expr::Path(_))
        ));
        assert!(matches!(
            arg("value &LIMITER"),
            EnhancerArg::Value(syn::Expr::Reference(_))
        ));
        assert!(matches!(
            arg("value - 1"),
            EnhancerArg::Expr(syn::Expr::Binary(_))
        ));
        assert!(matches!(
            arg("value::Limiter"),
            EnhancerArg::Expr(syn::Expr::Path(_))
        ));
        assert!(matches!(
            arg("value"),
            EnhancerArg::Expr(syn::Expr::Path(_))
        ));
    }

    #[test]
    fn a_string_is_refused() {
        for (role, named) in [
            ("guards", "dyn Guard<..>"),
            ("interceptors", "dyn Interceptor<..>"),
            ("error_handlers", "dyn ErrorHandler<..>"),
        ] {
            let Err(refusal) = extract_enhancer_info(arg("\"app.auth\""), role) else {
                panic!("a string is refused in `use_{role}`");
            };
            assert!(refusal.to_string().contains(named), "{refusal}");
        }
    }
}
