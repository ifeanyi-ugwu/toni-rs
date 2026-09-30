// A singleton or execution-scoped `#[injectable]` is handed out as one shared `Arc`, which a field
// or parameter written as the plain type cannot hold. The refusal lands on the written type and
// names the `Arc<T>` to write.

#[ulo::injectable]
pub struct Store {}

#[ulo::injectable(scope = "execution")]
pub struct Call {}

#[ulo::injectable]
pub struct Holder {
    #[inject]
    store: Store,
}

// An alias names the same type, and the refusal reads the type.
type PlainStore = Store;

#[ulo::injectable]
pub struct AliasHolder {
    #[inject]
    store: PlainStore,
}

#[ulo::controller("/calls")]
pub struct Calls {
    #[inject]
    call: Call,
}

#[ulo::injectable]
pub struct Built {
    store: std::sync::Arc<Store>,
}

impl Built {
    #[ulo::new]
    fn new(store: Store) -> Self {
        Self {
            store: std::sync::Arc::new(store),
        }
    }
}

fn main() {}
