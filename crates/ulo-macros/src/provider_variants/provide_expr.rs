//! `provide!(key => source)`: the declaration surface, as sugar over `ulo::di::Provide`.
//!
//! What each form builds is a value-API value, so a declaration written with the macro and one
//! written with the value API are the same value. The grammar is led by keywords and position:
//!
//! - key: a type, `dyn Trait` included, or none. A type in this position is a marker or a type
//!   implementing `Key`, and a bare path is never read as a const;
//! - `into` before the key: a contribution to the collection under it;
//! - source: `value e`, `factory f`, `alias E` naming the binding under the type `E`, or an
//!   expression: a bare path is a type's own declaration, a closure is a factory, anything else a
//!   value.
//!
//! A slot holding a trait object takes `Arc<dyn Trait>`, and converting to it needs both types
//! concrete, which they are here: the macro writes the cast, spanned at the user's source so a
//! refusal lands there. Under a marker, whether its slot holds the declared type or a trait object
//! is decided by `ulo::__di`, since the macro sees the marker only as a path.

use proc_macro2::{Span, TokenStream, TokenTree};
use quote::quote_spanned;
use syn::parse::{Parse, ParseStream};
use syn::spanned::Spanned;
use syn::{Expr, Result, Token, Type};

mod kw {
    syn::custom_keyword!(into);
    syn::custom_keyword!(value);
    syn::custom_keyword!(factory);
    syn::custom_keyword!(alias);
}

/// Whether `input` is written in this grammar rather than the comma grammar. This grammar has a
/// top-level `=>`, starts with `into`, or is a lone source.
pub fn is_expr_grammar(input: &TokenStream) -> bool {
    let tokens: Vec<TokenTree> = input.clone().into_iter().collect();
    if let Some(TokenTree::Ident(first)) = tokens.first() {
        if first == "into" || first == "async" || first == "move" {
            return true;
        }
    }
    if matches!(tokens.first(), Some(TokenTree::Punct(p)) if p.as_char() == '|') {
        return true;
    }
    let has_fat_arrow = tokens.windows(2).any(|w| {
        matches!((&w[0], &w[1]),
            (TokenTree::Punct(a), TokenTree::Punct(b))
                if a.as_char() == '=' && b.as_char() == '>'
                    && a.spacing() == proc_macro2::Spacing::Joint)
    });
    let has_top_level_comma = tokens
        .iter()
        .any(|t| matches!(t, TokenTree::Punct(p) if p.as_char() == ','));
    has_fat_arrow || !has_top_level_comma
}

enum Key {
    /// `dyn Trait`: the trait object's own slot.
    Dyn(Type),
    /// A marker, or a type implementing `Key`.
    Typed(Type),
}

enum Source {
    Value(Expr),
    Factory(Expr),
    Type(Type),
    Alias(Type),
}

struct ProvideExpr {
    into: Option<kw::into>,
    key: Option<Key>,
    source: Source,
}

fn reads_as_one_expression(input: ParseStream) -> bool {
    let whole = input.fork();
    whole.parse::<Expr>().is_ok() && whole.is_empty()
}

/// Whether the next token is a keyword followed by a complete source. A keyword reads as one only
/// where the source does not already read as a single expression, so a source starting with an
/// item named like a keyword keeps its meaning. Two spellings read as operators are keywords all
/// the same: `value &X`, a reference, and `factory || ..` or `factory |x| ..`, a closure.
fn keyword_then_source(input: ParseStream) -> bool {
    let fork = input.fork();
    let Ok(word) = fork.parse::<proc_macro2::Ident>() else {
        return false;
    };
    let operator_spelling =
        (word == "value" && fork.peek(Token![&])) || (word == "factory" && fork.peek(Token![|]));
    if reads_as_one_expression(input) && !operator_spelling {
        return false;
    }
    fork.parse::<Expr>().is_ok() && fork.is_empty()
}

fn parse_source(input: ParseStream) -> Result<Source> {
    if input.peek(kw::value) && keyword_then_source(input) {
        input.parse::<kw::value>()?;
        return Ok(Source::Value(input.parse()?));
    }
    if input.peek(kw::factory) && keyword_then_source(input) {
        input.parse::<kw::factory>()?;
        return Ok(Source::Factory(input.parse()?));
    }
    // `alias` is followed by a type, which need not read as an expression (`dyn Logger`,
    // `Repo<User>`), so the check is on the whole source rather than on a keyword and one
    // expression.
    if input.peek(kw::alias) && !reads_as_one_expression(input) {
        input.parse::<kw::alias>()?;
        return Ok(Source::Alias(input.parse()?));
    }
    // A generic type written without a turbofish is not an expression; it is still a bare path.
    if !reads_as_one_expression(input) {
        let fork = input.fork();
        if let Ok(ty @ Type::Path(_)) = fork.parse::<Type>() {
            if fork.is_empty() {
                input.parse::<Type>()?;
                return Ok(Source::Type(ty));
            }
        }
    }
    let expr: Expr = input.parse()?;
    Ok(match expr {
        Expr::Path(p) if p.attrs.is_empty() => Source::Type(Type::Path(syn::TypePath {
            qself: p.qself,
            path: p.path,
        })),
        Expr::Closure(_) => Source::Factory(expr),
        other => Source::Value(other),
    })
}

impl Parse for ProvideExpr {
    fn parse(input: ParseStream) -> Result<Self> {
        let into: Option<kw::into> = input.parse()?;
        // The key ends at the first top-level `=>`; without one, the whole input is the source.
        let has_key = {
            let fork = input.fork();
            let mut found = false;
            while !fork.is_empty() {
                if fork.peek(Token![=>]) {
                    found = true;
                    break;
                }
                fork.parse::<TokenTree>()?;
            }
            found
        };
        let key = if has_key {
            let key = match input.parse::<Type>()? {
                ty @ Type::TraitObject(_) => Key::Dyn(ty),
                ty => Key::Typed(ty),
            };
            input.parse::<Token![=>]>()?;
            Some(key)
        } else {
            None
        };
        let source = parse_source(input)?;
        if !input.is_empty() {
            return Err(input.error("unexpected tokens after the source"));
        }
        Ok(Self { into, key, source })
    }
}

fn span_of(source: &Source) -> Span {
    match source {
        Source::Value(e) | Source::Factory(e) => e.span(),
        Source::Type(t) | Source::Alias(t) => t.span(),
    }
}

/// The declaration a source builds under its own type, before any key moves it.
fn declaration(source: Source, span: Span) -> TokenStream {
    match source {
        Source::Type(ty) => quote_spanned!(span=> <#ty as ::ulo::di::DeclaresProvider>::provide()),
        Source::Factory(f) => quote_spanned!(span=> ::ulo::di::Provide::factory(#f)),
        Source::Value(v) => quote_spanned!(span=> ::ulo::di::Provide::value(#v)),
        Source::Alias(_) => unreachable!("an alias is expanded before its declaration"),
    }
}

pub fn handle_provide_expr(input: TokenStream) -> Result<TokenStream> {
    let ProvideExpr { into, key, source } = syn::parse2(input)?;
    let span = span_of(&source);
    // The cast from what a declaration builds to what its slot holds. A type mismatch is reported
    // on the item, so it is spanned at the user's source.
    let cast = quote_spanned!(span=> |__item| __item);

    let expanded = match (into.is_some(), key, source) {
        (true, _, Source::Alias(_)) => {
            return Err(syn::Error::new(
                span,
                "`alias` does not contribute to a collection",
            ));
        }
        (_, None, Source::Alias(_)) => {
            return Err(syn::Error::new(
                span,
                "`alias` names a second slot for an existing binding: write \
                 `provide!(Key => alias Existing)`",
            ));
        }
        (true, None, _) => {
            return Err(syn::Error::new(
                span,
                "`into` needs the collection's key: write `provide!(into dyn Trait => source)` or \
                 `provide!(into Marker => source)`",
            ));
        }

        (false, None, source) => declaration(source, span),

        (false, Some(Key::Dyn(t)), Source::Alias(e)) => {
            quote_spanned!(span=> ::ulo::di::Provide::alias::<#t, #e>())
        }
        (false, Some(Key::Typed(k)), Source::Alias(e)) => {
            quote_spanned!(span=> ::ulo::di::Provide::alias_key::<#k, #e>())
        }

        (false, Some(Key::Dyn(t)), source) => {
            let declaration = declaration(source, span);
            quote_spanned!(span=> ::ulo::di::Declaration::under_with::<#t>(#declaration, #cast))
        }
        (false, Some(Key::Typed(k)), source) => {
            let declaration = declaration(source, span);
            quote_spanned!(span=> {
                #[allow(unused_imports)]
                use ::ulo::__di::{BindAsIs as _, BindCast as _};
                let __keyed = ::ulo::__di::keyed::<#k, _>(#declaration);
                (&&__keyed).bind(#cast)
            })
        }

        (true, Some(Key::Dyn(t)), source) => {
            let declaration = declaration(source, span);
            quote_spanned!(span=> ::ulo::di::Declaration::into_with::<#t>(#declaration, #cast))
        }
        (true, Some(Key::Typed(k)), source) => {
            let declaration = declaration(source, span);
            quote_spanned!(span=> ::ulo::di::Declaration::into_key_with::<#k>(#declaration, #cast))
        }
    };
    Ok(expanded)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn parsed(input: &str) -> ProvideExpr {
        syn::parse_str::<ProvideExpr>(input).unwrap()
    }

    fn source(input: &str) -> Source {
        parsed(input).source
    }

    #[test]
    fn a_key_is_a_type_however_it_is_written() {
        for input in [
            "Port => 1",
            "tokens::PORT => 1",
            "Repo<User> => 1",
            "Repo::<User> => 1",
        ] {
            assert!(
                matches!(parsed(input).key, Some(Key::Typed(Type::Path(_)))),
                "`{input}` did not read its key as a type"
            );
        }
        assert!(matches!(
            parsed("dyn Plugin => A {}").key,
            Some(Key::Dyn(_))
        ));
        assert!(matches!(
            parsed("into dyn Guard<HttpContext> => A {}").key,
            Some(Key::Dyn(_))
        ));
    }

    #[test]
    fn a_bare_path_source_is_a_type_with_or_without_a_turbofish() {
        for input in [
            "K => Db",
            "K => db::Db",
            "K => Repo::<User>",
            "K => Repo<User, Pg>",
        ] {
            assert!(
                matches!(source(input), Source::Type(Type::Path(_))),
                "`{input}` did not read its source as a type"
            );
        }
        assert!(matches!(source("Repo<User>"), Source::Type(_)));
    }

    #[test]
    fn alias_names_the_binding_under_a_type() {
        for input in [
            "K => alias Db",
            "K => alias dyn Logger",
            "K => alias Repo<User>",
        ] {
            assert!(
                matches!(source(input), Source::Alias(_)),
                "`{input}` did not read as an alias"
            );
        }
        assert!(matches!(source("K => alias::Db"), Source::Type(_)));
        assert!(matches!(
            source("K => alias(x)"),
            Source::Value(Expr::Call(_))
        ));
    }

    #[test]
    fn a_keyword_reads_as_one_only_where_no_single_expression_does() {
        assert!(matches!(
            source("K => value SEVEN"),
            Source::Value(Expr::Path(_))
        ));
        assert!(matches!(
            source("K => factory make"),
            Source::Factory(Expr::Path(_))
        ));
        assert!(matches!(
            source("K => factory || async { 1 }"),
            Source::Factory(Expr::Closure(_))
        ));
        assert!(matches!(
            source("K => factory |d| d"),
            Source::Factory(Expr::Closure(_))
        ));
        assert!(matches!(
            source("K => value &SHARED"),
            Source::Value(Expr::Reference(_))
        ));
        assert!(matches!(
            source("K => value - 1"),
            Source::Value(Expr::Binary(_))
        ));
        assert!(matches!(source("K => value::Foo"), Source::Type(_)));
    }
}
