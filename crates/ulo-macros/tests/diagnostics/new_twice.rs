// One type has one `#[new]` constructor. A second fails as a single duplicate definition, labelled
// at both attributes.

#[ulo::injectable]
pub struct Service {}

impl Service {
    #[ulo::new]
    fn create() -> Self {
        Self {}
    }

    #[ulo::new]
    fn build() -> Self {
        Self {}
    }
}

fn main() {}
