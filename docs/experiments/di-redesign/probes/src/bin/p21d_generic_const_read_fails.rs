//! P21d_fails (X1): P21d with `mount` reading the const, the way `#[routes]` would write it for a
//! generic controller. The assertion is evaluated when `mount` is instantiated. Expected: E0080 at
//! the `assert!`, reached from the `mount` instantiation.
#![allow(non_upper_case_globals)]
pub struct Ctl<T>(pub T);

impl<T> Ctl<T> {
    pub const __FW_KEY_get: &'static str = "http";
    const __FW_KEYS_CHECK_htpp: () = assert!(false, "`htpp` is not the key of any handler's transport in this impl");

    pub fn mount() {
        let () = Self::__FW_KEYS_CHECK_htpp;
    }
}

fn main() {
    Ctl::<u8>::mount();
}
