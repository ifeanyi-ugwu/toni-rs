use syn::{Ident, ImplItemFn};

pub fn is_marker(segment: &Ident) -> bool {
    matches!(
        segment.to_string().as_str(),
        "body" | "param" | "query" | "inject"
    )
}

pub fn remove_marker_in_controller_fn_args(method: &mut ImplItemFn) {
    for input in method.sig.inputs.iter_mut() {
        if let syn::FnArg::Typed(pat_type) = input {
            pat_type.attrs.retain(|attr| {
                !attr
                    .path()
                    .segments
                    .last()
                    .is_some_and(|seg| is_marker(&seg.ident))
            });
        }
    }
}
