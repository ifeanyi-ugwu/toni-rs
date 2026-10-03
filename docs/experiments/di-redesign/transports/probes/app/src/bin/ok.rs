//! The outer macro's const sees the inner macro's associated consts. Expected: compiles, prints "ok".
use fwprobe_macros::{get_like, routes_like};

pub const fn key_in(key: &str, keys: &[&str]) -> bool {
    let k = key.as_bytes();
    let mut i = 0;
    while i < keys.len() {
        let c = keys[i].as_bytes();
        if c.len() == k.len() {
            let mut j = 0;
            let mut same = true;
            while j < c.len() {
                if c[j] != k[j] { same = false; }
                j += 1;
            }
            if same { return true; }
        }
        i += 1;
    }
    false
}

#[allow(non_upper_case_globals)]
pub struct UsersController;

#[routes_like("http")]
impl UsersController {
    #[get_like("http")]
    pub fn get(&self) {}

    #[get_like("rpc")]
    pub fn get_rpc(&self) {}
}

fn main() {
    UsersController::__fw_mount_get();
    println!("ok: {} {}", UsersController::__FW_KEY_get, UsersController::__FW_KEY_get_rpc);
}
