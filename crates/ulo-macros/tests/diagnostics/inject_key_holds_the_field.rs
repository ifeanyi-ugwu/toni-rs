// `#[inject(K)]` fills a field holding what `K`'s slot holds. A field of another type is refused
// at the key, naming both.

ulo::key!(Port: u16);

#[ulo::injectable]
pub struct Server {
    #[inject(Port)]
    port: String,
}

fn main() {}
