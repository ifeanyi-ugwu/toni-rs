// A key position reads a type implementing `Key`. A type that does not is refused, naming the two
// ways to make one: a marker for a foreign type, the derive for a type of the crate.

#[derive(Clone)]
struct Plain(u8);

fn main() {
    let _ = ulo::provide!(Plain => Plain(1));
}
