//! Attribute matching by last path segment, so `#[ulo::guards]` and `#[guards]` read alike.

use syn::Attribute;

pub(crate) fn is(attr: &Attribute, name: &str) -> bool {
    attr.path().segments.last().is_some_and(|s| s.ident == name)
}

/// Removes and returns every attribute named `name`, in order.
pub(crate) fn take_all(attrs: &mut Vec<Attribute>, name: &str) -> Vec<Attribute> {
    let mut taken = Vec::new();
    attrs.retain(|a| {
        if is(a, name) {
            taken.push(a.clone());
            false
        } else {
            true
        }
    });
    taken
}

/// Attributes that never make a method a handler: the language's own and the ulo enhancer
/// markers `#[routes]` consumes.
pub(crate) fn is_inert(attr: &Attribute) -> bool {
    const INERT: &[&str] = &[
        "doc", "allow", "warn", "deny", "expect", "cfg", "cfg_attr", "inline", "must_use", "deprecated",
        "track_caller", "guards", "interceptors", "error_handlers",
    ];
    INERT.iter().any(|name| is(attr, name))
}
