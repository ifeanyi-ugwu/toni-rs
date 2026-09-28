// A marker names what its slot holds. A value of another type is refused at the value.

ulo::key!(Port: u16);

fn main() {
    let _ = ulo::provide!(Port => 3000u32);
}
