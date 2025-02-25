use syn::{ImplItemFn, LitStr, Pat, Result, Type};

use crate::markers_params::remove_marker_controller_fn::is_marker;

#[derive(Debug)]
pub struct MarkerParam {
    pub param_name: syn::Ident,
    pub type_ident: syn::Ident,
    pub marker_name: String,
    pub marker_arg: Option<String>,
}

pub fn get_marker_params(method: &ImplItemFn) -> Result<Vec<MarkerParam>> {
    let mut marked_params = Vec::new();
    for input in method.sig.inputs.iter() {
        if let syn::FnArg::Typed(pat_type) = input {
            if !pat_type.attrs.is_empty() {
                if let Some(marker_ident) = pat_type.attrs[0].path().get_ident() {
                    if is_marker(marker_ident) {
                        let mut marker_arg = None;
                        if marker_ident.to_string() == "query"
                            || marker_ident.to_string() == "param"
                        {
                            marker_arg = Some(pat_type.attrs[0].parse_args::<LitStr>()?.value());
                        }
                        if let Pat::Ident(pat_ident) = &*pat_type.pat {
                            if let Type::Path(type_path) = &*pat_type.ty {
                                if let Some(last_segment) = type_path.path.segments.last() {
                                    marked_params.push(MarkerParam {
                                        param_name: pat_ident.ident.clone(),
                                        type_ident: last_segment.ident.clone(),
                                        marker_name: marker_ident.to_string(),
                                        marker_arg,
                                    });
                                }
                            }
                        }
                    }
                }
            }
        }
    }
    Ok(marked_params)
}
