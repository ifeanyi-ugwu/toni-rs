// `#[inject(K)]` reads a key type. A type that is no key is refused at the key.

#[derive(Clone)]
pub struct Plain;

#[ulo::injectable]
pub struct Server {
    #[inject(Plain)]
    plain: Plain,
}

fn main() {}
