// A key is a type. A string in `#[inject(..)]` is refused, naming the marker to declare instead.

#[ulo::injectable]
pub struct Server {
    #[inject("PORT")]
    port: u16,
}

fn main() {}
