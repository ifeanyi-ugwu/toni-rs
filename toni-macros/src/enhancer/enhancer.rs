use std::collections::HashMap;

use proc_macro2::TokenStream;
use quote::quote;
use syn::{Attribute, Error, Ident, Result, spanned::Spanned};

fn is_enhancer(segment: &Ident) -> bool {
    matches!(
        segment.to_string().as_str(),
        "use_guard" | "interceptor" | "pipe"
    )
}

pub fn has_enhancer_attribute(attr: &Attribute) -> bool {
    attr.path()
        .segments
        .iter()
        .any(|segment| is_enhancer(&segment.ident))
}

pub fn create_enchancers_token_stream(
    enhancers_attr: HashMap<&Ident, &Attribute>,
) -> Result<HashMap<String, Vec<TokenStream>>> {
    if enhancers_attr.is_empty() {
        return Ok(HashMap::new());
    }
    let mut enhancers: HashMap<String, Vec<TokenStream>> = HashMap::new();
    for (ident, attr) in enhancers_attr {
        let arg_ident = attr
            .parse_args::<Ident>()
            .map_err(|_| Error::new(attr.span(), "Invalid attribute format"))?;
        match enhancers.get_mut(ident.to_string().as_str()) {
            Some(enhancer_mut) => {
                enhancer_mut.push(quote! {::std::sync::Arc::new(#arg_ident)});
            }
            None => {
                enhancers.insert(ident.to_string(), vec![
                    quote! {::std::sync::Arc::new(#arg_ident)},
                ]);
            }
        };
    }
    Ok(enhancers)
}

pub fn get_enhancers_attr(attrs: &[Attribute]) -> Result<HashMap<&Ident, &Attribute>> {
    let mut enhancers_attr = HashMap::new();
    attrs.iter().for_each(|attr| {
        if has_enhancer_attribute(attr) {
            let ident = match attr.meta.path().get_ident() {
                Some(ident) => ident,
                None => return,
            };
            enhancers_attr.insert(ident, attr);
        }
    });
    Ok(enhancers_attr)
}
