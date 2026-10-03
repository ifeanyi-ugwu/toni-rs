//! P21 (transports X1): a transport attribute emits `const __FW_KEY_<name>: &'static str` beside
//! each handler and `#[routes]` asserts every controller-level scoped key against them in a const.
//! `str == str` is not `const` on 1.88, so `key_in` compares bytes. Two placements: a named
//! associated const inside the impl, where `Self::` resolves (`const _` is not an associated item,
//! P21c) but which is evaluated only when read (P21b); and a free `const _` after the impl, which
//! names the type and is evaluated with no reader (P21e). Expected: compiles, prints
//! "http in [http, rpc]: true".
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
    // what `#[fw_http::get("/users/{id}")]` emits beside `get`
    pub const __FW_KEY_get: &'static str = "http";
    pub fn get(&self) {}

    // what `#[fw_rpc::message("users.get")]` emits beside `get_rpc`
    pub const __FW_KEY_get_rpc: &'static str = "rpc";
    pub fn get_rpc(&self) {}

    // what `#[routes]` emits for `#[guards(http = AuthGuard)]` on the impl
    #[allow(dead_code)]
    const __FW_KEYS_CHECK_http: () = assert!(
        key_in("http", &[Self::__FW_KEY_get, Self::__FW_KEY_get_rpc]),
        "`http` is not the key of any handler's transport in this impl (handlers: get, get_rpc)"
    );
}

// The same check as a free item has no `Self` and names the type.
const _: () = assert!(
    key_in("rpc", &[UsersController::__FW_KEY_get, UsersController::__FW_KEY_get_rpc]),
    "`rpc` is not the key of any handler's transport in this impl (handlers: get, get_rpc)"
);

fn main() {
    println!(
        "http in [http, rpc]: {}",
        key_in("http", &[UsersController::__FW_KEY_get, UsersController::__FW_KEY_get_rpc])
    );
}
