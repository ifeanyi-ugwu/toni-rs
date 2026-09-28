// A constructor is marked with `#[new]`. `init = "…"` is an unknown key on both attributes, refused
// naming `scope`.

#[ulo::injectable(init = "make")]
pub struct Service {}

#[ulo::controller("/c", init = "make")]
pub struct Api {}

fn main() {}
