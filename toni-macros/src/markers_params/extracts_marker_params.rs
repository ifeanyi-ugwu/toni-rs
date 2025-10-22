use proc_macro2::TokenStream;
use quote::quote;
use syn::Result;

use super::get_marker_params::MarkerParam;

pub fn extract_body_from_param(marker_param: &MarkerParam) -> Result<TokenStream> {
    let param_name = &marker_param.param_name;
    let type_ident = &marker_param.type_ident;
    let extract_token_stream = quote! {
      let body = match req.body {
        Body::Json(json) => json.clone(),
        _ => ::serde_json::json!({}),
      };
      let #param_name: #type_ident = ::serde_json::from_value(body).unwrap();
    };
    Ok(extract_token_stream)
}

pub fn extract_query_from_param(marker_param: &MarkerParam) -> Result<TokenStream> {
    let param_name = &marker_param.param_name;
    let type_ident = &marker_param.type_ident;
    let marker_arg = &marker_param.marker_arg;
    let extract_token_stream = quote! {
      let #param_name: #type_ident = req.query_params.get(#marker_arg).unwrap().parse().unwrap();
    };
    Ok(extract_token_stream)
}

pub fn extract_path_param_from_param(marker_param: &MarkerParam) -> Result<TokenStream> {
    let param_name = &marker_param.param_name;
    let type_ident = &marker_param.type_ident;
    let marker_arg = &marker_param.marker_arg;
    let extract_token_stream = quote! {
      let #param_name: #type_ident = req.path_params.get(#marker_arg).unwrap().parse().unwrap();
    };
    Ok(extract_token_stream)
}
