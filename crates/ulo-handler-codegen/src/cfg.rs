//! The attributes that decide whether a handler is compiled, which `#[routes]` copies onto every
//! item it writes that names the handler's generated items.
//!
//! An attribute macro receives the items inside its input with their `#[cfg]` unevaluated, so
//! `#[routes]` sees a method behind `#[cfg(feature = "x")]` whether or not the feature is on. Rustc
//! evaluates that `cfg` after `#[routes]` has expanded, and with the feature off it removes the
//! method together with its transport attribute, which then writes none of
//! `__ULO_KEY_<name>`, `__ULO_CHECKS_<name>` and `__ulo_mount_<name>`.

use proc_macro2::{Delimiter, Group, TokenStream, TokenTree};
use quote::quote;
use syn::{AttrStyle, Attribute, Meta, MetaList};

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
