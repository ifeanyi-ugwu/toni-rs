// A marker holding a role trait object holds only what implements the role. A value that is not a
// guard, provided under a guard slot, is refused naming the role it lacks.

ulo::key!(Auth: dyn ulo::enhancer::Guard<ulo::http::HttpContext>);

fn main() {
    let _ = ulo::provide!(Auth => 3u8);
}
