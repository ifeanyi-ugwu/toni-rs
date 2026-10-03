//! P21d (X1): on a generic controller, `impl<T> Ctl<T>`, an associated const is evaluated only
//! where it is read with a concrete `T`. A failing assertion that nothing reads compiles, so the
//! X1 check on a generic impl needs a reader: `Controller::mount` referencing the const. Expected:
//! compiles and prints "generic check never ran"; P21d_fails adds the reader.
#![allow(non_upper_case_globals)]
pub struct Ctl<T>(pub T);

impl<T> Ctl<T> {
    pub const __FW_KEY_get: &'static str = "http";
    #[allow(dead_code)]
    const __FW_KEYS_CHECK_htpp: () = assert!(false, "`htpp` is not the key of any handler's transport in this impl");
}

fn main() {
    let _ = Ctl(1u8);
    println!("generic check never ran");
}
