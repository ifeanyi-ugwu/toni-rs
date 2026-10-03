//! The attributes that decide whether a handler, or one of its enhancer entries, is compiled,
//! which `#[routes]` copies onto every item it writes that names the handler's generated items,
//! and the reading of a method's attributes through `cfg_attr`.
//!
//! An attribute macro receives the items inside its input with their `#[cfg]` and `#[cfg_attr]`
//! unevaluated, so `#[routes]` sees a method behind `#[cfg(feature = "x")]` whether or not the
//! feature is on. Rustc evaluates that `cfg` after `#[routes]` has expanded, and with the feature
//! off it removes the method together with its transport attribute, which then writes none of
//! `__ULO_KEY_<name>`, `__ULO_CHECKS_<name>` and `__ulo_mount_<name>`. Likewise
//! `#[cfg_attr(feature = "x", guards(..))]` reaches `#[routes]` as a `cfg_attr`, and expands to
//! `#[guards(..)]` only after `#[routes]` has run; [`take`] reads through it, so `#[routes]` takes
//! the entry with its predicate as a gate.

use proc_macro2::{Delimiter, Group, TokenStream, TokenTree};
use quote::{ToTokens, quote, quote_spanned};
use syn::spanned::Spanned;
use syn::{AttrStyle, Attribute, Meta, MetaList};

/// One attribute as rustc applies it once every `cfg_attr` around it is evaluated, with the
/// predicates of those `cfg_attr`s, outermost first. An attribute written outside any `cfg_attr`
/// has no predicates.
#[derive(Clone)]
pub struct GatedAttr {
    pub predicates: Vec<TokenStream>,
    /// The attribute, as an outer attribute; for one read out of a `cfg_attr`, its tokens as
    /// written there.
    pub attr: Attribute,
}

impl GatedAttr {
    /// One `#[cfg(<predicate>)]` per predicate, spanned at the predicate. Empty when ungated.
    pub fn gates(&self) -> Vec<Attribute> {
        gates_of(&self.predicates)
    }
}

/// One `#[cfg(<predicate>)]` per predicate, spanned at the predicate.
pub fn gates_of(predicates: &[TokenStream]) -> Vec<Attribute> {
    predicates
        .iter()
        .map(|predicate| {
            let span = predicate.span();
            syn::parse_quote_spanned!(span=> #[cfg(#predicate)])
        })
        .collect()
}

/// `#[cfg_attr(<predicates>, <meta>)]`, the predicates joined with `all(..)` when there are
/// several; `#[<meta>]` when there are none.
pub fn under(predicates: &[TokenStream], meta: TokenStream) -> TokenStream {
    match predicates {
        [] => quote!(#[#meta]),
        [predicate] => quote_spanned!(predicate.span()=> #[cfg_attr(#predicate, #meta)]),
        [first, ..] => quote_spanned!(first.span()=> #[cfg_attr(all(#(#predicates),*), #meta)]),
    }
}

/// Every outer attribute among `attrs`, read through `cfg_attr` at any depth, in the order
/// written: `#[cfg_attr(a, inline, cfg_attr(b, get("/")))]` yields `inline` behind `a` and
/// `get("/")` behind `a` and `b`. An inner attribute, or a `cfg_attr` whose arguments do not read
/// as a predicate followed by attributes, is yielded whole, with no predicates.
pub fn leaves(attrs: &[Attribute]) -> Vec<GatedAttr> {
    let mut leaves = Vec::new();
    for attr in attrs {
        walk(attr, &[], &mut |leaf| {
            leaves.push(leaf.clone());
            false
        });
    }
    leaves
}

/// Removes from `attrs` every attribute [`leaves`] yields that `take` accepts, and returns those,
/// in the order written. A `cfg_attr` left holding nothing is removed; one holding something else
/// keeps it: `#[cfg_attr(p, doc = "..", guards(A))]` becomes `#[cfg_attr(p, doc = "..")]`.
pub fn take(attrs: &mut Vec<Attribute>, mut take: impl FnMut(&Attribute) -> bool) -> Vec<GatedAttr> {
    let mut taken = Vec::new();
    let written = std::mem::take(attrs);
    for attr in written {
        let walked = walk(&attr, &[], &mut |leaf| {
            let accepted = take(&leaf.attr);
            if accepted {
                taken.push(leaf.clone());
            }
            accepted
        });
        match walked {
            Walked::Unchanged => attrs.push(attr),
            Walked::Rewritten(rest) => attrs.push(rest),
            Walked::Removed => {}
        }
    }
    taken
}

/// What [`walk`] leaves of an attribute.
enum Walked {
    Unchanged,
    Rewritten(Attribute),
    Removed,
}

/// `attr` with every leaf `visit` accepts removed. `predicates` are those of the `cfg_attr`s
/// around `attr`.
fn walk(attr: &Attribute, predicates: &[TokenStream], visit: &mut dyn FnMut(&GatedAttr) -> bool) -> Walked {
    let whole = |visit: &mut dyn FnMut(&GatedAttr) -> bool| {
        let leaf = GatedAttr { predicates: predicates.to_vec(), attr: Attribute { style: AttrStyle::Outer, ..attr.clone() } };
        if visit(&leaf) { Walked::Removed } else { Walked::Unchanged }
    };
    let cfg_attr = match (&attr.style, &attr.meta) {
        (AttrStyle::Outer, Meta::List(list)) if attr.path().is_ident("cfg_attr") => list,
        _ => return whole(visit),
    };
    let mut pieces = split_commas(cfg_attr.tokens.clone()).into_iter();
    let Some(predicate) = pieces.next().filter(|predicate| !predicate.is_empty()) else {
        return whole(visit);
    };
    let mut inner = predicates.to_vec();
    inner.push(predicate.clone());
    let mut kept = Vec::new();
    let mut changed = false;
    for piece in pieces {
        let Ok(meta) = syn::parse2::<Meta>(read_through(piece.clone())) else {
            kept.push(piece);
            continue;
        };
        let nested = Attribute { pound_token: attr.pound_token, style: AttrStyle::Outer, bracket_token: attr.bracket_token, meta };
        match walk(&nested, &inner, visit) {
            Walked::Unchanged => kept.push(piece),
            Walked::Rewritten(rest) => {
                kept.push(rest.meta.to_token_stream());
                changed = true;
            }
            Walked::Removed => changed = true,
        }
    }
    if !changed {
        return Walked::Unchanged;
    }
    if kept.is_empty() {
        return Walked::Removed;
    }
    let tokens = quote!(#predicate, #(#kept),*);
    Walked::Rewritten(Attribute { meta: Meta::List(MetaList { tokens, ..cfg_attr.clone() }), ..attr.clone() })
}

/// The tokens inside an invisible group, as a `macro_rules!` `$attr:meta` leaves them; any other
/// tokens unchanged.
fn read_through(tokens: TokenStream) -> TokenStream {
    let mut iter = tokens.clone().into_iter();
    match (iter.next(), iter.next()) {
        (Some(TokenTree::Group(group)), None) if group.delimiter() == Delimiter::None => read_through(group.stream()),
        _ => tokens,
    }
}

/// The attributes among `attrs` that decide whether the item carrying them is compiled, each as an
/// outer attribute: every `cfg` as written, and every `cfg_attr` whose expansion holds a `cfg`,
/// reduced to the branches that hold one. The reduction keeps an attribute that applies only to a
/// method, such as `#[cfg_attr(test, cfg(unix), inline)]`'s `inline`, off the items a copy goes
/// on. Empty for an item compiled in every configuration.
///
/// An inner `#![cfg(..)]` in the method's body gates the method too, and its copy is written as
/// an outer attribute.
pub fn presence_gates(attrs: &[Attribute]) -> Vec<Attribute> {
    attrs.iter().filter_map(presence_gate).collect()
}

fn presence_gate(attr: &Attribute) -> Option<Attribute> {
    let path = attr.path();
    let gate = if path.is_ident("cfg") {
        attr.clone()
    } else if path.is_ident("cfg_attr") {
        let Meta::List(list) = &attr.meta else { return None };
        let tokens = reduce_cfg_attr(list.tokens.clone())?;
        Attribute { meta: Meta::List(MetaList { tokens, ..list.clone() }), ..attr.clone() }
    } else {
        return None;
    };
    Some(Attribute { style: AttrStyle::Outer, ..gate })
}

/// `cfg_attr`'s arguments, `<predicate>, <attr>, ..`, keeping the attributes that are a `cfg` or
/// a `cfg_attr` holding one. `None` when no attribute is kept or the predicate is missing, which
/// leaves the error to the original attribute.
fn reduce_cfg_attr(args: TokenStream) -> Option<TokenStream> {
    let mut pieces = split_commas(args).into_iter();
    let predicate = pieces.next().filter(|predicate| !predicate.is_empty())?;
    let kept: Vec<TokenStream> = pieces.filter_map(reduce_attr).collect();
    if kept.is_empty() {
        return None;
    }
    Some(quote!(#predicate, #(#kept),*))
}

/// One attribute inside a `cfg_attr`: itself when it is a `cfg`, its reduction when it is a
/// `cfg_attr`, `None` otherwise. An invisible group, as a `macro_rules!` `$attr:meta` leaves, is
/// read through.
fn reduce_attr(attr: TokenStream) -> Option<TokenStream> {
    let mut tokens = attr.clone().into_iter();
    match (tokens.next(), tokens.next(), tokens.next()) {
        (Some(TokenTree::Group(group)), None, None) if group.delimiter() == Delimiter::None => {
            reduce_attr(group.stream())
        }
        (Some(TokenTree::Ident(name)), Some(TokenTree::Group(args)), None)
            if args.delimiter() == Delimiter::Parenthesis =>
        {
            if name == "cfg" {
                Some(attr)
            } else if name == "cfg_attr" {
                let mut reduced = Group::new(Delimiter::Parenthesis, reduce_cfg_attr(args.stream())?);
                reduced.set_span(args.span());
                Some(quote!(#name #reduced))
            } else {
                None
            }
        }
        _ => None,
    }
}

/// `tokens` split at its top-level commas; a trailing comma yields no empty last piece.
fn split_commas(tokens: TokenStream) -> Vec<TokenStream> {
    let mut pieces = Vec::new();
    let mut current = TokenStream::new();
    for token in tokens {
        if matches!(&token, TokenTree::Punct(punct) if punct.as_char() == ',') {
            pieces.push(std::mem::take(&mut current));
        } else {
            current.extend([token]);
        }
    }
    if !current.is_empty() {
        pieces.push(current);
    }
    pieces
}
