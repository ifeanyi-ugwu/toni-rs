//! A type as a diagnostic prints it.

use quote::ToTokens;
use syn::Type;

/// `ty` as written, without the spaces token printing puts around `::`, `<`, `>` and `,`.
pub fn type_display(ty: &Type) -> String {
    ty.to_token_stream()
        .to_string()
        .replace(" :: ", "::")
        .replace(":: ", "::")
        .replace(" < ", "<")
        .replace("< ", "<")
        .replace(" <", "<")
        .replace(" >", ">")
        .replace(" ,", ",")
}
