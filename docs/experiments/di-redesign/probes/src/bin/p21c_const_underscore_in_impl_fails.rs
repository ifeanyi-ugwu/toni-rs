//! P21c (X1): the design writes `const _: () = assert!(.. Self::__FW_KEY_get ..)`. `const _` is
//! a free item only; an associated const needs a name, so that spelling cannot sit inside the
//! impl where `Self` resolves, and outside it `Self` does not resolve. Expected: a parse error at
//! the `_` ("expected identifier, found reserved identifier `_`").
#![allow(non_upper_case_globals)]
pub struct UsersController;

impl UsersController {
    pub const __FW_KEY_get: &'static str = "http";
    const _: () = assert!(Self::__FW_KEY_get.len() == 4);
}

fn main() {}
