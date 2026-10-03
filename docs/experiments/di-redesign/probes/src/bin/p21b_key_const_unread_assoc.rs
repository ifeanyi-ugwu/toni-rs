//! P21b (X1): P21 with the key misspelled `htpp` and nothing reading the associated const. The
//! assertion sits inside the impl as a named associated const, and compiles: an associated const
//! is evaluated only where it is read, in a non-generic inherent impl too (1.88 and 1.98.1 alike).
//! So the X1 check cannot live inside the impl unread; it is a free `const _` naming the type
//! (P21e, and the proc-macro pair in the appendix), or `mount` reads it (P21d_fails). Expected:
//! compiles, prints "the misspelled key was never checked".
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

    #[allow(dead_code)]
    const __FW_KEYS_CHECK_htpp: () = assert!(
        key_in("htpp", &[Self::__FW_KEY_get, Self::__FW_KEY_get_rpc]),
        "`htpp` is not the key of any handler's transport in this impl (handlers: get, get_rpc)"
    );
}

fn main() {
    println!("the misspelled key was never checked");
}
