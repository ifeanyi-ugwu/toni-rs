//! P21e (X1): the shape that checks without a reader. `#[routes]` emits a free `const _` after
//! the impl, naming the controller type rather than `Self`, and spans the key token; a misspelled
//! key is then E0080 at that token with no use site. Expected: E0080 at the `assert!`, carrying
//! "`htpp` is not the key of any handler's transport in this impl (handlers: get, get_rpc)".
#![allow(non_upper_case_globals)]
pub const fn str_eq(a: &str, b: &str) -> bool {
    let (a, b) = (a.as_bytes(), b.as_bytes());
    if a.len() != b.len() {
        return false;
    }
    let mut i = 0;
    while i < a.len() {
        if a[i] != b[i] {
            return false;
        }
        i += 1;
    }
    true
}

pub const fn key_in(key: &str, keys: &[&str]) -> bool {
    let mut i = 0;
    while i < keys.len() {
        if str_eq(key, keys[i]) {
            return true;
        }
        i += 1;
    }
    false
}

pub struct UsersController;

impl UsersController {
    pub const __FW_KEY_get: &'static str = "http";
    pub fn get(&self) {}
    pub const __FW_KEY_get_rpc: &'static str = "rpc";
    pub fn get_rpc(&self) {}
}

const _: () = assert!(
    key_in("htpp", &[UsersController::__FW_KEY_get, UsersController::__FW_KEY_get_rpc]),
    "`htpp` is not the key of any handler's transport in this impl (handlers: get, get_rpc)"
);

fn main() {}
