use proc_macro2::TokenStream;
use quote::{quote, quote_spanned};
use syn::ext::IdentExt;
use syn::meta::ParseNestedMeta;
use syn::{Data, DeriveInput, Expr, Fields, Ident, LitStr, Token, token};

/// One rule on one field.
pub(crate) enum Rule {
    /// `length(min = .., max = ..)`, either bound optional, inclusive: characters for text,
    /// items for a collection (`ulo_transport::__private::validate::LengthProbe`).
    Length { min: Option<Expr>, max: Option<Expr> },
    /// `email`: the shape `ulo_transport::__private::validate::is_email` checks, not the full
    /// RFC 5322 grammar.
    Email,
    /// `range(min = .., max = ..)`, either bound optional, inclusive.
    Range { min: Option<Expr>, max: Option<Expr> },
}

/// The rules of one field.
pub(crate) struct FieldRules {
    pub(crate) field: Ident,
    /// The name a request carries the field under, which each violation reports.
    pub(crate) wire: String,
    pub(crate) rules: Vec<Rule>,
}

/// `impl ::ulo_transport::Validate for <Type>`, collecting a `FieldViolation` per failing rule.
pub(crate) fn expand(input: TokenStream) -> syn::Result<TokenStream> {
    let input: DeriveInput = syn::parse2(input)?;
    let name = &input.ident;
    let fields = match &input.data {
        Data::Struct(data) => match &data.fields {
            Fields::Named(named) => &named.named,
            _ => return Err(syn::Error::new_spanned(name, "`Validate` derives on a struct with named fields")),
        },
        _ => return Err(syn::Error::new_spanned(name, "`Validate` derives on a struct with named fields")),
    };

    let rename_all = serde_value(&input.attrs, "rename_all");
    let mut validated = Vec::new();
    let mut errors = Vec::new();
    for field in fields {
        let Some(ident) = &field.ident else { continue };
        match rules_of(&field.attrs) {
            Ok(rules) if rules.is_empty() => {}
            Ok(rules) => validated.push(FieldRules {
                field: ident.clone(),
                wire: wire_name(ident, &field.attrs, rename_all.as_deref()),
                rules,
            }),
            Err(error) => errors.push(error),
        }
    }
    let errors = errors.into_iter().reduce(|mut all, next| {
        all.combine(next);
        all
    });
    if let Some(error) = errors {
        return Err(error);
    }

    let checks = validated.iter().flat_map(|field| field.rules.iter().map(move |rule| check(field, rule)));
    let (impl_generics, ty_generics, where_clause) = input.generics.split_for_impl();
    Ok(quote! {
        impl #impl_generics ::ulo_transport::Validate for #name #ty_generics #where_clause {
            fn validate(&self) -> ::core::result::Result<(), ::std::vec::Vec<::ulo_transport::FieldViolation>> {
                #[allow(unused_imports)]
                use ::ulo_transport::__private::validate::{ViaChars as _, ViaItems as _};
                #[allow(unused_mut)]
                let mut __ulo_violations: ::std::vec::Vec<::ulo_transport::FieldViolation> = ::std::vec::Vec::new();
                #(#checks)*
                if __ulo_violations.is_empty() {
                    ::core::result::Result::Ok(())
                } else {
                    ::core::result::Result::Err(__ulo_violations)
                }
            }
        }
    })
}

/// One rule's check, pushing one violation when it fails. The bounds are evaluated once each and
/// written into the description through `Display`.
fn check(field: &FieldRules, rule: &Rule) -> TokenStream {
    let ident = &field.field;
    let wire = &field.wire;
    let span = ident.span();
    match rule {
        Rule::Length { min, max } => {
            let measured = quote_spanned! {span=>
                (&&::ulo_transport::__private::validate::LengthProbe(&self.#ident)).length()
            };
            let test = bounded(quote!(__ulo_value), wire, "length must be", min.as_ref(), max.as_ref());
            quote! {{
                let __ulo_value: usize = #measured;
                #test
            }}
        }
        Rule::Range { min, max } => {
            let value = quote_spanned!(span=> self.#ident);
            bounded(value, wire, "must be", min.as_ref(), max.as_ref())
        }
        Rule::Email => {
            let text = quote_spanned!(span=> ::core::convert::AsRef::<str>::as_ref(&self.#ident));
            quote! {
                if !::ulo_transport::__private::validate::is_email(#text) {
                    __ulo_violations.push(::ulo_transport::FieldViolation::new(#wire, "must be an email address"));
                }
            }
        }
    }
}

/// `value` against inclusive bounds, written so that a value no bound orders, a float's NaN,
/// fails.
fn bounded(value: TokenStream, wire: &str, subject: &str, min: Option<&Expr>, max: Option<&Expr>) -> TokenStream {
    let violation = |template: String, args: TokenStream| {
        let template = LitStr::new(&template, proc_macro2::Span::call_site());
        quote! {
            __ulo_violations.push(::ulo_transport::FieldViolation::new(#wire, ::std::format!(#template, #args)));
        }
    };
    match (min, max) {
        (Some(min), Some(max)) => {
            let push = violation(format!("{subject} between {{}} and {{}}"), quote!(__ulo_min, __ulo_max));
            quote! {{
                let __ulo_min = #min;
                let __ulo_max = #max;
                if !(#value >= __ulo_min && #value <= __ulo_max) {
                    #push
                }
            }}
        }
        (Some(min), None) => {
            let push = violation(format!("{subject} at least {{}}"), quote!(__ulo_min));
            quote! {{
                let __ulo_min = #min;
                if !(#value >= __ulo_min) {
                    #push
                }
            }}
        }
        (None, Some(max)) => {
            let push = violation(format!("{subject} at most {{}}"), quote!(__ulo_max));
            quote! {{
                let __ulo_max = #max;
                if !(#value <= __ulo_max) {
                    #push
                }
            }}
        }
        (None, None) => TokenStream::new(),
    }
}

/// The rules in a field's `#[validate(..)]` attributes; an unknown rule is a span error naming the
/// three.
pub(crate) fn rules_of(attrs: &[syn::Attribute]) -> syn::Result<Vec<Rule>> {
    let mut rules = Vec::new();
    for attr in attrs {
        if !attr.path().is_ident("validate") {
            continue;
        }
        attr.parse_nested_meta(|meta| {
            if meta.path.is_ident("email") {
                if !(meta.input.is_empty() || meta.input.peek(Token![,])) {
                    return Err(meta.error("`email` takes no arguments"));
                }
                rules.push(Rule::Email);
            } else if meta.path.is_ident("length") {
                let (min, max) = bounds(&meta, "length")?;
                rules.push(Rule::Length { min, max });
            } else if meta.path.is_ident("range") {
                let (min, max) = bounds(&meta, "range")?;
                rules.push(Rule::Range { min, max });
            } else {
                return Err(meta.error(
                    "unknown rule; `#[validate(..)]` takes `length(min = .., max = ..)`, `email` and `range(min = .., max = ..)`",
                ));
            }
            Ok(())
        })?;
    }
    Ok(rules)
}

/// `(min = .., max = ..)` after `length` or `range`: either may be left out, not both, and
/// neither written twice.
fn bounds(meta: &ParseNestedMeta<'_>, rule: &str) -> syn::Result<(Option<Expr>, Option<Expr>)> {
    if !meta.input.peek(token::Paren) {
        return Err(meta.error(format!("`{rule}` takes its bounds in parentheses: `{rule}(min = .., max = ..)`")));
    }
    let mut min: Option<Expr> = None;
    let mut max: Option<Expr> = None;
    meta.parse_nested_meta(|bound| {
        let (slot, word) = if bound.path.is_ident("min") {
            (&mut min, "min")
        } else if bound.path.is_ident("max") {
            (&mut max, "max")
        } else {
            return Err(bound.error(format!("`{rule}` takes `min` and `max`")));
        };
        if slot.is_some() {
            return Err(bound.error(format!("`{word}` is written twice")));
        }
        *slot = Some(bound.value()?.parse()?);
        Ok(())
    })?;
    if min.is_none() && max.is_none() {
        return Err(meta.error(format!("`{rule}` needs `min`, `max` or both")));
    }
    Ok((min, max))
}

/// The name a deserializer reads the field under, which is how the request names it: serde's
/// `rename` (or its `deserialize` half), else the container's `rename_all` applied to the field's
/// name, else the name as written.
fn wire_name(field: &Ident, attrs: &[syn::Attribute], rename_all: Option<&str>) -> String {
    if let Some(name) = serde_value(attrs, "rename") {
        return name;
    }
    let name = field.unraw().to_string();
    match rename_all {
        Some(rule) => apply_case(&name, rule),
        None => name,
    }
}

/// The deserialize side of serde's `key = ".."` or `key(deserialize = "..")` among `attrs`.
///
/// Read leniently: serde reports its own malformed attributes, so a form this does not follow
/// counts as absent rather than as an error here.
fn serde_value(attrs: &[syn::Attribute], key: &str) -> Option<String> {
    let mut found = None;
    for attr in attrs {
        if !attr.path().is_ident("serde") {
            continue;
        }
        let _ = attr.parse_nested_meta(|meta| {
            if !meta.path.is_ident(key) {
                return skip(&meta);
            }
            if meta.input.peek(Token![=]) {
                let value: LitStr = meta.value()?.parse()?;
                found = Some(value.value());
                return Ok(());
            }
            if meta.input.peek(token::Paren) {
                meta.parse_nested_meta(|side| {
                    if !side.path.is_ident("deserialize") {
                        return skip(&side);
                    }
                    let value: LitStr = side.value()?.parse()?;
                    found = Some(value.value());
                    Ok(())
                })?;
            }
            Ok(())
        });
    }
    found
}

/// Consumes whatever follows a serde key this derive does not read.
fn skip(meta: &ParseNestedMeta<'_>) -> syn::Result<()> {
    if meta.input.peek(Token![=]) {
        let _: Expr = meta.value()?.parse()?;
    } else if meta.input.peek(token::Paren) {
        let _: proc_macro2::Group = meta.input.parse()?;
    }
    Ok(())
}

/// serde's `rename_all` rule applied to a field name, as serde applies it to a field: the name is
/// taken to be snake_case. An unknown rule leaves the name as written; serde refuses it.
fn apply_case(field: &str, rule: &str) -> String {
    match rule {
        "UPPERCASE" | "SCREAMING_SNAKE_CASE" => field.to_ascii_uppercase(),
        "PascalCase" => pascal(field),
        "camelCase" => {
            let pascal = pascal(field);
            let mut chars = pascal.chars();
            match chars.next() {
                Some(first) => first.to_ascii_lowercase().to_string() + chars.as_str(),
                None => String::new(),
            }
        }
        "kebab-case" => field.replace('_', "-"),
        "SCREAMING-KEBAB-CASE" => field.to_ascii_uppercase().replace('_', "-"),
        _ => field.to_owned(),
    }
}

fn pascal(field: &str) -> String {
    let mut out = String::with_capacity(field.len());
    let mut capitalize = true;
    for c in field.chars() {
        if c == '_' {
            capitalize = true;
        } else if capitalize {
            out.push(c.to_ascii_uppercase());
            capitalize = false;
        } else {
            out.push(c);
        }
    }
    out
}
