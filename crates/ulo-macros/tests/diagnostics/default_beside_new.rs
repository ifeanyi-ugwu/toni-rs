// A `#[new]` constructor sets every field, so a `#[default]` beside it never applies. Each struct
// macro refuses it at the attribute, naming the field and the constructor.

#[ulo::injectable]
pub struct Service {
    #[default(3)]
    limit: u8,
}

impl Service {
    #[ulo::new]
    fn new() -> Self {
        Self { limit: 1 }
    }
}

#[ulo::controller("/api")]
pub struct Api {
    #[default(3)]
    limit: u8,
}

impl Api {
    #[ulo::new]
    fn new() -> Self {
        Self { limit: 1 }
    }
}

#[ulo::websocket_gateway("/chat")]
pub struct Chat {
    #[default(3)]
    limit: u8,
}

impl Chat {
    #[ulo::new]
    fn new() -> Self {
        Self { limit: 1 }
    }
}

fn main() {}
